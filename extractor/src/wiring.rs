//! What one file registers with Laravel, read without running it: routes, event listeners, observers,
//! policies, container bindings, scheduled tasks and middleware. Each registration keeps the class and
//! method it is written in, because whether Laravel runs it depends on that class: a call in a service
//! provider runs at boot, the same call in a controller does not. `wiring_graph` decides that once every
//! file has been read.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use mago_allocator::LocalArena;
use mago_database::file::File;
use mago_database::file::FileType;
use mago_names::resolver::NameResolver;
use mago_span::HasPosition;
use mago_syntax::cst::*;
use mago_syntax::parser::parse_file;
use mago_syntax::walker::Walker;

use crate::php::*;
use crate::project::Project;
use crate::routes::Group;
use crate::routes::RouteItem;
use crate::routes::RouteReader;

#[derive(Clone, Debug, Default)]
pub struct WiringFacts {
    pub routes: Vec<RouteScope>,
    pub registrations: Vec<Registered>,
    pub properties: Vec<PropertyValue>,
    pub attributes: Vec<ClassAttribute>,
    pub returns: Vec<MethodReturn>,
    pub bootstrap: Option<Bootstrap>,
}

/// Route registrations written at the top of a file, or in one method of a class.
#[derive(Clone, Debug)]
pub struct RouteScope {
    pub class: Option<String>,
    pub items: Vec<RouteItem>,
}

#[derive(Clone, Debug)]
pub struct Registered {
    pub class: Option<String>,
    pub method: Option<String>,
    pub line: u32,
    pub wire: Wire,
}

