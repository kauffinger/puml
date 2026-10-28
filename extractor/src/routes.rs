//! Routes as Laravel registers them. `RouteReader` reads the route registrations written in a file:
//! `Route::get(...)`, groups, resources and `require`d files, each with the group attributes around it.
//! `compile` then applies those attributes the way `Illuminate\Routing\Router` does, so a route ends up
//! with the methods, URI, name, action and middleware `php artisan route:list` shows. A registration
//! puml cannot read without running code, such as a macro a package adds to the router, is reported as
//! unresolved with its file:line.

use std::collections::HashMap;

use mago_span::HasPosition;
use mago_syntax::cst::*;
use mago_syntax::walker::Walker;

use crate::php::*;

/// The attributes of one `Route::group()`, or of a `Route::prefix(...)->...` chain, as written.
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub prefix: Option<String>,
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub controller: Option<String>,
    pub domain: Option<String>,
    pub middleware: Vec<String>,
    pub excluded: Vec<String>,
}

impl Group {
    /// `RouteGroup::merge($new, $old)` for a group pushed onto the stack.
    fn merged_into(&self, old: &Group) -> Group {
        let prefix = match &self.prefix {
            Some(prefix) => Some(format!("{}/{}", trim(old.prefix.as_deref().unwrap_or(""), '/'), trim(prefix, '/'))),
            None => old.prefix.clone(),
        };

        let name = match &old.name {
            Some(old_name) => Some(format!("{old_name}{}", self.name.as_deref().unwrap_or(""))),
            None => self.name.clone(),
        };

        let namespace = match &self.namespace {
            Some(namespace) => match &old.namespace {
                Some(old_namespace) if !namespace.starts_with('\\') => {
                    Some(format!("{}\\{}", trim(old_namespace, '\\'), trim(namespace, '\\')))
                }
                _ => Some(trim(namespace, '\\').to_string()),
            },
            None => old.namespace.clone(),
        };

        Group {
            prefix,
            name,
            namespace,
            controller: self.controller.clone().or_else(|| old.controller.clone()),
            domain: self.domain.clone().or_else(|| old.domain.clone()),
            middleware: [old.middleware.clone(), self.middleware.clone()].concat(),
            excluded: [old.excluded.clone(), self.excluded.clone()].concat(),
        }
    }
}

/// The group a route is registered in: every group around it merged, outermost first.
pub fn last_group(groups: &[Group]) -> Option<Group> {
    let mut last: Option<Group> = None;

    for group in groups {
        last = Some(match &last {
            Some(old) => group.merged_into(old),
            None => group.clone(),
        });
    }

    last
}

#[derive(Clone, Debug)]
pub enum Action {
    /// A string or `Foo::class`, completed by the group's controller and namespace.
    Uses { target: String, class_constant: bool },
    Controller { class: String, method: String },
    Closure,
    View(String),
    Redirect(String),
    Livewire(String),
}

#[derive(Clone, Debug)]
pub enum Kind {
    Route { methods: Vec<String>, uri: String, action: Action, legacy: Group },
    Resource(Resource),
}

#[derive(Clone, Debug)]
pub struct Resource {
    pub name: String,
    pub controller: String,
    pub api: bool,
    pub only: Option<Vec<String>>,
    pub except: Vec<String>,
    pub names: Option<ResourceNames>,
    pub parameters: Vec<(String, String)>,
    pub resource_as: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ResourceNames {
    All(String),
    Each(Vec<(String, String)>),
}

#[derive(Clone, Debug)]
pub struct Registration {
    pub groups: Vec<Group>,
    pub registrar: Group,
    pub kind: Kind,
    pub names: Vec<String>,
    pub middleware: Vec<String>,
    pub excluded: Vec<String>,
    pub domain: Option<String>,
    pub prefixes: Vec<String>,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum RouteItem {
    Route(Registration),
    /// `->group('routes/admin.php')` or `require __DIR__.'/auth.php'`: the file's routes, in these groups.
    Include { groups: Vec<Group>, path: String, line: u32 },
    Unresolved { detail: String, line: u32 },
}

const ROUTE_METHODS: &[&str] =
    &["get", "post", "put", "patch", "delete", "options", "query", "any", "match", "view", "redirect", "permanentredirect", "fallback", "livewire"];

/// Router methods that register no route.
const NOT_ROUTES: &[&str] = &[
    "bind", "model", "pattern", "patterns", "macro", "mixin", "aliasmiddleware", "middlewaregroup", "pushmiddlewaretogroup",
    "prependmiddlewaretogroup", "removemiddlewarefromgroup", "middlewarepriority", "resourceparameters", "resourceverbs",
    "singularresourceparameters", "substitutebindings", "substituteimplicitbindings", "has", "is", "current", "currentroutename",
    "currentrouteaction", "currentroutenamed", "currentrouteuses", "getroutes", "setroutes", "getcurrentroute", "getcurrentrequest",
    "getmiddleware", "getmiddlewaregroups", "getpatterns", "hasmiddlewaregroup", "input", "uses", "dispatch", "dispatchtoroute",
    "respondwithroute", "torroute", "torresponse", "gatherroutemiddleware", "getbindingcallback", "flushmiddlewaregroups",
    "matched", "removemiddleware", "getlastgroupprefix", "hasgroupstack", "getgroupstack", "mergewithlastgroup", "singletonparameters",
];

/// Static calls in route files that are known to register no route.
const NOT_ROUTE_CALLS: &[&str] = &["Broadcast::channel", "Artisan::command", "Schedule::", "Gate::", "Event::", "RateLimiter::"];

/// Static calls that register routes in code puml does not read, wherever they are written. Of the
/// framework's facades, only these and `Route` reach the router.
const ROUTE_REGISTERING_CALLS: &[&str] = &["Broadcast::routes", "Broadcast::userRoutes", "Broadcast::channelRoutes", "Auth::routes"];

const FACADES: &str = "Illuminate\\Support\\Facades\\";

pub struct RouteReader<'s, 'a, 'arena> {
    source: &'s Source<'a, 'arena>,
    pub items: Vec<RouteItem>,
    /// In a route file every call may register routes, so calls puml does not know are reported. In the
    /// methods of a class they are not, as most of them have nothing to do with routes.
    pub route_file: bool,
    /// The string properties of the class whose method is read, for `->namespace($this->namespace)`.
    pub properties: HashMap<String, String>,
    paths: HashMap<String, String>,
}

impl<'s, 'a, 'arena> RouteReader<'s, 'a, 'arena> {
    pub fn new(source: &'s Source<'a, 'arena>) -> Self {
        RouteReader { source, items: Vec::new(), route_file: true, properties: HashMap::new(), paths: HashMap::new() }
    }

