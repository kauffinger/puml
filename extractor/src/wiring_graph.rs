//! Turns what each file registers into what Laravel registers at boot. A registration counts when Laravel
//! runs the code it is written in: bootstrap/app.php, the route files it loads, and the methods of service
//! providers and kernels. Conventions follow the framework in vendor/: which HTTP verbs `any()` means,
//! which model events an observer can handle, how policies are named and whether listeners are discovered.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use mago_allocator::LocalArena;
use mago_database::file::File;
use mago_database::file::FileType;
use mago_names::resolver::NameResolver;
use mago_syntax::cst::Array;
use mago_syntax::cst::ArrayElement;
use mago_syntax::cst::Method as PhpMethod;
use mago_syntax::cst::PropertyConcreteItem;
use mago_syntax::parser::parse_file;
use mago_syntax::walker::Walker;

use crate::collect::RawEdge;
use crate::collect::collect_file;
use crate::graph::*;
use crate::laravel::string_keyed_classes;
use crate::php::Source;
use crate::project::Project;
use crate::routes;
use crate::routes::Group;
use crate::routes::RouteItem;
use crate::wiring::*;

const SERVICE_PROVIDER: &str = "Illuminate\\Support\\ServiceProvider";
const EVENT_PROVIDER: &str = "Illuminate\\Foundation\\Support\\Providers\\EventServiceProvider";
const AUTH_PROVIDER: &str = "Illuminate\\Foundation\\Support\\Providers\\AuthServiceProvider";
const CONSOLE_KERNEL: &str = "Illuminate\\Foundation\\Console\\Kernel";
const HTTP_KERNEL: &str = "Illuminate\\Foundation\\Http\\Kernel";
const MODEL: &str = "Illuminate\\Database\\Eloquent\\Model";
const COMMAND: &str = "Illuminate\\Console\\Command";
const OBSERVED_BY: &str = "Illuminate\\Database\\Eloquent\\Attributes\\ObservedBy";
const USE_POLICY: &str = "Illuminate\\Database\\Eloquent\\Attributes\\UsePolicy";
const AS_COMMAND: &str = "Symfony\\Component\\Console\\Attribute\\AsCommand";
const SIGNATURE: &str = "Illuminate\\Console\\Attributes\\Signature";
const BOOTSTRAP: &str = "bootstrap/app.php";

#[derive(Default)]
pub struct Wired {
    pub laravel: Laravel,
    pub edges: Vec<RawEdge>,
    pub files: Vec<ClassNode>,
    pub unresolved: Vec<Unresolved>,
}

/// What the installed framework does where Laravel versions differ.
struct Framework {
    verbs: Vec<String>,
    observable: Vec<String>,
    models_policies: bool,
    use_policy: bool,
    use_policy_parents: bool,
    discovers_by_default: bool,
    aliases: HashMap<String, String>,
}

impl Framework {
    fn read(project: &Project) -> Framework {
        let source = |class: &str| project.vendor.file_for(class).and_then(|path| std::fs::read_to_string(path).ok());
        let gate = source("Illuminate\\Auth\\Access\\Gate");
        let events = source(EVENT_PROVIDER);

        let discovers_by_default = match &events {
            Some(text) => text
                .split("function shouldDiscoverEvents")
                .nth(1)
                .and_then(|body| body.split('}').next())
                .is_some_and(|body| body.contains("__CLASS__")),
            None => true,
        };

        Framework {
            verbs: vendor_strings(project, "Illuminate\\Routing\\Router", "verbs")
                .unwrap_or_else(|| ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "QUERY"].map(str::to_string).to_vec()),
            observable: vendor_strings(project, "Illuminate\\Database\\Eloquent\\Concerns\\HasEvents", "getObservableEvents").unwrap_or_else(|| {
                [
                    "retrieved", "creating", "created", "updating", "updated", "saving", "saved", "restoring", "restored", "replicating", "trashed",
                    "deleting", "deleted", "forceDeleting", "forceDeleted",
                ]
                .map(str::to_string)
                .to_vec()
            }),
            models_policies: gate.as_ref().is_none_or(|text| text.contains("\\\\Models\\\\Policies\\\\")),
            use_policy: gate.as_ref().is_none_or(|text| text.contains("getPolicyFromAttribute")),
            use_policy_parents: gate.as_ref().is_none_or(|text| text.contains("includeParents: true")),
            discovers_by_default,
            aliases: project
                .vendor
                .file_for("Illuminate\\Foundation\\Configuration\\Middleware")
                .map(|path| string_keyed_classes(&path, "Illuminate\\Foundation\\Configuration\\Middleware", "defaultAliases"))
                .unwrap_or_default(),
        }
    }
}

struct Index<'a> {
    project: &'a Project,
    classes: HashMap<String, &'a ClassNode>,
    parents: HashMap<String, String>,
    interfaces: HashMap<String, Vec<String>>,
    vendor_parents: RefCell<HashMap<String, Option<String>>>,
}

impl<'a> Index<'a> {
    fn new(project: &'a Project, classes: &'a [ClassNode], edges: &[RawEdge]) -> Self {
        let mut parents = HashMap::new();
        let mut interfaces: HashMap<String, Vec<String>> = HashMap::new();

        for edge in edges {
            match edge.kind {
                EdgeKind::Extends => {
                    parents.entry(edge.from.to_ascii_lowercase()).or_insert_with(|| edge.to.clone());
                }
                EdgeKind::Implements => interfaces.entry(edge.from.to_ascii_lowercase()).or_default().push(edge.to.clone()),
                _ => {}
            }
        }

        Index {
            project,
            classes: classes.iter().map(|class| (class.id.to_ascii_lowercase(), class)).collect(),
            parents,
            interfaces,
            vendor_parents: RefCell::new(HashMap::new()),
        }
    }