#[derive(Clone, Debug)]
pub enum Wire {
    Listen { events: Vec<String>, listener: Listener },
    Subscribe { subscriber: String },
    Observe { model: String, observers: Vec<String> },
    Policy { model: String, policy: String },
    GuessPolicyNames,
    Bind { lifetime: &'static str, key: String, concrete: Concrete },
    Schedule { task: &'static str, target: String, class: Option<String> },
    Middleware { slot: String, add: bool, classes: Vec<String> },
    Require { path: String },
    /// `$this->app->register(Provider::class)`.
    Provider { class: String },
    /// `Fortify::createUsersUsing(CreateNewUser::class)`: a static call on another class with class names
    /// as arguments, which may bind them in the container.
    Call { class: String, method: String, arguments: Vec<Option<String>> },
    Unresolved { kind: &'static str, detail: String },
}

impl Wire {
    fn kind(&self) -> &'static str {
        match self {
            Wire::Listen { .. } | Wire::Subscribe { .. } => "listener",
            Wire::Observe { .. } => "observer",
            Wire::Policy { .. } | Wire::GuessPolicyNames => "policy",
            Wire::Bind { .. } => "binding",
            Wire::Schedule { .. } => "schedule",
            Wire::Middleware { .. } => "middleware",
            Wire::Require { .. } | Wire::Provider { .. } => "provider",
            Wire::Call { .. } => "binding",
            Wire::Unresolved { kind, .. } => kind,
        }
    }

    fn describe(&self) -> String {
        match self {
            Wire::Listen { events, .. } => format!("a listener for {} is registered", events.join(", ")),
            Wire::Subscribe { subscriber } => format!("{subscriber} is subscribed"),
            Wire::Observe { model, observers } => format!("{} observes {model}", observers.join(", ")),
            Wire::Policy { model, policy } => format!("{policy} is the policy of {model}"),
            Wire::GuessPolicyNames => "policy names are guessed by a callback".to_string(),
            Wire::Bind { key, .. } => format!("{key} is bound"),
            Wire::Schedule { task, target, .. } => format!("the {task} {target} is scheduled"),
            Wire::Middleware { slot, classes, .. } => format!("{} is added to {slot}", classes.join(", ")),
            Wire::Require { path } => format!("{path} is loaded"),
            Wire::Provider { class } => format!("{class} is registered"),
            Wire::Call { class, method, .. } => format!("{class}::{method}() is called"),
            Wire::Unresolved { detail, .. } => detail.clone(),
        }
    }
}

/// Framework methods that listen to one event for you, such as `DB::listen()`.
const LISTENING_HELPERS: &[(&str, &str, &str)] = &[
    ("DB", "listen", "Illuminate\\Database\\Events\\QueryExecuted"),
    ("Queue", "before", "Illuminate\\Queue\\Events\\JobProcessing"),
    ("Queue", "after", "Illuminate\\Queue\\Events\\JobProcessed"),
    ("Queue", "exceptionOccurred", "Illuminate\\Queue\\Events\\JobExceptionOccurred"),
    ("Queue", "looping", "Illuminate\\Queue\\Events\\Looping"),
    ("Queue", "failing", "Illuminate\\Queue\\Events\\JobFailed"),
];

#[derive(Clone, Debug)]
pub enum Listener {
    Class(String),
    Method(String, String),
    /// A method of the subscriber itself, `'handleLogin'` in `subscribe()`.
    Own(String),
    Closure,
}

#[derive(Clone, Debug)]
pub enum Concrete {
    Same,
    Class(String),
    /// The value of the method's parameter at this index, as in `Fortify::createUsersUsing($callback)`.
    Parameter(usize),
    Unresolved(String),
}

/// A property default such as `$listen`, `$policies` or `$signature`.
#[derive(Clone, Debug)]
pub struct PropertyValue {
    pub class: String,
    pub name: String,
    pub line: u32,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub enum Value {
    Text(String),
    List(Vec<String>),
    Map(Vec<(String, Vec<String>)>),
}

#[derive(Clone, Debug)]
pub struct ClassAttribute {
    pub class: String,
    pub attribute: String,
    pub line: u32,
    /// Positional arguments under `None`, named ones under their name; class constants as class names.
    pub arguments: Vec<(Option<String>, Vec<String>)>,
}

/// What a method such as `shouldDiscoverEvents()` or `subscribe()` returns, when it is a literal.
#[derive(Clone, Debug)]
pub struct MethodReturn {
    pub class: String,
    pub method: String,
    pub line: u32,
    pub value: Returned,
}

#[derive(Clone, Debug)]
pub enum Returned {
    Bool(bool),
    Paths(Vec<String>),
    Listeners(Vec<(String, Listener)>),
    Other,
}

/// `bootstrap/app.php`: how `Application::configure()` sets the application up.
#[derive(Clone, Debug, Default)]
pub struct Bootstrap {
    pub configure: bool,
    pub routing: Option<Routing>,
    pub discover: Option<Discovery>,
    pub commands: Vec<String>,
    pub providers: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Routing {
    pub web: Vec<String>,
    pub api: Vec<String>,
    pub api_prefix: String,
    pub commands: Option<String>,
    pub then: Vec<RouteItem>,
    pub using: Option<Vec<RouteItem>>,
    pub line: u32,
}

#[derive(Clone, Debug)]
pub enum Discovery {
    Paths(Vec<String>),
    Disabled,
}

pub fn collect(file: &File, relative: &str, program: &Program<'_>, names: &mago_names::ResolvedNames<'_>) -> WiringFacts {
    let source = Source { file, relative, names };
    let mut context = Context { source: &source, classes: Vec::new(), method: None, roles: vec![HashMap::new()], pending: HashMap::new(), nested: 0, strings: Vec::new(), conditions: Vec::new(), parameters: Vec::new(), facts: WiringFacts::default() };

    let mut reader = RouteReader::new(&source);
    reader.statements(program.statements.iter(), &[]);

    if !reader.items.is_empty() {
        context.facts.routes.push(RouteScope { class: None, items: reader.items });
    }

    WiringWalker.walk_program(program, &mut context);

    context.facts
}

/// What a file outside the project registers, such as the service provider of a package.
pub fn collect_path(path: &Path, relative: &str) -> Option<WiringFacts> {
    let contents = std::fs::read(path).ok()?;
    let file = File::new(Cow::Owned(relative.as_bytes().to_vec()), FileType::Vendored, None, Cow::Owned(contents));
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    let names = NameResolver::new(&arena).resolve(program);

    Some(collect(&file, relative, program, &names))
}

/// The classes a file names as `Foo::class`: all of them, or those inside the array entry `key`. This is how
/// bootstrap/providers.php and the `providers` entry of config/app.php list the service providers.
pub fn listed_classes(project: &Project, relative: &str, key: Option<&str>) -> Vec<String> {
    let Some(contents) = project.read(relative) else {
        return Vec::new();
    };

    let file = File::new(Cow::Owned(relative.as_bytes().to_vec()), FileType::Host, None, Cow::Owned(contents));
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    let names = NameResolver::new(&arena).resolve(program);
    let source = Source { file: &file, relative, names: &names };
    let mut listed = Listed { source: &source, key, inside: 0, classes: Vec::new() };

    ListedWalker.walk_program(program, &mut listed);

    listed.classes
}

struct Listed<'s, 'a, 'arena> {
    source: &'s Source<'a, 'arena>,
    key: Option<&'s str>,
    inside: usize,
    classes: Vec<String>,
}

impl Listed<'_, '_, '_> {
    fn is_key(&self, element: &KeyValueArrayElement<'_>) -> bool {
        self.key.is_some_and(|key| self.source.string(element.key).as_deref() == Some(key))
    }
}

struct ListedWalker;

impl<'ast, 'arena> Walker<'ast, 'arena, Listed<'_, '_, 'arena>> for ListedWalker {
    fn walk_in_key_value_array_element(&self, element: &'ast KeyValueArrayElement<'arena>, listed: &mut Listed<'_, '_, 'arena>) {
        if listed.is_key(element) {
            listed.inside += 1;
        }
    }

    fn walk_out_key_value_array_element(&self, element: &'ast KeyValueArrayElement<'arena>, listed: &mut Listed<'_, '_, 'arena>) {
        if listed.is_key(element) {
            listed.inside -= 1;
        }
    }

    fn walk_in_class_constant_access(&self, access: &'ast ClassConstantAccess<'arena>, listed: &mut Listed<'_, '_, 'arena>) {
        if listed.key.is_some() && listed.inside == 0 {
            return;
        }

        let (Expression::Identifier(identifier), ClassLikeConstantSelector::Identifier(constant)) = (access.class, &access.constant) else {
            return;
        };

        if constant.value.eq_ignore_ascii_case(b"class") {
            listed.classes.push(listed.source.name_of(identifier));
        }
    }
}

/// The route registrations of a file outside the PSR-4 roots that a route file loads.
pub fn read_route_file(project: &Project, relative: &str) -> Option<Vec<RouteItem>> {
    route_items(relative, project.read(relative)?)
}

pub fn route_items(relative: &str, contents: Vec<u8>) -> Option<Vec<RouteItem>> {
    let file = File::new(Cow::Owned(relative.as_bytes().to_vec()), FileType::Host, None, Cow::Owned(contents));
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    let names = NameResolver::new(&arena).resolve(program);
    let source = Source { file: &file, relative, names: &names };
    let mut reader = RouteReader::new(&source);

    reader.statements(program.statements.iter(), &[]);

    Some(reader.items)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    Schedule,
    Middleware,
    Events,
    Container,
}

struct Context<'s, 'a, 'arena> {
    source: &'s Source<'a, 'arena>,
    classes: Vec<Option<String>>,
    method: Option<String>,
    roles: Vec<HashMap<String, Role>>,
    /// Closures passed to `withMiddleware()` and `withSchedule()`, whose first parameter has a role.
    pending: HashMap<u32, Role>,
    /// Closures entered inside the current method; their `return` is not the method's.
    nested: usize,
    /// The string properties of each class being read, innermost last.
    strings: Vec<HashMap<String, String>>,
    /// The lines of the conditions and loops around the code being read, innermost last.
    conditions: Vec<u32>,
    /// The parameters of the method being read.
    parameters: Vec<String>,
    facts: WiringFacts,
}