    fn nested(&self) -> Self {
        RouteReader { source: self.source, items: Vec::new(), route_file: self.route_file, properties: self.properties.clone(), paths: self.paths.clone() }
    }

    /// Reads routes that Laravel registers only under a condition puml cannot evaluate, and reports each
    /// of them as unresolved on the line it is registered on.
    fn flagged(&mut self, detail: &str, read: impl FnOnce(&mut Self)) {
        let mut reader = self.nested();
        read(&mut reader);

        for item in reader.items {
            self.items.push(match item {
                RouteItem::Route(registration) => RouteItem::Unresolved { detail: detail.to_string(), line: registration.line },
                RouteItem::Include { path, line, .. } => RouteItem::Unresolved { detail: format!("{detail}; it loads {path}"), line },
                unresolved => unresolved,
            });
        }
    }

    /// The expressions a statement or callback runs, without descending into the ones they contain.
    fn run_expressions<'ast>(&mut self, groups: &[Group], walk: impl FnOnce(&Finder, &mut Found<'ast, 'arena>))
    where
        'arena: 'ast,
    {
        let mut found = Found { depth: 0, expressions: Vec::new() };
        walk(&Finder, &mut found);

        for expression in found.expressions {
            self.expression(expression, groups);
        }
    }

    pub fn statements<'ast>(&mut self, statements: impl IntoIterator<Item = &'ast Statement<'arena>>, groups: &[Group])
    where
        'arena: 'ast,
    {
        for statement in statements {
            self.statement(statement, groups);
        }
    }

    fn statement(&mut self, statement: &Statement<'arena>, groups: &[Group]) {
        match statement {
            Statement::Expression(expression) => {
                if let Expression::Assignment(assignment) = expression.expression {
                    if let (Expression::Variable(Variable::Direct(variable)), Some(path)) = (assignment.lhs, self.source.path(assignment.rhs)) {
                        self.paths.insert(String::from_utf8_lossy(variable.name).into_owned(), path);
                    }
                }

                self.expression(expression.expression, groups)
            }
            Statement::Namespace(namespace) => match &namespace.body {
                NamespaceBody::Implicit(body) => self.statements(body.statements.iter(), groups),
                NamespaceBody::BraceDelimited(block) => self.statements(block.statements.iter(), groups),
            },
            Statement::Block(block) => self.statements(block.statements.iter(), groups),
            Statement::Try(r#try) => self.statements(r#try.block.statements.iter(), groups),
            Statement::Return(r#return) => {
                if let Some(value) = r#return.value {
                    self.expression(value, groups);
                }
            }
            Statement::If(_) | Statement::Switch(_) => {
                let detail = format!("registered only when the condition on line {} holds", self.source.line(statement.offset()));

                self.flagged(&detail, |reader| reader.run_expressions(groups, |finder, found| finder.walk_statement(statement, found)));
            }
            Statement::Foreach(_) | Statement::For(_) | Statement::While(_) | Statement::DoWhile(_) => {
                let detail = format!("registered in the loop on line {}, so how often and with which values is only known at runtime", self.source.line(statement.offset()));

                self.flagged(&detail, |reader| reader.run_expressions(groups, |finder, found| finder.walk_statement(statement, found)));
            }
            _ => {}
        }
    }

    pub fn expression(&mut self, expression: &Expression<'arena>, groups: &[Group]) {
        match expression {
            Expression::Construct(construct) => {
                let value = match construct {
                    Construct::Require(require) => require.value,
                    Construct::RequireOnce(require) => require.value,
                    Construct::Include(include) => include.value,
                    Construct::IncludeOnce(include) => include.value,
                    _ => return,
                };

                self.include(value, groups);
            }
            Expression::Assignment(assignment) => self.expression(assignment.rhs, groups),
            Expression::Call(_) => {
                let read = match chain(self.source, expression) {
                    Some((Base::Static(class), links)) if is_facade(&class, "Route") => {
                        self.chain(&links, groups);
                        true
                    }
                    Some((Base::Static(class), links)) => self.other_static_call(&class, &links, expression),
                    Some((Base::This, links)) => match links.first().map(|link| link.method.to_ascii_lowercase()).as_deref() {
                        Some("routes") => {
                            if let Some(routes) = argument(links[0].arguments, 0, "routesCallback") {
                                self.callback(routes, groups);
                            }

                            true
                        }
                        Some("loadroutesfrom") => {
                            if let Some(path) = argument(links[0].arguments, 0, "path") {
                                self.include(path, groups);
                            }

                            true
                        }
                        _ => false,
                    },
                    _ => false,
                };

                if !read && self.source.text(expression).contains("Route::") {
                    let detail = format!("registered in a callback on line {}, which only runs when the code calling it does", self.source.line(expression.offset()));

                    self.flagged(&detail, |reader| reader.run_expressions(groups, |finder, found| finder.walk_expression(expression, found)));
                }
            }
            _ => {}
        }
    }