    fn class(&self, id: &str) -> Option<&'a ClassNode> {
        self.classes.get(&id.to_ascii_lowercase()).copied()
    }

    fn parent(&self, id: &str) -> Option<String> {
        let key = id.to_ascii_lowercase();

        if self.classes.contains_key(&key) {
            return self.parents.get(&key).cloned();
        }

        if let Some(parent) = self.vendor_parents.borrow().get(&key) {
            return parent.clone();
        }

        let parent = self.project.vendor.file_for(id).and_then(|path| {
            let facts = collect_file(&path, &self.project.relative(&path));

            facts.edges.into_iter().find(|edge| edge.kind == EdgeKind::Extends && edge.from.eq_ignore_ascii_case(id)).map(|edge| edge.to)
        });

        self.vendor_parents.borrow_mut().insert(key, parent.clone());

        parent
    }

    fn ancestors(&self, id: &str) -> Vec<String> {
        let mut ancestors = Vec::new();
        let mut current = id.to_string();

        while let Some(parent) = self.parent(&current) {
            if ancestors.len() > 32 {
                break;
            }

            ancestors.push(parent.clone());
            current = parent;
        }

        ancestors
    }

    fn descends(&self, id: &str, base: &str) -> bool {
        self.ancestors(id).iter().any(|ancestor| ancestor.eq_ignore_ascii_case(base))
    }

    /// The methods PHP finds on a class: its own, then those of its app parents.
    fn methods(&self, id: &str) -> Vec<&'a Method> {
        let mut methods: Vec<&Method> = Vec::new();

        for class in std::iter::once(id.to_string()).chain(self.ancestors(id)).filter_map(|id| self.class(&id)) {
            for method in &class.methods {
                if !methods.iter().any(|known| known.name.eq_ignore_ascii_case(&method.name)) {
                    methods.push(method);
                }
            }
        }

        methods
    }

    fn has_method(&self, id: &str, name: &str) -> bool {
        self.methods(id).iter().any(|method| method.name.eq_ignore_ascii_case(name))
    }

    fn exists(&self, id: &str) -> bool {
        self.class(id).is_some()
    }

    /// Every class `id` is a subclass of in PHP's sense: its parents and the interfaces they implement.
    fn supertypes(&self, id: &str) -> Vec<String> {
        let mut pending: Vec<String> = std::iter::once(id.to_string()).chain(self.ancestors(id)).collect();
        let mut seen: Vec<String> = Vec::new();

        while let Some(next) = pending.pop() {
            if seen.iter().any(|known| known.eq_ignore_ascii_case(&next)) {
                continue;
            }

            pending.extend(self.interfaces.get(&next.to_ascii_lowercase()).cloned().unwrap_or_default());

            if let Some(parent) = self.parents.get(&next.to_ascii_lowercase()) {
                pending.push(parent.clone());
            }

            seen.push(next);
        }

        seen.into_iter().filter(|known| !known.eq_ignore_ascii_case(id)).collect()
    }
}

struct Registration<'f> {
    file: &'f str,
    registered: &'f Registered,
}