impl<'arena> Context<'_, '_, 'arena> {
    fn class(&self) -> Option<String> {
        self.classes.last().cloned().flatten()
    }

    fn register(&mut self, offset: u32, wire: Wire) {
        let line = self.source.line(offset);

        let wire = match (self.conditions.last(), wire) {
            (Some(_), Wire::Call { .. }) => return,
            (_, wire @ Wire::Unresolved { .. }) => wire,
            (Some(condition), wire) => Wire::Unresolved {
                kind: wire.kind(),
                detail: format!("{} only when the condition or loop on line {condition} runs it", wire.describe()),
            },
            (None, wire) => wire,
        };

        self.facts.registrations.push(Registered { class: self.class(), method: self.method.clone(), line, wire });
    }

    fn unresolved(&mut self, offset: u32, kind: &'static str, detail: String) {
        self.register(offset, Wire::Unresolved { kind, detail });
    }

    fn role_of(&self, variable: &str) -> Option<Role> {
        self.roles.iter().rev().find_map(|scope| scope.get(variable).copied())
    }

    fn enter(&mut self, parameters: &FunctionLikeParameterList<'arena>, first: Option<Role>) {
        let mut scope = HashMap::new();

        for (index, parameter) in parameters.parameters.iter().enumerate() {
            let name = String::from_utf8_lossy(parameter.variable.name).into_owned();
            let typed = parameter.hint.as_ref().and_then(|hint| match hint {
                Hint::Identifier(identifier) => role_of_type(&self.source.name_of(identifier)),
                _ => None,
            });

            if let Some(role) = typed.or(if index == 0 { first } else { None }) {
                scope.insert(name, role);
            }
        }

        self.roles.push(scope);
    }

    fn declare(&mut self, name: &LocalIdentifier<'_>, attribute_lists: &Sequence<'_, AttributeList<'arena>>) {
        let class = String::from_utf8_lossy(self.source.names.resolve(name).unwrap_or(name.value)).into_owned();

        for list in attribute_lists.iter() {
            for attribute in list.attributes.iter() {
                let mut arguments = Vec::new();

                if let Some(list) = &attribute.argument_list {
                    for argument in list.arguments.iter() {
                        let (key, value) = match argument {
                            PartialArgument::Positional(positional) => (None, positional.value),
                            PartialArgument::Named(named) => (Some(String::from_utf8_lossy(named.name.value).into_owned()), named.value),
                            _ => continue,
                        };

                        arguments.push((key, self.source.strings(value).unwrap_or_default()));
                    }
                }

                self.facts.attributes.push(ClassAttribute {
                    class: class.clone(),
                    attribute: self.source.name_of(&attribute.name),
                    line: self.source.line(attribute.name.offset()),
                    arguments,
                });
            }
        }

        self.classes.push(Some(class));
    }

    fn property(&mut self, property: &Property<'arena>) {
        let Some(class) = self.class() else {
            return;
        };

        let items: Vec<&PropertyItem<'arena>> = match property {
            Property::Plain(plain) => plain.items.iter().collect(),
            Property::Hooked(hooked) => vec![&hooked.item],
        };