    /// A static call on another class than the router, which may register routes in code puml does not read.
    fn other_static_call(&mut self, class: &str, links: &[Link<'_, 'arena>], expression: &Expression<'arena>) -> bool {
        let Some(first) = links.first() else {
            return false;
        };

        let short = class.rsplit('\\').next().unwrap_or(class);
        let call = format!("{short}::{}", first.method);

        if ROUTE_REGISTERING_CALLS.iter().any(|known| call.eq_ignore_ascii_case(known)) {
            self.unresolved(&format!("{call}() registers routes in code puml does not read"), expression.offset());

            return true;
        }

        let framework_facade = class.len() > FACADES.len() && class[..FACADES.len()].eq_ignore_ascii_case(FACADES);

        if self.route_file && !framework_facade && !NOT_ROUTE_CALLS.iter().any(|known| call.to_ascii_lowercase().starts_with(&known.to_ascii_lowercase())) {
            self.unresolved(&format!("{call}() is not a router method; the routes it registers, if any, are only known at runtime"), expression.offset());

            return true;
        }

        false
    }

    fn include(&mut self, path: &Expression<'arena>, groups: &[Group]) {
        let line = self.source.line(path.offset());
        let variable = match path {
            Expression::Variable(Variable::Direct(variable)) => self.paths.get(&*String::from_utf8_lossy(variable.name)).cloned(),
            _ => None,
        };

        match variable.or_else(|| self.source.path(path)) {
            Some(path) => self.items.push(RouteItem::Include { groups: groups.to_vec(), path, line }),
            None => self.unresolved("a route file loaded from a path that is only known at runtime", path.offset()),
        }
    }

    /// The routes of a group: a closure, a route file, or a list of them.
    pub fn callback(&mut self, routes: &Expression<'arena>, groups: &[Group]) {
        match routes {
            Expression::Closure(closure) => self.statements(closure.body.statements.iter(), groups),
            Expression::ArrowFunction(function) => self.expression(function.expression, groups),
            Expression::Array(_) | Expression::LegacyArray(_) => {
                for element in array_elements(routes).unwrap_or_default() {
                    if let Some(value) = element.value() {
                        self.callback(value, groups);
                    }
                }
            }
            _ => self.include(routes, groups),
        }
    }

    fn chain(&mut self, links: &[Link<'_, 'arena>], groups: &[Group]) {
        let mut registrar = Group::default();
        let mut uncertain: Option<String> = None;

        for (index, link) in links.iter().enumerate() {
            let method = link.method.to_ascii_lowercase();

            if let Some(detail) = &uncertain {
                if method == "group" || method.contains("resource") || ROUTE_METHODS.contains(&method.as_str()) {
                    let detail = detail.clone();
                    let rest = &links[index..];

                    return self.flagged(&detail, |reader| reader.chain_without_uncertainty(rest, groups, registrar));
                }
            }

            let attribute = match method.as_str() {
                "prefix" => self.text_argument(link).map(|prefix| registrar.prefix = Some(prefix)),
                "name" | "as" => self.text_argument(link).map(|name| registrar.name = Some(name)),
                "namespace" => self.namespace_argument(link).map(|namespace| registrar.namespace = namespace),
                "domain" => self.text_argument(link).map(|domain| registrar.domain = Some(domain)),
                "controller" => {
                    argument(link.arguments, 0, "controller").and_then(|value| self.source.class_name(value)).map(|class| registrar.controller = Some(class))
                }
                "middleware" => Some(registrar.middleware = self.middleware_arguments(link)),
                "withoutmiddleware" => Some(registrar.excluded.extend(self.middleware_arguments(link))),
                "where" | "can" | "missing" | "scopebindings" | "withoutscopedbindings" | "metadata" => Some(()),
                "group" => {
                    let positional = positional(link.arguments);

                    if index == 0 && positional.len() >= 2 {
                        let Some(attributes) = self.group_attributes(positional[0]) else {
                            let detail = format!("registered in a group whose attributes on line {} are only known at runtime", self.source.line(link.offset));
                            let routes = positional[1];

                            return self.flagged(&detail, |reader| reader.callback(routes, groups));
                        };

                        self.callback(positional[1], &[groups, &[attributes]].concat());
                    } else if let Some(routes) = argument(link.arguments, 0, "routes") {
                        self.callback(routes, &[groups, &[registrar]].concat());
                    }

                    return;
                }
                "resource" | "apiresource" => return self.resource(&method, link, &links[index + 1..], groups, registrar),
                "resources" | "apiresources" => {
                    let single = if method == "resources" { "resource" } else { "apiresource" };

                    for (name, controller) in argument(link.arguments, 0, "resources").and_then(array_pairs).unwrap_or_default() {
                        match (self.source.string(name), self.source.class_name(controller)) {
                            (Some(name), Some(controller)) => self.push_resource(single, name, controller, link, &[], groups, registrar.clone()),
                            _ => self.unresolved("a resource whose name or controller is only known at runtime", link.offset),
                        }
                    }

                    return;
                }
                _ if ROUTE_METHODS.contains(&method.as_str()) => return self.route(&method, link, &links[index + 1..], groups, registrar),
                _ if index == 0 && NOT_ROUTES.contains(&method.as_str()) => return,
                _ if index == 0 => {
                    return self.unresolved(
                        &format!("Route::{}() is not a router method; a package macro registers its routes at runtime", link.method),
                        link.offset,
                    );
                }
                _ => {
                    return self.unresolved(&format!("->{}() on a route registrar is only known at runtime", link.method), link.offset);
                }
            };

            if attribute.is_none() {
                uncertain = Some(format!("registered in a group whose ->{}() on line {} is only known at runtime", link.method, self.source.line(link.offset)));
            }
        }
    }