pub fn wire(project: &Project, classes: &[ClassNode], edges: &[RawEdge], files: &[(String, &WiringFacts)]) -> Wired {
    let framework = Framework::read(project);
    let index = Index::new(project, classes, edges);
    let mut wired = Wired::default();
    let facts: HashMap<&str, &WiringFacts> = files.iter().map(|(path, facts)| (path.as_str(), *facts)).collect();
    let bootstrap = facts.get(BOOTSTRAP).and_then(|facts| facts.bootstrap.clone()).unwrap_or_default();

    let providers = registered_providers(project, files, &bootstrap);
    let kernel = |contract: &str| {
        facts.get(BOOTSTRAP).into_iter().flat_map(|facts| facts.registrations.iter()).find_map(|registered| match &registered.wire {
            Wire::Bind { key, concrete: Concrete::Class(class), .. } if registered.class.is_none() && key.eq_ignore_ascii_case(contract) => Some(class.to_ascii_lowercase()),
            _ => None,
        })
    };
    let console_kernel = kernel("Illuminate\\Contracts\\Console\\Kernel");
    let http_kernel = kernel("Illuminate\\Contracts\\Http\\Kernel");

    let mut roles: HashMap<String, Vec<&'static str>> = HashMap::new();
    let role_of = |id: &str| -> Vec<&'static str> {
        let key = id.to_ascii_lowercase();
        let registered = providers.contains(&key);

        [
            (SERVICE_PROVIDER, "provider", registered),
            (EVENT_PROVIDER, "events", registered),
            (AUTH_PROVIDER, "auth", registered),
            (CONSOLE_KERNEL, "console", console_kernel.as_deref() == Some(key.as_str())),
            (HTTP_KERNEL, "http", http_kernel.as_deref() == Some(key.as_str())),
            (MODEL, "model", true),
            (COMMAND, "command", true),
        ]
        .into_iter()
        .filter(|(base, _, used)| *used && index.descends(id, base))
        .map(|(_, role, _)| role)
        .collect()
    };

    for class in classes.iter().filter(|class| !class.external) {
        roles.insert(class.id.to_ascii_lowercase(), role_of(&class.id));
    }

    let is = |id: &str, role: &str| roles.get(&id.to_ascii_lowercase()).is_some_and(|roles| roles.contains(&role));

    // Routes, and with them the route files Laravel loads.
    let route_files: HashMap<String, Vec<RouteItem>> = files
        .iter()
        .map(|(path, facts)| (path.clone(), facts.routes.iter().filter(|scope| scope.class.is_none()).flat_map(|scope| scope.items.clone()).collect()))
        .collect();

    let mut collector = RouteCollector { project, route_files: &route_files, verbs: &framework.verbs, routes: Vec::new(), unresolved: Vec::new(), loaded: BTreeSet::new() };

    if let Some(routing) = &bootstrap.routing {
        match &routing.using {
            Some(using) => collector.collect(using, &[], BOOTSTRAP, BOOTSTRAP, 0),
            None => {
                let mut items: Vec<RouteItem> = Vec::new();

                for api in &routing.api {
                    let group = Group { middleware: vec!["api".to_string()], prefix: Some(routing.api_prefix.clone()), ..Group::default() };
                    items.push(RouteItem::Include { groups: vec![group], path: api.clone(), line: routing.line });
                }

                for web in &routing.web {
                    let group = Group { middleware: vec!["web".to_string()], ..Group::default() };
                    items.push(RouteItem::Include { groups: vec![group], path: web.clone(), line: routing.line });
                }

                items.extend(routing.then.iter().cloned());
                collector.collect(&items, &[], BOOTSTRAP, BOOTSTRAP, 0);
            }
        }
    }

    for (path, file_facts) in files {
        for scope in &file_facts.routes {
            if let Some(class) = scope.class.as_deref().filter(|class| is(class, "provider")) {
                collector.collect(&scope.items, &[], path, class, 0);
            }
        }
    }

    for package in project.vendor.packages.iter().filter(|package| !project.dont_discover.iter().any(|name| name == "*" || *name == package.name)) {
        for provider in &package.providers {
            let Some(path) = project.vendor.file_for(provider) else {
                continue;
            };

            let relative = project.relative(&path);

            for scope in collect_path(&path, &relative).map(|facts| facts.routes).unwrap_or_default() {
                let app_files: Vec<RouteItem> = scope
                    .items
                    .into_iter()
                    .filter(|item| matches!(item, RouteItem::Include { path, .. } if !path.starts_with("vendor/") && project.is_file(path)))
                    .collect();

                collector.collect(&app_files, &[], &relative, provider, 0);
            }
        }
    }

    // Laravel keeps one route per methods, domain and URI: the one registered last.
    let mut registered: HashSet<(Vec<String>, Option<String>, String)> = HashSet::new();
    let mut routes = collector.routes;

    routes.reverse();
    routes.retain(|route| {
        let mut methods = route.methods.clone();
        methods.sort();

        registered.insert((methods, route.domain.clone(), route.uri.clone()))
    });

    let loaded = collector.loaded.clone();
    wired.laravel.routes = routes;
    wired.unresolved.extend(collector.unresolved);

    // Files the console kernel loads: routes/console.php and the like.
    let mut console: BTreeSet<String> = BTreeSet::new();
    console.extend(bootstrap.routing.as_ref().and_then(|routing| routing.commands.clone()));
    console.extend(bootstrap.commands.iter().filter(|path| path.ends_with(".php")).cloned());

    for (_, file_facts) in files {
        for registered in &file_facts.registrations {
            if let (Some(class), Wire::Require { path }) = (&registered.class, &registered.wire) {
                if is(class, "console") {
                    console.insert(path.clone());
                }
            }
        }
    }

    console.retain(|path| project.is_file(path));

    let runs_at_boot = |file: &str, registered: &Registered| -> bool {
        match &registered.class {
            None => file == BOOTSTRAP || loaded.contains(file) || console.contains(file),
            Some(class) => match &registered.wire {
                Wire::Schedule { .. } => is(class, "console") || is(class, "provider"),
                Wire::Observe { .. } => {
                    is(class, "provider") || (is(class, "model") && registered.method.as_deref().is_some_and(|method| method.eq_ignore_ascii_case("boot") || method.eq_ignore_ascii_case("booted")))
                }
                Wire::Listen { .. } if registered.method.as_deref().is_some_and(|method| method.eq_ignore_ascii_case("subscribe")) => true,
                _ => is(class, "provider"),
            },
        }
    };

    let registrations: Vec<Registration> = files
        .iter()
        .flat_map(|(path, file_facts)| file_facts.registrations.iter().map(move |registered| Registration { file: path, registered }))
        .filter(|registration| runs_at_boot(registration.file, registration.registered))
        .collect();

    let properties = |role: &str, name: &str| -> Vec<(String, &PropertyValue)> {
        files
            .iter()
            .flat_map(|(path, file_facts)| file_facts.properties.iter().map(move |property| (path.clone(), property)))
            .filter(|(_, property)| property.name == name && is(&property.class, role))
            .collect()
    };

    let returns: Vec<(&String, &MethodReturn)> = files.iter().flat_map(|(path, file_facts)| file_facts.returns.iter().map(move |value| (path, value))).collect();
    let attributes: Vec<(&String, &ClassAttribute)> = files.iter().flat_map(|(path, file_facts)| file_facts.attributes.iter().map(move |value| (path, value))).collect();

    for registration in &registrations {
        if let Wire::Unresolved { kind, detail } = &registration.registered.wire {
            wired.unresolved.push(Unresolved { kind, detail: detail.clone(), file: registration.file.to_string(), line: registration.registered.line });
        }
    }

    listeners(&index, &framework, &bootstrap, &registrations, &properties, &returns, &is, &mut wired);
    observers(&index, &framework, classes, &registrations, &properties, &attributes, &is, &mut wired);
    policies(&index, &framework, classes, &registrations, &properties, &attributes, &mut wired);
    bindings(project, &index, &registrations, &properties, &mut wired);
    schedule(&index, classes, &registrations, &properties, &attributes, &is, &mut wired);
    middleware(&framework, &registrations, &properties, &mut wired);

    edges_and_files(project, &mut wired);

    wired
}

/// The service providers Laravel registers: those bootstrap/providers.php, config/app.php and
/// `withProviders()` list, and those a registered provider registers with `$this->app->register()`.
fn registered_providers(project: &Project, files: &[(String, &WiringFacts)], bootstrap: &Bootstrap) -> HashSet<String> {
    let mut providers: HashSet<String> = listed_classes(project, "bootstrap/providers.php", None)
        .into_iter()
        .chain(listed_classes(project, "config/app.php", Some("providers")))
        .chain(bootstrap.providers.iter().cloned())
        .map(|class| class.to_ascii_lowercase())
        .collect();

    loop {
        let mut added = false;

        for (_, file_facts) in files {
            for registered in &file_facts.registrations {
                if let (Some(class), Wire::Provider { class: provider }) = (&registered.class, &registered.wire) {
                    if providers.contains(&class.to_ascii_lowercase()) {
                        added |= providers.insert(provider.to_ascii_lowercase());
                    }
                }
            }
        }

        if !added {
            return providers;
        }
    }
}

struct RouteCollector<'a> {
    project: &'a Project,
    route_files: &'a HashMap<String, Vec<RouteItem>>,
    verbs: &'a [String],
    routes: Vec<RouteFact>,
    unresolved: Vec<Unresolved>,
    loaded: BTreeSet<String>,
}