        for item in items {
            let PropertyItem::Concrete(item) = item else {
                continue;
            };

            let name = String::from_utf8_lossy(item.variable.name).trim_start_matches('$').to_string();

            if !WIRING_PROPERTIES.contains(&name.as_str()) {
                continue;
            }

            let Some(value) = self.value(item.value) else {
                continue;
            };

            self.facts.properties.push(PropertyValue { class: class.clone(), name, line: self.source.line(item.variable.offset()), value });
        }
    }

    fn value(&self, expression: &Expression<'arena>) -> Option<Value> {
        if let Some(text) = self.source.string(expression) {
            return Some(Value::Text(text));
        }

        let elements = array_elements(expression)?;

        if elements.iter().all(|element| matches!(element, ArrayElement::Value(_))) {
            return Some(Value::List(elements.iter().filter_map(|element| self.listener_text(element.value()?)).collect()));
        }

        let mut pairs = Vec::new();

        for element in elements {
            match element {
                ArrayElement::KeyValue(pair) => {
                    let Some(key) = self.source.class_name(pair.key) else {
                        continue;
                    };

                    let values = match array_elements(pair.value) {
                        Some(values) if !is_callable_pair(self.source, pair.value) => {
                            values.iter().filter_map(|value| self.listener_text(value.value()?)).collect()
                        }
                        _ => self.listener_text(pair.value).into_iter().collect(),
                    };

                    pairs.push((key, values));
                }
                ArrayElement::Value(value) => {
                    if let Some(text) = self.listener_text(value.value) {
                        pairs.push((text.clone(), vec![text]));
                    }
                }
                _ => {}
            }
        }

        Some(Value::Map(pairs))
    }

    /// A class name, a string, or `[Foo::class, 'method']` written as `Foo@method`.
    fn listener_text(&self, expression: &Expression<'arena>) -> Option<String> {
        if is_callable_pair(self.source, expression) {
            let elements = array_elements(expression)?;

            return Some(format!("{}@{}", self.source.class_name(elements[0].value()?)?, self.source.string(elements[1].value()?)?));
        }

        self.source.class_name(expression)
    }

    fn listener(&self, expression: &Expression<'arena>) -> Option<Listener> {
        match expression {
            Expression::Closure(_) | Expression::ArrowFunction(_) => Some(Listener::Closure),
            _ => {
                let text = self.listener_text(expression)?;

                Some(match text.split_once('@') {
                    Some((class, method)) => Listener::Method(class.to_string(), method.to_string()),
                    None => Listener::Class(text),
                })
            }
        }
    }

    fn closure_event_types(&self, expression: &Expression<'arena>) -> Option<Vec<String>> {
        let parameters = match expression {
            Expression::Closure(closure) => &closure.parameter_list,
            Expression::ArrowFunction(function) => &function.parameter_list,
            _ => return None,
        };

        let hint = parameters.parameters.first()?.hint.as_ref()?;
        let mut identifiers = Vec::new();
        hint_identifiers(hint, &mut identifiers);

        Some(identifiers.into_iter().map(|identifier| self.source.name_of(identifier)).collect())
    }

    fn static_call(&mut self, call: &StaticMethodCall<'arena>) {
        let Some(method) = method_name(&call.method) else {
            return;
        };

        let class = match call.class {
            Expression::Identifier(identifier) => self.source.name_of(identifier),
            Expression::Static(_) | Expression::Self_(_) => match self.class() {
                Some(class) => class,
                None => return,
            },
            _ => return,
        };

        let offset = call.class.offset();
        let arguments = &call.argument_list;

        match method.to_ascii_lowercase().as_str() {
            "configure" if self.class().is_none() && class.ends_with("Application") => {
                self.facts.bootstrap.get_or_insert_with(Bootstrap::default).configure = true;
            }
            "listen" | "subscribe" if is_facade(&class, "Event") => self.events(&method, arguments, offset),
            "policy" if is_facade(&class, "Gate") => {
                let model = argument(arguments, 0, "class").and_then(|model| self.source.class_name(model));
                let policy = argument(arguments, 1, "policy").and_then(|policy| self.source.class_name(policy));

                match (model, policy) {
                    (Some(model), Some(policy)) => self.register(offset, Wire::Policy { model, policy }),
                    _ => self.unresolved(offset, "policy", "Gate::policy() with a class that is only known at runtime".to_string()),
                }
            }
            "guesspolicynamesusing" if is_facade(&class, "Gate") => self.register(offset, Wire::GuessPolicyNames),
            "observe" => {
                let observers = argument(arguments, 0, "classes").and_then(|observers| self.observers(observers));

                match observers {
                    Some(observers) => self.register(offset, Wire::Observe { model: class, observers }),
                    None => self.unresolved(offset, "observer", format!("{class}::observe() with an observer that is only known at runtime")),
                }
            }
            _ if is_facade(&class, "Schedule") => self.schedule(&method, arguments, offset),
            _ if LISTENING_HELPERS.iter().any(|(facade, helper, _)| is_facade(&class, facade) && method.eq_ignore_ascii_case(helper)) => {
                let (_, _, event) = LISTENING_HELPERS.iter().find(|(facade, helper, _)| is_facade(&class, facade) && method.eq_ignore_ascii_case(helper)).expect("matched above");

                match argument(arguments, 0, "callback").and_then(|callback| self.listener(callback)) {
                    Some(listener) => self.register(offset, Wire::Listen { events: vec![event.to_string()], listener }),
                    None => self.unresolved(offset, "listener", format!("{class}::{method}() with a listener that is only known at runtime")),
                }
            }
            _ if is_facade(&class, "App") => self.binding(&method, arguments, offset),
            _ if self.class().is_some() && self.nested == 0 => {
                let arguments: Vec<Option<String>> = positional(arguments).iter().map(|value| self.source.class_constant(value)).collect();

                if arguments.iter().any(Option::is_some) {
                    self.register(offset, Wire::Call { class, method, arguments });
                }
            }
            _ => {}
        }
    }

    fn method_call(&mut self, call: &MethodCall<'arena>) {
        let Some(method) = method_name(&call.method) else {
            return;
        };

        let offset = call.method.offset();
        let arguments = &call.argument_list;

        if let Some((Base::Static(class), links)) = chain(self.source, call.object) {
            if is_facade(&class, "Artisan") && links.first().is_some_and(|link| link.method.eq_ignore_ascii_case("command")) && links[1..].iter().all(|link| configures_closure_command(&link.method)) && !configures_closure_command(&method) {
                return self.closure_command_schedule(links[0].arguments, offset);
            }
        }

        let receiver = match fluent_root(call.object) {
            Expression::Variable(Variable::Direct(variable)) if variable.name == b"$app" && self.class().is_none() => Some(Role::Container),
            Expression::Variable(Variable::Direct(variable)) => self.role_of(&String::from_utf8_lossy(variable.name)),
            Expression::Access(Access::Property(access)) => match (access.object, method_name(&access.property).as_deref()) {
                (Expression::Variable(Variable::Direct(variable)), Some("app")) if variable.name == b"$this" => Some(Role::Container),
                _ => None,
            },
            Expression::Call(Call::Function(function)) => match function.function {
                Expression::Identifier(identifier)
                    if function.argument_list.arguments.is_empty() && identifier.value().eq_ignore_ascii_case(b"app") =>
                {
                    Some(Role::Container)
                }
                _ => None,
            },
            _ => None,
        };

        match receiver {
            Some(Role::Schedule) => self.schedule(&method, arguments, offset),
            Some(Role::Events) => self.events(&method, arguments, offset),
            Some(Role::Container) => self.binding(&method, arguments, offset),
            Some(Role::Middleware) => self.middleware(&method, arguments, offset),
            None if self.class().is_none() => self.bootstrap(&method, arguments, offset),
            None => {}
        }
    }

    /// `Artisan::command('inspire', ...)->hourly()`: a closure command scheduled by name.
    fn closure_command_schedule(&mut self, arguments: &ArgumentList<'arena>, offset: u32) {
        match argument(arguments, 0, "signature").and_then(|signature| self.source.string(signature)) {
            Some(signature) => {
                let name = signature.split_whitespace().next().unwrap_or_default().to_string();

                self.register(offset, Wire::Schedule { task: "command", target: name, class: None });
            }
            None => self.unresolved(offset, "schedule", "a closure command whose name is only known at runtime is scheduled".to_string()),
        }
    }

    fn events(&mut self, method: &str, arguments: &ArgumentList<'arena>, offset: u32) {
        if method.eq_ignore_ascii_case("subscribe") {
            match argument(arguments, 0, "subscriber").and_then(|subscriber| self.source.class_name(subscriber)) {
                Some(subscriber) => self.register(offset, Wire::Subscribe { subscriber }),
                None => self.unresolved(offset, "listener", "a subscriber that is only known at runtime".to_string()),
            }

            return;
        }

        if !method.eq_ignore_ascii_case("listen") {
            return;
        }

        let Some(events) = argument(arguments, 0, "events") else {
            return;
        };

        if let Some(types) = self.closure_event_types(events) {
            return self.register(offset, Wire::Listen { events: types, listener: Listener::Closure });
        }

        let events = self.source.strings(events);
        let listener = argument(arguments, 1, "listener").and_then(|listener| self.listener(listener));

        match (events, listener) {
            (Some(events), Some(listener)) => self.register(offset, Wire::Listen { events, listener }),
            _ => self.unresolved(offset, "listener", "Event::listen() with an event or listener that is only known at runtime".to_string()),
        }
    }

    fn observers(&self, expression: &Expression<'arena>) -> Option<Vec<String>> {
        match expression {
            Expression::Instantiation(instantiation) => match instantiation.class {
                Expression::Identifier(identifier) => Some(vec![self.source.name_of(identifier)]),
                _ => None,
            },
            _ => self.source.strings(expression),
        }
    }

    fn schedule(&mut self, method: &str, arguments: &ArgumentList<'arena>, offset: u32) {
        let first = argument(arguments, 0, "command");

        let (task, target, class) = match method.to_ascii_lowercase().as_str() {
            "command" => match first.and_then(|command| self.source.class_constant(command)) {
                Some(class) => ("command", class.clone(), Some(class)),
                None => match first.and_then(|command| self.source.string(command)) {
                    Some(command) => ("command", command.split_whitespace().next().unwrap_or_default().to_string(), None),
                    None => return self.unresolved(offset, "schedule", "a scheduled command that is only known at runtime".to_string()),
                },
            },
            "job" => {
                let job = argument(arguments, 0, "job");
                let class = job.and_then(|job| match job {
                    Expression::Instantiation(instantiation) => match instantiation.class {
                        Expression::Identifier(identifier) => Some(self.source.name_of(identifier)),
                        _ => None,
                    },
                    _ => self.source.class_name(job),
                });

                match class {
                    Some(class) => ("job", class.clone(), Some(class)),
                    None => return self.unresolved(offset, "schedule", "a scheduled job that is only known at runtime".to_string()),
                }
            }
            "call" => ("call", "closure".to_string(), None),
            "exec" => ("exec", first.and_then(|command| self.source.string(command)).unwrap_or_default(), None),
            _ => return,
        };

        self.register(offset, Wire::Schedule { task, target, class });
    }

    fn binding(&mut self, method: &str, arguments: &ArgumentList<'arena>, offset: u32) {
        if method.eq_ignore_ascii_case("register") {
            if let Some(class) = argument(arguments, 0, "provider").and_then(|provider| self.source.class_name(provider)) {
                self.register(offset, Wire::Provider { class });
            }

            return;
        }

        let lifetime = match method.to_ascii_lowercase().as_str() {
            "bind" | "bindif" => "bind",
            "singleton" | "singletonif" => "singleton",
            "scoped" | "scopedif" => "scoped",
            _ => return,
        };

        let Some(key) = argument(arguments, 0, "abstract").and_then(|key| self.source.class_name(key)) else {
            return self.unresolved(offset, "binding", format!("{method}() with an abstract that is only known at runtime"));
        };

        let concrete = match argument(arguments, 1, "concrete") {
            None | Some(Expression::Literal(Literal::Null(_))) => Concrete::Same,
            Some(Expression::Variable(Variable::Direct(variable))) if self.parameters.iter().any(|parameter| parameter.as_bytes() == variable.name) => {
                Concrete::Parameter(self.parameters.iter().position(|parameter| parameter.as_bytes() == variable.name).expect("found above"))
            }
            Some(concrete @ (Expression::Closure(_) | Expression::ArrowFunction(_))) => match returned_instance(self.source, concrete) {
                Some(class) => Concrete::Class(class),
                None => Concrete::Unresolved(format!("the closure binding {key} does not return `new Class(...)`")),
            },
            Some(concrete) => match self.source.class_name(concrete) {
                Some(class) => Concrete::Class(class),
                None => Concrete::Unresolved(format!("{key} is bound to a value that is only known at runtime")),
            },
        };

        self.register(offset, Wire::Bind { lifetime, key, concrete });
    }

    fn middleware(&mut self, method: &str, arguments: &ArgumentList<'arena>, offset: u32) {
        let list = |index: usize, name: &str| argument(arguments, index, name).and_then(|value| self.source.strings(value)).unwrap_or_default();
        let mut changes: Vec<(String, bool, Vec<String>)> = Vec::new();

        match method.to_ascii_lowercase().as_str() {
            "append" | "prepend" | "use" => changes.push(("global".to_string(), true, list(0, "middleware"))),
            "remove" => changes.push(("global".to_string(), false, list(0, "middleware"))),
            "replace" => changes.push(("global".to_string(), true, list(1, "with"))),
            "web" | "api" => {
                let slot = format!("group:{}", method.to_ascii_lowercase());

                changes.push((slot.clone(), true, [list(0, "append"), list(1, "prepend")].concat()));
                changes.push((slot.clone(), false, list(2, "remove")));

                if let Some(replace) = argument(arguments, 3, "replace").and_then(array_pairs) {
                    let with = replace.iter().filter_map(|(_, with)| self.source.class_name(with)).collect();
                    changes.push((slot, true, with));
                }
            }
            "group" | "appendtogroup" | "prependtogroup" | "removefromgroup" | "replaceingroup" => {
                let Some(group) = argument(arguments, 0, "group").and_then(|group| self.source.string(group)) else {
                    return self.unresolved(offset, "middleware", "a middleware group whose name is only known at runtime".to_string());
                };

                let lower = method.to_ascii_lowercase();
                let (add, middleware) = match lower.as_str() {
                    "removefromgroup" => (false, list(1, "middleware")),
                    "replaceingroup" => (true, list(2, "with")),
                    _ => (true, list(1, "middleware")),
                };

                changes.push((format!("group:{group}"), add, middleware));
            }
            "alias" => {
                for (alias, class) in argument(arguments, 0, "aliases").and_then(array_pairs).unwrap_or_default() {
                    if let (Some(alias), Some(class)) = (self.source.string(alias), self.source.class_name(class)) {
                        changes.push((format!("alias:{alias}"), true, vec![class]));
                    }
                }
            }
            _ => {}
        }

        for (slot, add, classes) in changes {
            if !classes.is_empty() {
                self.register(offset, Wire::Middleware { slot, add, classes });
            }
        }
    }

    /// `Application::configure()->withRouting(...)->withEvents(...)` in bootstrap/app.php.
    fn bootstrap(&mut self, method: &str, arguments: &ArgumentList<'arena>, offset: u32) {
        match method {
            "withRouting" => {
                let paths = |name: &str| match argument(arguments, usize::MAX, name) {
                    Some(value) => match array_elements(value) {
                        Some(elements) => elements.iter().filter_map(|element| self.source.path(element.value()?)).collect(),
                        None => self.source.path(value).into_iter().collect(),
                    },
                    None => Vec::new(),
                };

                let mut routing = Routing {
                    web: paths("web"),
                    api: paths("api"),
                    api_prefix: argument(arguments, usize::MAX, "apiPrefix").and_then(|prefix| self.source.string(prefix)).unwrap_or_else(|| "api".to_string()),
                    commands: argument(arguments, usize::MAX, "commands").and_then(|commands| self.source.path(commands)),
                    line: self.source.line(offset),
                    ..Routing::default()
                };

                if let Some(then) = argument(arguments, usize::MAX, "then") {
                    let mut reader = RouteReader::new(self.source);
                    reader.callback(then, &[]);
                    routing.then = reader.items;
                }

                if let Some(using) = argument(arguments, 0, "using") {
                    let mut reader = RouteReader::new(self.source);
                    reader.callback(using, &[]);
                    routing.using = Some(reader.items);
                }

                self.facts.bootstrap.get_or_insert_with(Bootstrap::default).routing = Some(routing);
            }
            "withEvents" => {
                let discover = match argument(arguments, 0, "discover") {
                    Some(Expression::Literal(Literal::False(_))) => Some(Discovery::Disabled),
                    Some(value) => {
                        let paths: Vec<String> = array_elements(value)
                            .unwrap_or_default()
                            .iter()
                            .filter_map(|element| self.source.path(element.value()?))
                            .collect();

                        (!paths.is_empty()).then_some(Discovery::Paths(paths))
                    }
                    None => None,
                };

                self.facts.bootstrap.get_or_insert_with(Bootstrap::default).discover = discover;
            }
            "withProviders" => {
                let providers: Vec<String> = argument(arguments, 0, "providers")
                    .and_then(array_elements)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|value| value.value().and_then(|value| self.source.class_name(value)))
                    .collect();

                self.facts.bootstrap.get_or_insert_with(Bootstrap::default).providers.extend(providers);
            }
            "withCommands" => {
                let paths: Vec<String> = argument(arguments, 0, "commands")
                    .and_then(array_elements)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|value| value.value().and_then(|value| self.source.path(value)))
                    .collect();

                self.facts.bootstrap.get_or_insert_with(Bootstrap::default).commands.extend(paths);
            }
            "withMiddleware" | "withSchedule" => {
                let role = if method == "withMiddleware" { Role::Middleware } else { Role::Schedule };

                if let Some(callback) = argument(arguments, 0, "callback") {
                    self.pending.insert(callback.offset(), role);
                }
            }
            _ => {}
        }
    }

    fn method_return(&mut self, value: &Expression<'arena>, offset: u32) {
        let (Some(class), Some(method)) = (self.class(), self.method.clone()) else {
            return;
        };

        if self.nested > 0 {
            return;
        }

        let returned = match method.to_ascii_lowercase().as_str() {
            "shoulddiscoverevents" => match value {
                Expression::Literal(Literal::True(_)) => Returned::Bool(true),
                Expression::Literal(Literal::False(_)) => Returned::Bool(false),
                _ => Returned::Other,
            },
            "discovereventswithin" => {
                let paths: Option<Vec<String>> = array_elements(value).and_then(|elements| elements.iter().map(|element| self.source.path(element.value()?)).collect());

                paths.map_or(Returned::Other, Returned::Paths)
            }
            "subscribe" => {
                let mut listeners = Vec::new();

                for (event, listener) in array_pairs(value).unwrap_or_default() {
                    let Some(event) = self.source.class_name(event) else {
                        continue;
                    };

                    let values: Vec<&Expression<'arena>> = match array_elements(listener) {
                        Some(elements) if !is_callable_pair(self.source, listener) => elements.iter().filter_map(|element| element.value()).collect(),
                        _ => vec![listener],
                    };

                    for value in values {
                        let parsed = match self.source.string(value) {
                            Some(text) if !text.contains('@') && !text.contains('\\') => Some(Listener::Own(text)),
                            _ => self.listener(value),
                        };

                        if let Some(parsed) = parsed {
                            listeners.push((event.clone(), parsed));
                        }
                    }
                }

                Returned::Listeners(listeners)
            }
            _ => return,
        };

        let line = self.source.line(offset);

        self.facts.returns.push(MethodReturn { class, method, line, value: returned });
    }
}