    /// The rest of a chain whose registrar attributes are already taken, read as if they were known.
    fn chain_without_uncertainty(&mut self, links: &[Link<'_, 'arena>], groups: &[Group], registrar: Group) {
        let Some(link) = links.first() else {
            return;
        };

        let method = link.method.to_ascii_lowercase();

        match method.as_str() {
            "group" => {
                if let Some(routes) = argument(link.arguments, 0, "routes") {
                    self.callback(routes, &[groups, &[registrar]].concat());
                }
            }
            "resource" | "apiresource" => self.resource(&method, link, &links[1..], groups, registrar),
            _ if ROUTE_METHODS.contains(&method.as_str()) => self.route(&method, link, &links[1..], groups, registrar),
            _ => {}
        }
    }

    fn route(&mut self, method: &str, link: &Link<'_, 'arena>, rest: &[Link<'_, 'arena>], groups: &[Group], registrar: Group) {
        let arguments = link.arguments;

        let (methods, uri, action): (Vec<String>, Option<&Expression<'arena>>, Option<Action>) = match method {
            "any" => (vec!["*".to_string()], argument(arguments, 0, "uri"), argument(arguments, 1, "action").and_then(|action| self.action(action))),
            "match" => {
                let methods = argument(arguments, 0, "methods").and_then(|methods| self.source.strings(methods));

                let Some(methods) = methods else {
                    return self.unresolved("Route::match() with methods that are only known at runtime", link.offset);
                };

                (
                    methods.iter().map(|method| method.to_ascii_uppercase()).collect(),
                    argument(arguments, 1, "uri"),
                    argument(arguments, 2, "action").and_then(|action| self.action(action)),
                )
            }
            "view" => (
                vec!["GET".to_string(), "HEAD".to_string()],
                argument(arguments, 0, "uri"),
                argument(arguments, 1, "view").and_then(|view| self.source.string(view)).map(Action::View),
            ),
            "redirect" | "permanentredirect" => (
                vec!["*".to_string()],
                argument(arguments, 0, "uri"),
                argument(arguments, 1, "destination").and_then(|destination| self.source.string(destination)).map(Action::Redirect),
            ),
            "fallback" => (
                vec!["GET".to_string()],
                None,
                argument(arguments, 0, "action").and_then(|action| self.action(action)),
            ),
            "livewire" => (
                vec!["GET".to_string()],
                argument(arguments, 0, "uri"),
                argument(arguments, 1, "component").and_then(|component| self.source.class_name(component)).map(Action::Livewire),
            ),
            verb => (vec![verb.to_ascii_uppercase()], argument(arguments, 0, "uri"), argument(arguments, 1, "action").and_then(|action| self.action(action))),
        };

        let uri = match (method, uri) {
            ("fallback", _) => Some("{fallbackPlaceholder}".to_string()),
            (_, Some(uri)) => self.source.string(uri),
            (_, None) => None,
        };

        let (Some(uri), Some(action)) = (uri, action) else {
            return self.unresolved(&format!("Route::{}() with a URI or action that is only known at runtime", link.method), link.offset);
        };

        let legacy = match argument(arguments, if method == "match" { 2 } else { 1 }, "action") {
            Some(action) => self.legacy_attributes(action),
            None => Group::default(),
        };

        let mut registration = Registration {
            groups: groups.to_vec(),
            registrar,
            kind: Kind::Route { methods, uri, action, legacy },
            names: Vec::new(),
            middleware: Vec::new(),
            excluded: Vec::new(),
            domain: None,
            prefixes: Vec::new(),
            line: self.source.line(link.offset),
        };

        for modifier in rest {
            let applied = match modifier.method.to_ascii_lowercase().as_str() {
                "name" => self.text_argument(modifier).map(|name| registration.names.push(name)),
                "middleware" => Some(registration.middleware.extend(self.middleware_arguments(modifier))),
                "withoutmiddleware" => Some(registration.excluded.extend(self.middleware_arguments(modifier))),
                "domain" => self.text_argument(modifier).map(|domain| registration.domain = Some(domain)),
                "prefix" => self.text_argument(modifier).map(|prefix| registration.prefixes.push(prefix)),
                "can" => {
                    let ability = argument(modifier.arguments, 0, "ability").and_then(|ability| self.source.string(ability));
                    let models = match argument(modifier.arguments, 1, "models") {
                        Some(models) => self.source.strings(models),
                        None => Some(Vec::new()),
                    };

                    ability.zip(models).map(|(ability, models)| {
                        registration.middleware.push(if models.is_empty() {
                            format!("can:{ability}")
                        } else {
                            format!("can:{ability},{}", models.join(","))
                        })
                    })
                }
                _ => Some(()),
            };

            if applied.is_none() {
                return self.unresolved(&format!("->{}() with a value that is only known at runtime", modifier.method), link.offset);
            }
        }

        self.items.push(RouteItem::Route(registration));
    }