impl RouteCollector<'_> {
    fn collect(&mut self, items: &[RouteItem], outer: &[Group], file: &str, from: &str, depth: usize) {
        for item in items {
            match item {
                RouteItem::Route(registration) => {
                    for route in routes::compile(registration, outer, self.verbs) {
                        self.routes.push(RouteFact {
                            from: from.to_string(),
                            methods: route.methods,
                            uri: route.uri,
                            domain: route.domain,
                            name: route.name,
                            action: route.action,
                            target: route.target,
                            middleware: route.middleware,
                            excluded_middleware: route.excluded_middleware,
                            file: file.to_string(),
                            line: route.line,
                        });
                    }
                }
                RouteItem::Include { groups, path, line } => {
                    let items = self.route_files.get(path).cloned().or_else(|| read_route_file(self.project, path));

                    match items {
                        Some(items) if depth < 16 => {
                            self.loaded.insert(path.clone());
                            self.collect(&items, &[outer, groups].concat(), path, path, depth + 1);
                        }
                        _ => self.unresolved.push(Unresolved {
                            kind: "route",
                            detail: format!("the route file {path} cannot be read"),
                            file: file.to_string(),
                            line: *line,
                        }),
                    }
                }
                RouteItem::Unresolved { detail, line } => {
                    self.unresolved.push(Unresolved { kind: "route", detail: detail.clone(), file: file.to_string(), line: *line });
                }
            }
        }
    }
}

type Properties<'p> = dyn Fn(&str, &str) -> Vec<(String, &'p PropertyValue)> + 'p;

#[allow(clippy::too_many_arguments)]
fn listeners<'p>(
    index: &Index<'_>,
    framework: &Framework,
    bootstrap: &Bootstrap,
    registrations: &[Registration<'_>],
    properties: &Properties<'p>,
    returns: &[(&String, &MethodReturn)],
    is: &dyn Fn(&str, &str) -> bool,
    wired: &mut Wired,
) {
    let mut found: Vec<ListenerFact> = Vec::new();
    let mut directories: Vec<String> = Vec::new();

    if bootstrap.configure && framework.discovers_by_default {
        match &bootstrap.discover {
            Some(Discovery::Disabled) => {}
            Some(Discovery::Paths(paths)) => directories.extend(paths.iter().cloned()),
            None => directories.push("app/Listeners".to_string()),
        }
    }

    for (_, value) in returns {
        if value.method.eq_ignore_ascii_case("shouldDiscoverEvents") && matches!(value.value, Returned::Bool(true)) && is(&value.class, "events") {
            let within = returns.iter().find_map(|(_, other)| match &other.value {
                Returned::Paths(paths) if other.class == value.class && other.method.eq_ignore_ascii_case("discoverEventsWithin") => Some(paths.clone()),
                _ => None,
            });

            directories.extend(within.unwrap_or_else(|| vec!["app/Listeners".to_string()]));
        }
    }

    for directory in directories.iter().flat_map(|pattern| glob_directories(&index.project.root, pattern)) {
        for class in index.classes.values() {
            let Some(file) = &class.file else {
                continue;
            };

            if class.kind != "class" || class.r#abstract || !file.starts_with(&format!("{directory}/")) || !named_by_path(class, file) {
                continue;
            }

            let methods = index.methods(&class.id);

            if methods.iter().any(|method| method.name.eq_ignore_ascii_case("__construct") && method.visibility != "public") {
                continue;
            }

            for method in methods {
                let listens = method.visibility == "public" && (method.name.starts_with("handle") || method.name == "__invoke") && !method.params.is_empty();

                if !listens {
                    continue;
                }

                for event in &method.first_parameter {
                    found.push(ListenerFact {
                        event: event.clone(),
                        listener: format!("{}@{}", class.id, method.name),
                        class: Some(class.id.clone()),
                        how: "discovered",
                        file: file.clone(),
                        line: method.line,
                    });
                }
            }
        }
    }

    let normalized = |listener: &Listener, owner: Option<&str>| -> Option<(String, Option<String>)> {
        match listener {
            Listener::Closure => Some(("closure".to_string(), owner.map(str::to_string))),
            Listener::Method(class, method) => Some((format!("{class}@{method}"), Some(class.clone()))),
            Listener::Class(class) => {
                let method = if !index.has_method(class, "handle") && index.has_method(class, "__invoke") { "__invoke" } else { "handle" };

                Some((format!("{class}@{method}"), Some(class.clone())))
            }
            Listener::Own(method) => {
                let owner = owner?;

                index.has_method(owner, method).then(|| (format!("{owner}@{method}"), Some(owner.to_string())))
            }
        }
    };

    let mut subscribers: Vec<(String, String, u32)> = Vec::new();

    for registration in registrations {
        let registered = registration.registered;

        match &registered.wire {
            Wire::Listen { events, listener } => {
                if let Some((listener, class)) = normalized(listener, registered.class.as_deref()) {
                    for event in events {
                        found.push(ListenerFact {
                            event: event.clone(),
                            listener: listener.clone(),
                            class: class.clone(),
                            how: if registered.method.as_deref().is_some_and(|method| method.eq_ignore_ascii_case("subscribe")) && !is(registered.class.as_deref().unwrap_or_default(), "provider") { "subscriber-listen" } else { "listen" },
                            file: registration.file.to_string(),
                            line: registered.line,
                        });
                    }
                }
            }
            Wire::Subscribe { subscriber } => subscribers.push((subscriber.clone(), registration.file.to_string(), registered.line)),
            _ => {}
        }
    }

    for (file, property) in properties("events", "listen") {
        let Value::Map(pairs) = &property.value else {
            continue;
        };

        for (event, listeners) in pairs {
            for listener in listeners {
                let parsed = match listener.split_once('@') {
                    Some((class, method)) => Listener::Method(class.to_string(), method.to_string()),
                    None => Listener::Class(listener.clone()),
                };

                if let Some((listener, class)) = normalized(&parsed, None) {
                    found.push(ListenerFact { event: event.clone(), listener, class, how: "listen", file: file.clone(), line: property.line });
                }
            }
        }
    }

    for (file, property) in properties("events", "subscribe") {
        if let Value::List(classes) = &property.value {
            subscribers.extend(classes.iter().map(|class| (class.clone(), file.clone(), property.line)));
        }
    }

    for (subscriber, _, _) in &subscribers {
        let Some(class) = index.class(subscriber) else {
            continue;
        };

        let file = class.file.clone().unwrap_or_default();

        for (_, value) in returns {
            if !value.class.eq_ignore_ascii_case(subscriber) || !value.method.eq_ignore_ascii_case("subscribe") {
                continue;
            }

            if let Returned::Listeners(pairs) = &value.value {
                for (event, listener) in pairs {
                    if let Some((listener, class)) = normalized(listener, Some(&class.id)) {
                        found.push(ListenerFact { event: event.clone(), listener, class, how: "subscribe", file: file.clone(), line: value.line });
                    }
                }
            }
        }
    }

    let subscribed: BTreeSet<String> = subscribers.iter().map(|(subscriber, _, _)| subscriber.to_ascii_lowercase()).collect();

    found.retain(|fact| fact.how != "subscriber-listen" || fact.class.as_ref().is_some_and(|class| subscribed.contains(&class.to_ascii_lowercase())));

    for fact in &mut found {
        if fact.how == "subscriber-listen" {
            fact.how = "subscribe";
        }
    }

    wired.laravel.listeners.extend(found);
}