const WIRING_PROPERTIES: &[&str] = &[
    "listen", "subscribe", "observers", "policies", "bindings", "singletons", "middleware", "middlewareGroups", "middlewareAliases",
    "routeMiddleware", "signature", "name", "observables",
];

/// Methods of a closure command that configure it rather than schedule it.
fn configures_closure_command(method: &str) -> bool {
    let method = method.to_ascii_lowercase();

    matches!(method.as_str(), "purpose" | "describe") || ["set", "add", "get", "ignore", "is", "has"].iter().any(|prefix| method.starts_with(prefix))
}

/// The variable a fluent chain starts from: `$middleware` in `$middleware->web(...)->alias(...)`.
fn fluent_root<'ast, 'arena>(expression: &'ast Expression<'arena>) -> &'ast Expression<'arena> {
    match expression {
        Expression::Call(Call::Method(call)) if matches!(call.object, Expression::Call(Call::Method(_)) | Expression::Variable(_)) => {
            let root = fluent_root(call.object);

            if matches!(root, Expression::Variable(_)) { root } else { expression }
        }
        _ => expression,
    }
}

fn role_of_type(name: &str) -> Option<Role> {
    match name.to_ascii_lowercase().as_str() {
        "illuminate\\console\\scheduling\\schedule" => Some(Role::Schedule),
        "illuminate\\foundation\\configuration\\middleware" => Some(Role::Middleware),
        "illuminate\\events\\dispatcher" | "illuminate\\contracts\\events\\dispatcher" => Some(Role::Events),
        "illuminate\\foundation\\application"
        | "illuminate\\contracts\\foundation\\application"
        | "illuminate\\container\\container"
        | "illuminate\\contracts\\container\\container" => Some(Role::Container),
        _ => None,
    }
}