    fn resource(&mut self, method: &str, link: &Link<'_, 'arena>, rest: &[Link<'_, 'arena>], groups: &[Group], registrar: Group) {
        let name = argument(link.arguments, 0, "name").and_then(|name| self.source.string(name));
        let controller = argument(link.arguments, 1, "controller").and_then(|controller| self.source.class_name(controller));

        match (name, controller) {
            (Some(name), Some(controller)) => self.push_resource(method, name, controller, link, rest, groups, registrar),
            _ => self.unresolved("a resource whose name or controller is only known at runtime", link.offset),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push_resource(
        &mut self,
        method: &str,
        name: String,
        controller: String,
        link: &Link<'_, 'arena>,
        rest: &[Link<'_, 'arena>],
        groups: &[Group],
        registrar: Group,
    ) {
        let mut resource = Resource {
            name,
            controller,
            api: method == "apiresource",
            only: None,
            except: Vec::new(),
            names: None,
            parameters: Vec::new(),
            resource_as: registrar.name.clone(),
        };

        let mut registration = Registration {
            groups: groups.to_vec(),
            registrar: Group::default(),
            kind: Kind::Route { methods: Vec::new(), uri: String::new(), action: Action::Closure, legacy: Group::default() },
            names: Vec::new(),
            middleware: registrar.middleware,
            excluded: Vec::new(),
            domain: None,
            prefixes: Vec::new(),
            line: self.source.line(link.offset),
        };

        for modifier in rest {
            let list = || match positional(modifier.arguments).as_slice() {
                [single] => self.source.strings(single),
                many => many.iter().map(|value| self.source.string(value)).collect(),
            };

            let applied = match modifier.method.to_ascii_lowercase().as_str() {
                "only" => list().map(|only| resource.only = Some(only)),
                "except" => list().map(|except| resource.except = except),
                "middleware" => Some(registration.middleware = self.middleware_arguments(modifier)),
                "withoutmiddleware" => Some(registration.excluded.extend(self.middleware_arguments(modifier))),
                "names" => argument(modifier.arguments, 0, "names").and_then(|names| self.resource_names(names)).map(|names| resource.names = Some(names)),
                "name" => {
                    let method = argument(modifier.arguments, 0, "method").and_then(|method| self.source.string(method));
                    let name = argument(modifier.arguments, 1, "name").and_then(|name| self.source.string(name));

                    method.zip(name).map(|pair| match &mut resource.names {
                        Some(ResourceNames::Each(names)) => names.push(pair),
                        _ => resource.names = Some(ResourceNames::Each(vec![pair])),
                    })
                }
                "parameters" => argument(modifier.arguments, 0, "parameters").and_then(|parameters| self.string_pairs(parameters)).map(|pairs| resource.parameters = pairs),
                "parameter" => {
                    let previous = argument(modifier.arguments, 0, "previous").and_then(|value| self.source.string(value));
                    let new = argument(modifier.arguments, 1, "new").and_then(|value| self.source.string(value));

                    previous.zip(new).map(|pair| resource.parameters.push(pair))
                }
                "shallow" | "scoped" | "creatable" | "destroyable" => None,
                _ => Some(()),
            };

            if applied.is_none() {
                return self.unresolved(&format!("->{}() on a resource is not read by puml", modifier.method), link.offset);
            }
        }

        registration.kind = Kind::Resource(resource);
        self.items.push(RouteItem::Route(registration));
    }

    fn resource_names(&self, names: &Expression<'arena>) -> Option<ResourceNames> {
        match self.source.string(names) {
            Some(name) => Some(ResourceNames::All(name)),
            None => self.string_pairs(names).map(ResourceNames::Each),
        }
    }

    fn string_pairs(&self, expression: &Expression<'arena>) -> Option<Vec<(String, String)>> {
        array_pairs(expression)?
            .into_iter()
            .map(|(key, value)| Some((self.source.string(key)?, self.source.string(value)?)))
            .collect()
    }

    fn action(&self, action: &Expression<'arena>) -> Option<Action> {
        match action {
            Expression::Closure(_) | Expression::ArrowFunction(_) => Some(Action::Closure),
            Expression::Array(_) | Expression::LegacyArray(_) => {
                let elements = array_elements(action)?;

                if elements.iter().all(|element| matches!(element, ArrayElement::Value(_))) {
                    let [class, method] = elements.as_slice() else {
                        return None;
                    };

                    return Some(Action::Controller {
                        class: self.source.class_name(class.value()?)?,
                        method: self.source.string(method.value()?)?,
                    });
                }

                let uses = array_pairs(action)?.into_iter().find(|(key, _)| self.source.string(key).as_deref() == Some("uses"))?.1;

                self.action(uses)
            }
            _ => match self.source.class_constant(action) {
                Some(class) => Some(Action::Uses { target: class, class_constant: true }),
                None => self.source.string(action).map(|target| Action::Uses { target, class_constant: false }),
            },
        }
    }

    /// `['as' => …, 'middleware' => …, 'uses' => …]`, the action array of Laravel before 5.
    fn legacy_attributes(&self, action: &Expression<'arena>) -> Group {
        let mut group = Group::default();

        for (key, value) in array_pairs(action).unwrap_or_default() {
            match self.source.string(key).as_deref() {
                Some("as") => group.name = self.source.string(value),
                Some("middleware") => group.middleware = self.middleware_values(value),
                Some("prefix") => group.prefix = self.source.string(value),
                Some("domain") => group.domain = self.source.string(value),
                _ => {}
            }
        }

        group
    }

    fn group_attributes(&self, attributes: &Expression<'arena>) -> Option<Group> {
        let mut group = Group::default();

        for (key, value) in array_pairs(attributes)? {
            let key = self.source.string(key)?;

            match key.as_str() {
                "prefix" => group.prefix = Some(self.source.string(value)?),
                "as" => group.name = Some(self.source.string(value)?),
                "namespace" => group.namespace = Some(self.source.string(value)?),
                "domain" => group.domain = Some(self.source.string(value)?),
                "controller" => group.controller = Some(self.source.class_name(value)?),
                "middleware" => group.middleware = self.middleware_values(value),
                "excluded_middleware" | "withoutMiddleware" => group.excluded = self.middleware_values(value),
                _ => {}
            }
        }

        Some(group)
    }

    fn text_argument(&self, link: &Link<'_, 'arena>) -> Option<String> {
        positional(link.arguments).first().and_then(|value| self.source.string(value))
    }

    /// `->namespace('App\\Http')`, or `->namespace($this->namespace)` in a RouteServiceProvider, whose
    /// parent declares the property as null.
    fn namespace_argument(&self, link: &Link<'_, 'arena>) -> Option<Option<String>> {
        let value = *positional(link.arguments).first()?;

        if let Expression::Access(Access::Property(access)) = value {
            if let (Expression::Variable(Variable::Direct(variable)), Some(name)) = (access.object, method_name(&access.property)) {
                if variable.name == b"$this" {
                    return Some(self.properties.get(&name).cloned());
                }
            }
        }

        match value {
            Expression::Literal(Literal::Null(_)) => Some(None),
            _ => self.source.string(value).map(Some),
        }
    }

    /// `->middleware(['a', 'b'])` or `->middleware('a', 'b')`.
    fn middleware_arguments(&self, link: &Link<'_, 'arena>) -> Vec<String> {
        positional(link.arguments).iter().flat_map(|value| self.middleware_values(value)).collect()
    }

    /// Middleware as written. One that is only known at runtime, such as `config('jetstream.auth_session')`,
    /// is kept as `runtime:` and its source, so the routes are still read and the gap is visible.
    fn middleware_values(&self, value: &Expression<'arena>) -> Vec<String> {
        let one = |value: &Expression<'arena>| self.source.class_name(value).unwrap_or_else(|| format!("runtime:{}", self.source.text(value)));

        match array_elements(value) {
            Some(elements) => elements.iter().filter_map(|element| element.value()).map(one).collect(),
            None => vec![one(value)],
        }
    }

    fn unresolved(&mut self, detail: &str, offset: u32) {
        let line = self.source.line(offset);

        self.items.push(RouteItem::Unresolved { detail: detail.to_string(), line });
    }
}

/// One route, as `route:list` shows it.
#[derive(Clone, Debug)]
pub struct CompiledRoute {
    pub methods: Vec<String>,
    pub uri: String,
    pub domain: Option<String>,
    pub name: Option<String>,
    /// `Class@method`, `view:…`, `redirect:…`, `livewire:…` or `closure`.
    pub action: String,
    /// The class the route dispatches to: the controller, or the Livewire component.
    pub target: Option<String>,
    pub middleware: Vec<String>,
    pub excluded_middleware: Vec<String>,
    pub line: u32,
}

/// The routes of a registration, created in `outer` and the groups it is written in.
pub fn compile(registration: &Registration, outer: &[Group], verbs: &[String]) -> Vec<CompiledRoute> {
    let groups = [outer, &registration.groups].concat();

    match &registration.kind {
        Kind::Route { methods, uri, action, legacy } => {
            vec![create_route(registration, &groups, methods, uri, action.clone(), legacy, verbs)]
        }
        Kind::Resource(resource) => compile_resource(registration, resource, &groups, verbs),
    }
}

fn create_route(
    registration: &Registration,
    groups: &[Group],
    methods: &[String],
    uri: &str,
    action: Action,
    legacy: &Group,
    verbs: &[String],
) -> CompiledRoute {
    let last = last_group(groups);
    let registrar = &registration.registrar;

    let (action, target) = match action {
        Action::Controller { class, method } => (format!("{class}@{method}"), Some(class)),
        Action::Uses { mut target, class_constant } => {
            if let Some(group) = &last {
                if let Some(controller) = &group.controller {
                    if !class_constant && !target.contains('@') && !target.contains('\\') {
                        target = format!("{controller}@{target}");
                    }
                }

                if let Some(namespace) = &group.namespace {
                    if !target.starts_with('\\') && !target.starts_with(namespace.as_str()) {
                        target = format!("{namespace}\\{target}");
                    }
                }
            }

            let target = target.trim_start_matches('\\').to_string();
            let uses = if target.contains('@') { target } else { format!("{target}@__invoke") };
            let class = uses.split('@').next().map(str::to_string);

            (uses, class)
        }
        Action::Closure => ("closure".to_string(), None),
        Action::View(view) => (format!("view:{view}"), None),
        Action::Redirect(destination) => (format!("redirect:{destination}"), None),
        Action::Livewire(component) => {
            let class = component.contains('\\').then(|| component.clone());

            (format!("livewire:{component}"), class)
        }
    };

    let group_prefix = last.as_ref().and_then(|group| group.prefix.clone()).unwrap_or_default();
    let routed = trim(&format!("{}/{}", trim(&group_prefix, '/'), trim(uri, '/')), '/').to_string();
    let mut uri = if routed.is_empty() { "/".to_string() } else { routed };

    for prefix in registrar.prefix.iter().chain(legacy.prefix.iter()).chain(registration.prefixes.iter()) {
        uri = route_prefix(prefix, &uri);
    }

    let uri = without_binding_fields(&uri);

    let own_name = registrar.name.clone().or_else(|| legacy.name.clone());
    let mut name = match last.as_ref().and_then(|group| group.name.clone()) {
        Some(group_name) => Some(format!("{group_name}{}", own_name.unwrap_or_default())),
        None => own_name,
    };

    for appended in &registration.names {
        name = Some(format!("{}{appended}", name.unwrap_or_default()));
    }

    let mut methods: Vec<String> = methods.iter().flat_map(|method| if method == "*" { verbs.to_vec() } else { vec![method.clone()] }).collect();

    if methods.iter().any(|method| method == "GET") && !methods.iter().any(|method| method == "HEAD") {
        methods.push("HEAD".to_string());
    }

    let group_middleware = last.as_ref().map(|group| group.middleware.clone()).unwrap_or_default();
    let group_excluded = last.as_ref().map(|group| group.excluded.clone()).unwrap_or_default();

    CompiledRoute {
        methods,
        uri,
        domain: registration
            .domain
            .clone()
            .or_else(|| registrar.domain.clone())
            .or_else(|| legacy.domain.clone())
            .or_else(|| last.as_ref().and_then(|group| group.domain.clone())),
        name,
        action,
        target,
        middleware: [group_middleware, registrar.middleware.clone(), legacy.middleware.clone(), registration.middleware.clone()].concat(),
        excluded_middleware: [group_excluded, registrar.excluded.clone(), registration.excluded.clone()].concat(),
        line: registration.line,
    }
}

/// `Route::prefix()`, which the route constructor applies with the registrar's prefix.
fn route_prefix(prefix: &str, uri: &str) -> String {
    let joined = format!("{}/{}", prefix.trim_end_matches('/'), uri.trim_start_matches('/'));

    if joined == "/" { joined } else { trim(&joined, '/').to_string() }
}

/// `ResourceRegistrar::register`.
fn compile_resource(registration: &Registration, resource: &Resource, groups: &[Group], verbs: &[String]) -> Vec<CompiledRoute> {
    if let Some((prefix, name)) = resource.name.rsplit_once('/') {
        let inner = Resource { name: name.to_string(), ..resource.clone() };
        let groups = [groups, &[Group { prefix: Some(prefix.to_string()), ..Group::default() }]].concat();

        return compile_resource(registration, &inner, &groups, verbs);
    }

    let wildcard = |segment: &str| {
        let value = match resource.parameters.iter().find(|(name, _)| name == segment) {
            Some((_, parameter)) => parameter.clone(),
            None => singular(segment),
        };

        value.replace('-', "_")
    };

    let segments: Vec<&str> = resource.name.split('.').collect();
    let base = wildcard(segments.last().copied().unwrap_or_default());
    let resource_uri = if segments.len() == 1 {
        resource.name.clone()
    } else {
        let nested = segments.iter().map(|segment| format!("{segment}/{{{}}}", wildcard(segment))).collect::<Vec<_>>().join("/");

        nested.replace(&format!("/{{{base}}}"), "")
    };

    let defaults = ["index", "create", "store", "show", "edit", "update", "destroy"];
    let api_only = ["index", "show", "store", "update", "destroy"];

    let only: Option<Vec<String>> = match (&resource.only, resource.api) {
        (Some(only), _) => Some(only.clone()),
        (None, true) => Some(api_only.iter().filter(|method| !resource.except.iter().any(|except| except == *method)).map(|method| method.to_string()).collect()),
        (None, false) => None,
    };

    defaults
        .iter()
        .filter(|method| only.as_ref().is_none_or(|only| only.iter().any(|allowed| allowed == *method)))
        .filter(|method| !resource.except.iter().any(|except| except == *method))
        .map(|method| {
            let (methods, uri) = match *method {
                "index" => (vec!["GET"], resource_uri.clone()),
                "create" => (vec!["GET"], format!("{resource_uri}/create")),
                "store" => (vec!["POST"], resource_uri.clone()),
                "show" => (vec!["GET"], format!("{resource_uri}/{{{base}}}")),
                "edit" => (vec!["GET"], format!("{resource_uri}/{{{base}}}/edit")),
                "update" => (vec!["PUT", "PATCH"], format!("{resource_uri}/{{{base}}}")),
                _ => (vec!["DELETE"], format!("{resource_uri}/{{{base}}}")),
            };

            let name = match &resource.names {
                Some(ResourceNames::Each(names)) if names.iter().any(|(each, _)| each == method) => {
                    names.iter().find(|(each, _)| each == method).map(|(_, name)| name.clone()).unwrap_or_default()
                }
                names => {
                    let base = match names {
                        Some(ResourceNames::All(name)) => name.clone(),
                        _ => resource.name.clone(),
                    };
                    let prefix = resource.resource_as.as_ref().map(|prefix| format!("{prefix}.")).unwrap_or_default();

                    trim(&format!("{prefix}{base}.{method}"), '.').to_string()
                }
            };

            let legacy = Group { name: Some(name), middleware: registration.middleware.clone(), ..Group::default() };
            let action = Action::Uses { target: format!("{}@{method}", resource.controller), class_constant: false };
            let single = Registration {
                registrar: registration.registrar.clone(),
                middleware: Vec::new(),
                ..registration.clone()
            };

            create_route(&single, groups, &methods.iter().map(|method| method.to_string()).collect::<Vec<_>>(), &uri, action, &legacy, verbs)
        })
        .collect()
}

/// `Str::singular` for the resource names written in route files.
fn singular(word: &str) -> String {
    const UNCOUNTABLE: &[&str] = &["data", "equipment", "information", "media", "metadata", "news", "series", "species", "feedback", "staff"];
    const IRREGULAR: &[(&str, &str)] =
        &[("people", "person"), ("men", "man"), ("women", "woman"), ("children", "child"), ("mice", "mouse"), ("geese", "goose"), ("teeth", "tooth"), ("feet", "foot")];

    let lower = word.to_ascii_lowercase();

    if UNCOUNTABLE.contains(&lower.as_str()) {
        return word.to_string();
    }

    if let Some((plural, single)) = IRREGULAR.iter().find(|(plural, _)| lower.ends_with(plural)) {
        return format!("{}{single}", &word[..word.len() - plural.len()]);
    }

    for (plural, single) in [("ies", "y"), ("sses", "ss"), ("shes", "sh"), ("ches", "ch"), ("xes", "x"), ("zzes", "zz"), ("oes", "o")] {
        if let Some(stem) = word.strip_suffix(plural) {
            return format!("{stem}{single}");
        }
    }

    if word.ends_with("ss") || word.ends_with("us") || word.ends_with("is") {
        return word.to_string();
    }

    word.strip_suffix('s').unwrap_or(word).to_string()
}

/// `RouteUri::parse`: `{post:slug}` binds by the slug column and is stored as `{post}`.
fn without_binding_fields(uri: &str) -> String {
    let mut result = String::with_capacity(uri.len());
    let mut in_field = false;

    for character in uri.chars() {
        match character {
            ':' if result.rfind('{') > result.rfind('}') => in_field = true,
            '}' => {
                in_field = false;
                result.push(character);
            }
            '?' => result.push(character),
            _ if in_field => {}
            _ => result.push(character),
        }
    }

    result
}

fn trim(value: &str, character: char) -> &str {
    value.trim_matches(character)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_prefixes_and_names_join_like_the_router() {
        let groups = [
            Group { prefix: Some("admin".to_string()), name: Some("admin.".to_string()), middleware: vec!["web".to_string()], ..Group::default() },
            Group { prefix: Some("/users/".to_string()), name: Some("users.".to_string()), middleware: vec!["auth".to_string()], ..Group::default() },
        ];

        let last = last_group(&groups).unwrap();

        assert_eq!(last.prefix.as_deref(), Some("admin/users"));
        assert_eq!(last.name.as_deref(), Some("admin.users."));
        assert_eq!(last.middleware, ["web", "auth"]);
    }

    const VERBS: [&str; 8] = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "QUERY"];

    /// The routes a route file registers, as `METHODS uri name action`, and its notes, as `line: detail`.
    fn read(source: &str) -> (Vec<String>, Vec<String>) {
        let contents = format!("<?php\n\nuse Illuminate\\Support\\Facades\\Route;\nuse App\\Http\\Controllers\\PhotoController;\n\n{source}");

        let verbs: Vec<String> = VERBS.map(str::to_string).to_vec();
        let items = crate::wiring::route_items("routes/web.php", contents.into_bytes()).unwrap();

        let mut routes = Vec::new();
        let mut notes = Vec::new();

        for item in items {
            match item {
                RouteItem::Route(registration) => {
                    for route in compile(&registration, &[], &verbs) {
                        let name = route.name.unwrap_or_default();
                        let middleware = route.middleware.join(",");

                        routes.push(format!("{} {} {name} {} [{middleware}] :{}", route.methods.join("|"), route.uri, route.action, route.line).replace("  ", " "));
                    }
                }
                RouteItem::Include { path, line, .. } => notes.push(format!("{line}: include {path}")),
                RouteItem::Unresolved { detail, line } => notes.push(format!("{line}: {detail}")),
            }
        }

        (routes, notes)
    }

    #[test]
    fn a_resource_registers_the_actions_it_keeps() {
        let (routes, notes) = read("Route::resource('photos', PhotoController::class)->only(['index', 'show']);\n");

        assert_eq!(
            routes,
            [
                "GET|HEAD photos photos.index App\\Http\\Controllers\\PhotoController@index [] :6",
                "GET|HEAD photos/{photo} photos.show App\\Http\\Controllers\\PhotoController@show [] :6",
            ]
        );
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn array_and_fluent_groups_nest() {
        let (routes, _) = read(
            "Route::group(['prefix' => 'admin', 'as' => 'admin.', 'middleware' => ['auth']], function () {\n    \
             Route::middleware('can:manage')->name('photos.')->prefix('photos')->group(function () {\n        \
             Route::match(['get', 'post'], '{photo:slug}/crop', [PhotoController::class, 'crop'])->name('crop');\n    \
             });\n\
             });\n",
        );

        assert_eq!(routes, ["GET|POST|HEAD admin/photos/{photo}/crop admin.photos.crop App\\Http\\Controllers\\PhotoController@crop [auth,can:manage] :8"]);
    }

    #[test]
    fn routes_under_a_condition_or_in_a_loop_are_noted_on_their_own_line() {
        let (routes, notes) = read(
            "if (app()->isLocal()) {\n    Route::get('debug', fn () => 'debug');\n}\n\n\
             foreach (['a', 'b'] as $page) {\n    Route::view('page', 'page');\n}\n\n\
             Route::get('/', fn () => 'home');\n",
        );

        assert_eq!(routes, ["GET|HEAD / closure [] :14"]);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].starts_with("7: ") && notes[0].contains("line 6"), "{notes:?}");
        assert!(notes[1].starts_with("11: ") && notes[1].contains("line 10"), "{notes:?}");
    }