#[allow(clippy::too_many_arguments)]
fn observers<'p>(
    index: &Index<'_>,
    framework: &Framework,
    classes: &[ClassNode],
    registrations: &[Registration<'_>],
    properties: &Properties<'p>,
    attributes: &[(&String, &ClassAttribute)],
    is: &dyn Fn(&str, &str) -> bool,
    wired: &mut Wired,
) {
    let mut found: Vec<(String, String, &'static str, String, u32)> = Vec::new();

    for registration in registrations {
        if let Wire::Observe { model, observers } = &registration.registered.wire {
            for observer in observers {
                found.push((model.clone(), observer.clone(), "observe", registration.file.to_string(), registration.registered.line));
            }
        }
    }

    for (file, property) in properties("events", "observers") {
        if let Value::Map(pairs) = &property.value {
            for (model, observers) in pairs {
                for observer in observers {
                    found.push((model.clone(), observer.clone(), "property", file.clone(), property.line));
                }
            }
        }
    }

    for model in classes.iter().filter(|class| !class.external && class.kind == "class" && !class.r#abstract && is(&class.id, "model")) {
        let mut chain = vec![model.id.clone()];

        while let Some(parent) = index.parent(chain.last().expect("the chain starts with the model")) {
            if parent.eq_ignore_ascii_case(MODEL) || chain.len() > 32 {
                break;
            }

            chain.push(parent);
        }

        for declaring in chain.iter().rev() {
            for (file, attribute) in attributes {
                if attribute.class.eq_ignore_ascii_case(declaring) && attribute.attribute.eq_ignore_ascii_case(OBSERVED_BY) {
                    for (_, values) in &attribute.arguments {
                        for observer in values {
                            found.push((model.id.clone(), observer.clone(), "attribute", (*file).clone(), attribute.line));
                        }
                    }
                }
            }
        }
    }

    for (model, observer, how, file, line) in found {
        if index.class(&observer).is_none() {
            wired.unresolved.push(Unresolved {
                kind: "observer",
                detail: format!("{observer} observes {model} but is not a class of this project, so its methods are not read"),
                file,
                line,
            });

            continue;
        }

        let extra = properties("model", "observables")
            .into_iter()
            .filter(|(_, property)| property.class.eq_ignore_ascii_case(&model))
            .flat_map(|(_, property)| match &property.value {
                Value::List(events) => events.clone(),
                _ => Vec::new(),
            });

        let events: Vec<String> =
            framework.observable.iter().cloned().chain(extra).filter(|event| index.has_method(&observer, event)).collect();

        wired.laravel.observers.push(ObserverFact { model, observer, events, how, file, line });
    }
}

fn policies<'p>(
    index: &Index<'_>,
    framework: &Framework,
    classes: &[ClassNode],
    registrations: &[Registration<'_>],
    properties: &Properties<'p>,
    attributes: &[(&String, &ClassAttribute)],
    wired: &mut Wired,
) {
    let mut explicit: Vec<(String, String, String, u32)> = Vec::new();
    let mut guessed_by_callback = None;

    for (file, property) in properties("auth", "policies") {
        if let Value::Map(pairs) = &property.value {
            for (model, policies) in pairs {
                if let Some(policy) = policies.first() {
                    explicit.push((model.clone(), policy.clone(), file.clone(), property.line));
                }
            }
        }
    }

    for registration in registrations {
        match &registration.registered.wire {
            Wire::Policy { model, policy } => explicit.push((model.clone(), policy.clone(), registration.file.to_string(), registration.registered.line)),
            Wire::GuessPolicyNames => guessed_by_callback = Some((registration.file.to_string(), registration.registered.line)),
            _ => {}
        }
    }

    let mut map: BTreeMap<String, (String, String, String, u32)> = BTreeMap::new();

    for (model, policy, file, line) in explicit {
        map.insert(model.to_ascii_lowercase(), (model, policy, file, line));
    }

    if let Some((file, line)) = &guessed_by_callback {
        wired.unresolved.push(Unresolved {
            kind: "policy",
            detail: "Gate::guessPolicyNamesUsing() replaces the policy naming convention, so policies found by name are not listed".to_string(),
            file: file.clone(),
            line: *line,
        });
    }

    let attribute_policy = |class: &str| {
        attributes.iter().find(|(_, attribute)| attribute.class.eq_ignore_ascii_case(class) && attribute.attribute.eq_ignore_ascii_case(USE_POLICY)).and_then(|(file, attribute)| {
            attribute.arguments.first().and_then(|(_, values)| values.first()).map(|policy| (policy.clone(), (*file).clone(), attribute.line))
        })
    };

    for class in classes.iter().filter(|class| !class.external && class.kind != "trait") {
        let key = class.id.to_ascii_lowercase();

        if let Some((_, policy, file, line)) = map.get(&key) {
            wired.laravel.policies.push(PolicyFact { model: class.id.clone(), policy: policy.clone(), how: "gate", file: file.clone(), line: *line });
            continue;
        }

        if framework.use_policy {
            if let Some((policy, file, line)) = attribute_policy(&class.id) {
                wired.laravel.policies.push(PolicyFact { model: class.id.clone(), policy, how: "attribute", file, line });
                continue;
            }
        }

        if guessed_by_callback.is_none() {
            if let Some(policy) = guess_policy(&class.id, framework).into_iter().find(|candidate| index.exists(candidate)) {
                let declared = index.class(&policy).expect("the guessed policy exists");

                wired.laravel.policies.push(PolicyFact {
                    model: class.id.clone(),
                    policy: declared.id.clone(),
                    how: "convention",
                    file: declared.file.clone().unwrap_or_default(),
                    line: declared.line.unwrap_or_default(),
                });
                continue;
            }
        }

        let supertypes = index.supertypes(&class.id);

        if let Some((_, policy, file, line)) = map.values().find(|(model, ..)| supertypes.iter().any(|supertype| supertype.eq_ignore_ascii_case(model))) {
            wired.laravel.policies.push(PolicyFact { model: class.id.clone(), policy: policy.clone(), how: "gate", file: file.clone(), line: *line });
            continue;
        }

        if framework.use_policy_parents {
            if let Some((policy, file, line)) = supertypes.iter().find_map(|supertype| attribute_policy(supertype)) {
                wired.laravel.policies.push(PolicyFact { model: class.id.clone(), policy, how: "attribute", file, line });
            }
        }
    }

    for (key, (model, policy, file, line)) in &map {
        if index.class(key).is_none() {
            wired.laravel.policies.push(PolicyFact { model: model.clone(), policy: policy.clone(), how: "gate", file: file.clone(), line: *line });
        }
    }
}

/// `Gate::guessPolicyName`: the policies Laravel looks for, the one it tries first first.
fn guess_policy(class: &str, framework: &Framework) -> Vec<String> {
    let (directory, short) = split_name(class);
    let segments: Vec<&str> = directory.split('\\').collect();
    let mut candidates: Vec<String> = (1..=segments.len()).map(|count| format!("{}\\Policies\\{short}Policy", segments[..count].join("\\"))).collect();

    if framework.models_policies && directory.contains("\\Models\\") {
        candidates.push(format!("{}\\{short}Policy", directory.replace("\\Models\\", "\\Policies\\")));
        candidates.push(format!("{}\\{short}Policy", directory.replace("\\Models\\", "\\Models\\Policies\\")));
    }

    candidates.reverse();

    candidates
}

fn bindings<'p>(project: &Project, index: &Index<'_>, registrations: &[Registration<'_>], properties: &Properties<'p>, wired: &mut Wired) {
    let mut push = |key: String, concrete: Concrete, lifetime: &'static str, file: String, line: u32, unresolved: &mut Vec<Unresolved>| {
        let concrete = match concrete {
            Concrete::Same => Some(key.clone()),
            Concrete::Class(class) => Some(class),
            Concrete::Parameter(_) => {
                unresolved.push(Unresolved { kind: "binding", detail: format!("{key} is bound to a parameter, which is only known at runtime"), file: file.clone(), line });
                None
            }
            Concrete::Unresolved(detail) => {
                unresolved.push(Unresolved { kind: "binding", detail, file: file.clone(), line });
                None
            }
        };

        wired.laravel.bindings.push(BindingFact { key, concrete, lifetime, file, line });
    };

    let mut unresolved = Vec::new();

    let mut helpers: HashMap<String, Vec<Registered>> = HashMap::new();

    for registration in registrations {
        match &registration.registered.wire {
            Wire::Bind { lifetime, key, concrete } => {
                push(key.clone(), concrete.clone(), lifetime, registration.file.to_string(), registration.registered.line, &mut unresolved);
            }
            Wire::Call { class, method, arguments } if !index.exists(class) => {
                let bound = helpers.entry(class.to_ascii_lowercase()).or_insert_with(|| {
                    project
                        .vendor
                        .file_for(class)
                        .and_then(|path| collect_path(&path, &project.relative(&path)))
                        .map(|facts| facts.registrations)
                        .unwrap_or_default()
                });

                for helper in bound.iter().filter(|helper| helper.class.as_deref().is_some_and(|owner| owner.eq_ignore_ascii_case(class)) && helper.method.as_deref().is_some_and(|name| name.eq_ignore_ascii_case(method))) {
                    let Wire::Bind { lifetime, key, concrete } = &helper.wire else {
                        continue;
                    };

                    let concrete = match concrete {
                        Concrete::Parameter(position) => match arguments.get(*position).cloned().flatten() {
                            Some(class) => Concrete::Class(class),
                            None => continue,
                        },
                        other => other.clone(),
                    };

                    push(key.clone(), concrete, lifetime, registration.file.to_string(), registration.registered.line, &mut unresolved);
                }
            }
            _ => {}
        }
    }

    for (name, lifetime) in [("bindings", "bind"), ("singletons", "singleton")] {
        for (file, property) in properties("provider", name) {
            let pairs = match &property.value {
                Value::Map(pairs) => pairs.clone(),
                Value::List(classes) => classes.iter().map(|class| (class.clone(), vec![class.clone()])).collect(),
                Value::Text(_) => Vec::new(),
            };

            for (key, concrete) in pairs {
                let concrete = concrete.first().cloned().map_or(Concrete::Same, Concrete::Class);
                push(key, concrete, lifetime, file.clone(), property.line, &mut unresolved);
            }
        }
    }

    wired.unresolved.extend(unresolved);
}