fn is_callable_pair(source: &Source<'_, '_>, expression: &Expression<'_>) -> bool {
    let Some(elements) = array_elements(expression) else {
        return false;
    };

    matches!(elements.as_slice(), [class, method]
        if class.value().is_some_and(|class| source.class_constant(class).is_some())
            && method.value().is_some_and(|method| source.string(method).is_some()))
}

/// The class a binding closure creates, when its only `return` is `new Class(...)`.
fn returned_instance(source: &Source<'_, '_>, closure: &Expression<'_>) -> Option<String> {
    let value = match closure {
        Expression::ArrowFunction(function) => function.expression,
        Expression::Closure(closure) => {
            let returns: Vec<&Return<'_>> = closure
                .body
                .statements
                .iter()
                .filter_map(|statement| match statement {
                    Statement::Return(r#return) => Some(r#return),
                    _ => None,
                })
                .collect();

            let [only] = returns.as_slice() else {
                return None;
            };

            only.value?
        }
        _ => return None,
    };

    match value {
        Expression::Instantiation(instantiation) => match instantiation.class {
            Expression::Identifier(identifier) => Some(source.name_of(identifier)),
            _ => None,
        },
        _ => None,
    }
}

fn hint_identifiers<'ast, 'arena>(hint: &'ast Hint<'arena>, identifiers: &mut Vec<&'ast Identifier<'arena>>) {
    match hint {
        Hint::Identifier(identifier) => identifiers.push(identifier),
        Hint::Parenthesized(hint) => hint_identifiers(hint.hint, identifiers),
        Hint::Nullable(hint) => hint_identifiers(hint.hint, identifiers),
        Hint::Union(hint) => {
            hint_identifiers(hint.left, identifiers);
            hint_identifiers(hint.right, identifiers);
        }
        _ => {}
    }
}

struct WiringWalker;

impl<'ast, 'arena> Walker<'ast, 'arena, Context<'_, '_, 'arena>> for WiringWalker {
    fn walk_in_class(&self, class: &'ast Class<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.declare(&class.name, &class.attribute_lists);

        let mut strings = HashMap::new();

        for member in class.members.iter() {
            let ClassLikeMember::Property(Property::Plain(property)) = member else {
                continue;
            };

            for item in property.items.iter() {
                if let PropertyItem::Concrete(item) = item {
                    if let Some(value) = context.source.string(item.value) {
                        strings.insert(String::from_utf8_lossy(item.variable.name).trim_start_matches('$').to_string(), value);
                    }
                }
            }
        }

        context.strings.push(strings);
    }

    fn walk_out_class(&self, _: &'ast Class<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.classes.pop();
        context.strings.pop();
    }

    fn walk_in_interface(&self, interface: &'ast Interface<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.declare(&interface.name, &interface.attribute_lists);
    }

    fn walk_out_interface(&self, _: &'ast Interface<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.classes.pop();
    }

    fn walk_in_trait(&self, r#trait: &'ast Trait<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.declare(&r#trait.name, &r#trait.attribute_lists);
    }

    fn walk_out_trait(&self, _: &'ast Trait<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.classes.pop();
    }

    fn walk_in_enum(&self, r#enum: &'ast Enum<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.declare(&r#enum.name, &r#enum.attribute_lists);
    }

    fn walk_out_enum(&self, _: &'ast Enum<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.classes.pop();
    }

    fn walk_in_anonymous_class(&self, _: &'ast AnonymousClass<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.classes.push(None);
    }

    fn walk_out_anonymous_class(&self, _: &'ast AnonymousClass<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.classes.pop();
    }

    fn walk_in_property(&self, property: &'ast Property<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.property(property);
    }

    fn walk_in_method(&self, method: &'ast Method<'arena>, context: &mut Context<'_, '_, 'arena>) {
        let name = String::from_utf8_lossy(method.name.value).into_owned();
        let first = name.eq_ignore_ascii_case("subscribe").then_some(Role::Events);

        context.method = Some(name);
        context.parameters = method.parameter_list.parameters.iter().map(|parameter| String::from_utf8_lossy(parameter.variable.name).into_owned()).collect();
        context.enter(&method.parameter_list, first);

        if let (Some(class), MethodBody::Concrete(body)) = (context.class(), &method.body) {
            let mut reader = RouteReader::new(context.source);
            reader.route_file = false;
            reader.properties = context.strings.last().cloned().unwrap_or_default();
            reader.statements(body.statements.iter(), &[] as &[Group]);

            if !reader.items.is_empty() {
                context.facts.routes.push(RouteScope { class: Some(class), items: reader.items });
            }
        }
    }

    fn walk_out_method(&self, _: &'ast Method<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.method = None;
        context.roles.pop();
    }

    fn walk_in_closure(&self, closure: &'ast Closure<'arena>, context: &mut Context<'_, '_, 'arena>) {
        let first = context.pending.get(&closure.offset()).copied();
        context.enter(&closure.parameter_list, first);
        context.nested += 1;
    }

    fn walk_out_closure(&self, _: &'ast Closure<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.roles.pop();
        context.nested -= 1;
    }

    fn walk_in_arrow_function(&self, function: &'ast ArrowFunction<'arena>, context: &mut Context<'_, '_, 'arena>) {
        let first = context.pending.get(&function.offset()).copied();
        context.enter(&function.parameter_list, first);
        context.nested += 1;
    }

    fn walk_out_arrow_function(&self, _: &'ast ArrowFunction<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.roles.pop();
        context.nested -= 1;
    }

    fn walk_in_if(&self, statement: &'ast If<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.push(context.source.line(statement.offset()));
    }

    fn walk_out_if(&self, _: &'ast If<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.pop();
    }

    fn walk_in_switch(&self, statement: &'ast Switch<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.push(context.source.line(statement.offset()));
    }

    fn walk_out_switch(&self, _: &'ast Switch<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.pop();
    }

    fn walk_in_foreach(&self, statement: &'ast Foreach<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.push(context.source.line(statement.offset()));
    }

    fn walk_out_foreach(&self, _: &'ast Foreach<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.pop();
    }

    fn walk_in_for(&self, statement: &'ast For<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.push(context.source.line(statement.offset()));
    }

    fn walk_out_for(&self, _: &'ast For<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.pop();
    }

    fn walk_in_while(&self, statement: &'ast While<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.push(context.source.line(statement.offset()));
    }

    fn walk_out_while(&self, _: &'ast While<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.conditions.pop();
    }

    fn walk_in_static_method_call(&self, call: &'ast StaticMethodCall<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.static_call(call);
    }

    fn walk_in_method_call(&self, call: &'ast MethodCall<'arena>, context: &mut Context<'_, '_, 'arena>) {
        context.method_call(call);
    }

    fn walk_in_return(&self, r#return: &'ast Return<'arena>, context: &mut Context<'_, '_, 'arena>) {
        if let Some(value) = r#return.value {
            context.method_return(value, r#return.offset());
        }
    }

    fn walk_in_construct(&self, construct: &'ast Construct<'arena>, context: &mut Context<'_, '_, 'arena>) {
        let value = match construct {
            Construct::Require(require) => require.value,
            Construct::RequireOnce(require) => require.value,
            _ => return,
        };

        if context.class().is_some() {
            if let Some(path) = context.source.path(value) {
                context.register(value.offset(), Wire::Require { path });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registrations(source: &str) -> Vec<(u32, String)> {
        let dir = std::env::temp_dir().join(format!("puml-wiring-{}-{}", std::process::id(), source.len()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("AppServiceProvider.php"), source).unwrap();

        let facts = collect_path(&dir.join("AppServiceProvider.php"), "app/Providers/AppServiceProvider.php").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        facts
            .registrations
            .into_iter()
            .map(|registered| {
                let wire = match registered.wire {
                    Wire::Bind { lifetime, key, .. } => format!("{lifetime} {key}"),
                    Wire::Unresolved { kind, detail } => format!("unresolved {kind}: {detail}"),
                    other => other.describe(),
                };

                (registered.line, wire)
            })
            .collect()
    }

    #[test]
    fn a_registration_under_a_condition_is_noted_with_the_line_of_the_condition() {
        let registered = registrations(
            "<?php\n\nnamespace App\\Providers;\n\nuse App\\Contracts\\Feed;\nuse App\\Services\\RssFeed;\nuse App\\Services\\FakeFeed;\n\n\
             class AppServiceProvider extends \\Illuminate\\Support\\ServiceProvider\n{\n    public function register(): void\n    {\n        \
             $this->app->singleton(Feed::class, RssFeed::class);\n\n        \
             if ($this->app->runningUnitTests()) {\n            $this->app->bind(FakeFeed::class, FakeFeed::class);\n        }\n    }\n}\n",
        );

        assert_eq!(
            registered,
            [
                (13, "singleton App\\Contracts\\Feed".to_string()),
                (16, "unresolved binding: App\\Services\\FakeFeed is bound only when the condition or loop on line 15 runs it".to_string()),
            ]
        );
    }
}