    #[test]
    fn a_route_macro_is_noted_not_guessed() {
        let (routes, notes) = read("Route::feeds();\nRoute::get('about', fn () => 'about');\n");

        assert_eq!(routes, ["GET|HEAD about closure [] :7"]);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("6: "), "{notes:?}");
    }

    #[test]
    fn a_framework_facade_other_than_route_registers_no_routes() {
        let (routes, notes) = read(
            "Illuminate\\Support\\Facades\\Blade::directive('title', fn ($expression) => $expression);\n\
             Illuminate\\Support\\Facades\\Broadcast::userRoutes();\n",
        );

        assert!(routes.is_empty(), "{routes:?}");
        assert_eq!(notes, ["7: Broadcast::userRoutes() registers routes in code puml does not read"]);
    }

    #[test]
    fn resource_wildcards_are_singular() {
        assert_eq!(singular("photos"), "photo");
        assert_eq!(singular("categories"), "category");
        assert_eq!(singular("addresses"), "address");
        assert_eq!(singular("people"), "person");
        assert_eq!(singular("news"), "news");
    }
}

pub struct Found<'ast, 'arena> {
    depth: usize,
    expressions: Vec<&'ast Expression<'arena>>,
}

/// Finds the outermost expressions a piece of code runs: its expression statements and the bodies of its
/// arrow functions, but not the statements inside closures those expressions pass on.
pub struct Finder;

impl<'ast, 'arena> Walker<'ast, 'arena, Found<'ast, 'arena>> for Finder {
    fn walk_in_statement_expression(&self, statement: &'ast ExpressionStatement<'arena>, found: &mut Found<'ast, 'arena>) {
        if found.depth == 0 {
            found.expressions.push(statement.expression);
        }

        found.depth += 1;
    }

    fn walk_out_statement_expression(&self, _: &'ast ExpressionStatement<'arena>, found: &mut Found<'ast, 'arena>) {
        found.depth -= 1;
    }

    fn walk_in_arrow_function(&self, function: &'ast ArrowFunction<'arena>, found: &mut Found<'ast, 'arena>) {
        if found.depth == 0 {
            found.expressions.push(function.expression);
        }

        found.depth += 1;
    }

    fn walk_out_arrow_function(&self, _: &'ast ArrowFunction<'arena>, found: &mut Found<'ast, 'arena>) {
        found.depth -= 1;
    }
}