fn schedule<'p>(
    index: &Index<'_>,
    classes: &[ClassNode],
    registrations: &[Registration<'_>],
    properties: &Properties<'p>,
    attributes: &[(&String, &ClassAttribute)],
    is: &dyn Fn(&str, &str) -> bool,
    wired: &mut Wired,
) {
    let mut names: HashMap<String, String> = HashMap::new();

    for class in classes.iter().filter(|class| !class.external && is(&class.id, "command")) {
        let from_attribute = |attribute_class: &str, argument: &str| {
            let attribute = attributes.iter().find(|(_, attribute)| attribute.class.eq_ignore_ascii_case(&class.id) && attribute.attribute.eq_ignore_ascii_case(attribute_class));

            attribute.and_then(|(_, attribute)| {
                attribute
                    .arguments
                    .iter()
                    .find(|(name, _)| name.as_deref() == Some(argument))
                    .or_else(|| attribute.arguments.iter().find(|(name, _)| name.is_none()))
                    .and_then(|(_, values)| values.first())
                    .and_then(|value| value.split_whitespace().next().map(str::to_string))
            })
        };

        let property = |name: &str| {
            properties("command", name).into_iter().find(|(_, property)| property.class.eq_ignore_ascii_case(&class.id)).and_then(|(_, property)| match &property.value {
                Value::Text(text) => text.split_whitespace().next().map(str::to_string),
                _ => None,
            })
        };

        let name = from_attribute(SIGNATURE, "signature")
            .or_else(|| property("signature"))
            .or_else(|| property("name"))
            .or_else(|| from_attribute(AS_COMMAND, "name"));

        if let Some(name) = name {
            names.insert(name, class.id.clone());
        }
    }

    for registration in registrations {
        let Wire::Schedule { task, target, class } = &registration.registered.wire else {
            continue;
        };

        let from = registration.registered.class.clone().unwrap_or_else(|| registration.file.to_string());

        let (target, class) = match (*task, class) {
            ("command", Some(class)) => {
                let name = names.iter().find(|(_, command)| command.eq_ignore_ascii_case(class)).map(|(name, _)| name.clone());

                (name.unwrap_or_else(|| class.clone()), Some(class.clone()))
            }
            ("command", None) => (target.clone(), names.get(target).cloned()),
            _ => (target.clone(), class.clone()),
        };

        let class = class.map(|class| index.class(&class).map_or(class, |known| known.id.clone()));

        wired.laravel.schedule.push(ScheduleFact { from, task, target, class, file: registration.file.to_string(), line: registration.registered.line });
    }
}

fn middleware<'p>(framework: &Framework, registrations: &[Registration<'_>], properties: &Properties<'p>, wired: &mut Wired) {
    let mut slots: Vec<MiddlewareFact> = Vec::new();

    let mut apply = |from: &str, slot: &str, add: bool, class: &str, file: &str, line: u32| {
        if add {
            if !slots.iter().any(|known| known.slot == slot && known.class.eq_ignore_ascii_case(class)) {
                slots.push(MiddlewareFact { from: from.to_string(), slot: slot.to_string(), class: class.to_string(), file: file.to_string(), line });
            }
        } else {
            slots.retain(|known| !(known.slot == slot && known.class.eq_ignore_ascii_case(class)));
        }
    };

    for registration in registrations {
        if let Wire::Middleware { slot, add, classes } = &registration.registered.wire {
            let from = registration.registered.class.clone().unwrap_or_else(|| registration.file.to_string());

            for class in classes {
                apply(&from, slot, *add, class, registration.file, registration.registered.line);
            }
        }
    }

    for (name, prefix) in [("middleware", None), ("middlewareGroups", Some("group")), ("middlewareAliases", Some("alias")), ("routeMiddleware", Some("alias"))] {
        for (file, property) in properties("http", name) {
            match (&property.value, prefix) {
                (Value::List(classes), None) => {
                    for class in classes {
                        apply(&property.class, "global", true, class, &file, property.line);
                    }
                }
                (Value::Map(pairs), Some(prefix)) => {
                    for (key, classes) in pairs {
                        for class in classes {
                            apply(&property.class, &format!("{prefix}:{key}"), true, class, &file, property.line);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut aliases: HashMap<String, String> = framework.aliases.clone();

    for fact in &slots {
        if let Some(alias) = fact.slot.strip_prefix("alias:") {
            aliases.insert(alias.to_string(), fact.class.clone());
        }
    }

    for route in &wired.laravel.routes {
        for middleware in &route.middleware {
            let name = middleware.split(':').next().unwrap_or_default();
            let class = if name.contains('\\') { Some(name.to_string()) } else { aliases.get(name).cloned() };

            if let Some(class) = class {
                wired.edges.push(RawEdge {
                    from: route.from.clone(),
                    to: class,
                    kind: EdgeKind::Middleware,
                    label: Some(format!("route:{name}")),
                    file: route.file.clone(),
                    line: route.line,
                    declared_at: 0,
                });
            }
        }
    }

    wired.laravel.middleware = slots;
}

fn edges_and_files(project: &Project, wired: &mut Wired) {
    let laravel = &mut wired.laravel;
    let mut edge = |from: &str, to: &str, kind: EdgeKind, label: Option<String>, file: &str, line: u32| {
        if !from.is_empty() && !to.is_empty() && !from.eq_ignore_ascii_case(to) {
            wired.edges.push(RawEdge { from: from.to_string(), to: to.to_string(), kind, label, file: file.to_string(), line, declared_at: 0 });
        }
    };

    for route in &laravel.routes {
        if let Some(target) = &route.target {
            edge(&route.from, target, EdgeKind::Route, Some(route_label(route)), &route.file, route.line);
        }
    }

    for listener in &laravel.listeners {
        if let Some(class) = &listener.class {
            if looks_like_class(&listener.event) {
                let method = listener.listener.split_once('@').map_or_else(|| "closure".to_string(), |(_, method)| format!("{method}()"));
                edge(&listener.event, class, EdgeKind::Listener, Some(method), &listener.file, listener.line);
            }
        }
    }

    for observer in &laravel.observers {
        edge(&observer.model, &observer.observer, EdgeKind::Observer, Some(observer.events.join(", ")), &observer.file, observer.line);
    }

    for policy in &laravel.policies {
        edge(&policy.model, &policy.policy, EdgeKind::Policy, None, &policy.file, policy.line);
    }

    for binding in &laravel.bindings {
        if let Some(concrete) = &binding.concrete {
            if looks_like_class(&binding.key) {
                edge(&binding.key, concrete, EdgeKind::Binding, Some(binding.lifetime.to_string()), &binding.file, binding.line);
            }
        }
    }

    for task in &laravel.schedule {
        if let Some(class) = &task.class {
            edge(&task.from, class, EdgeKind::Schedule, Some(format!("{} {}", task.task, task.target)), &task.file, task.line);
        }
    }

    for fact in &laravel.middleware {
        edge(&fact.from, &fact.class, EdgeKind::Middleware, Some(fact.slot.clone()), &fact.file, fact.line);
    }

    laravel.routes.sort_by(|a, b| (&a.file, a.line, &a.methods, &a.uri).cmp(&(&b.file, b.line, &b.methods, &b.uri)));
    laravel.listeners.sort();
    laravel.observers.sort();
    laravel.policies.sort();
    laravel.bindings.sort();
    laravel.schedule.sort();
    laravel.middleware.sort();

    let files: BTreeSet<&str> = laravel
        .routes
        .iter()
        .map(|route| route.from.as_str())
        .chain(laravel.schedule.iter().map(|task| task.from.as_str()))
        .chain(laravel.middleware.iter().map(|fact| fact.from.as_str()))
        .filter(|from| from.ends_with(".php"))
        .collect();

    for path in files {
        let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
        let mut node = ClassNode::external(path, &project.name);

        node.name = name.to_string();
        node.namespace = directory.replace('/', "\\");
        node.kind = "file";
        node.external = false;
        node.layer = "entry";
        node.file = Some(path.to_string());
        node.line = Some(1);

        wired.files.push(node);
    }

    wired.unresolved.sort_by(|a, b| (&a.file, a.line, &a.detail).cmp(&(&b.file, b.line, &b.detail)));
    wired.unresolved.dedup_by(|a, b| a.file == b.file && a.line == b.line && a.detail == b.detail);
}

pub fn route_label(route: &RouteFact) -> String {
    let methods: Vec<&str> = route.methods.iter().map(String::as_str).filter(|method| *method != "HEAD" || route.methods.len() == 1).collect();

    format!("{} {}", methods.join("|"), route.uri)
}

fn looks_like_class(name: &str) -> bool {
    !name.is_empty() && !name.contains(' ') && !name.contains(':') && !name.contains('.') && !name.contains('*')
}

/// `DiscoverEvents::classFromFile`: a listener is discovered only when its file's path names its class.
fn named_by_path(class: &ClassNode, file: &str) -> bool {
    let Some(rest) = file.strip_prefix("app/").and_then(|rest| rest.strip_suffix(".php")) else {
        return false;
    };

    let expected = format!("App\\{}", rest.replace('/', "\\"));

    class.id.eq_ignore_ascii_case(&expected)
}

/// The directories a discovery path such as `app/Domain/*/Listeners` names.
fn glob_directories(root: &Path, pattern: &str) -> Vec<String> {
    let mut found = vec![String::new()];

    for part in pattern.split('/').filter(|part| !part.is_empty()) {
        let mut next = Vec::new();

        for base in &found {
            let directory = if base.is_empty() { root.to_path_buf() } else { root.join(base) };

            if part.contains('*') {
                let (prefix, suffix) = part.split_once('*').unwrap_or((part, ""));

                for entry in std::fs::read_dir(&directory).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();

                    if entry.path().is_dir() && name.starts_with(prefix) && name.ends_with(suffix) {
                        next.push(if base.is_empty() { name } else { format!("{base}/{name}") });
                    }
                }
            } else if directory.join(part).is_dir() {
                next.push(if base.is_empty() { part.to_string() } else { format!("{base}/{part}") });
            }
        }

        found = next;
    }

    found.sort();
    found
}

/// A static property's array, or the first array a method of a vendor class returns.
fn vendor_strings(project: &Project, class: &str, member: &str) -> Option<Vec<String>> {
    let path = project.vendor.file_for(class)?;
    let contents = std::fs::read(&path).ok()?;
    let relative = project.relative(&path);
    let file = File::new(std::borrow::Cow::Owned(relative.as_bytes().to_vec()), FileType::Vendored, None, std::borrow::Cow::Owned(contents));
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    let names = NameResolver::new(&arena).resolve(program);
    let source = Source { file: &file, relative: &relative, names: &names };
    let mut context = MemberArray { source: &source, member, inside: false, found: None };

    MemberArrayWalker.walk_program(program, &mut context);

    context.found
}

struct MemberArray<'s, 'a, 'arena> {
    source: &'s Source<'a, 'arena>,
    member: &'s str,
    inside: bool,
    found: Option<Vec<String>>,
}

struct MemberArrayWalker;

impl<'ast, 'arena> Walker<'ast, 'arena, MemberArray<'_, '_, 'arena>> for MemberArrayWalker {
    fn walk_in_method(&self, method: &'ast PhpMethod<'arena>, context: &mut MemberArray<'_, '_, 'arena>) {
        context.inside = method.name.value.eq_ignore_ascii_case(context.member.as_bytes());
    }

    fn walk_out_method(&self, _: &'ast PhpMethod<'arena>, context: &mut MemberArray<'_, '_, 'arena>) {
        context.inside = false;
    }

    fn walk_in_property_concrete_item(&self, item: &'ast PropertyConcreteItem<'arena>, context: &mut MemberArray<'_, '_, 'arena>) {
        if context.found.is_none() && item.variable.name.strip_prefix(b"$").is_some_and(|name| name == context.member.as_bytes()) {
            context.found = context.source.strings(item.value);
        }
    }

    fn walk_in_array(&self, array: &'ast Array<'arena>, context: &mut MemberArray<'_, '_, 'arena>) {
        if context.inside && context.found.is_none() {
            context.found = array.elements.iter().map(|element| match element {
                ArrayElement::Value(value) => context.source.string(value.value),
                _ => None,
            }).collect();
        }
    }
}
