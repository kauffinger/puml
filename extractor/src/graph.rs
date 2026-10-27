use serde::Serialize;

use crate::verdict::Verdict;

pub const FORMAT_VERSION: u32 = 3;

#[derive(Debug, Serialize)]
pub struct Graph {
    pub version: u32,
    pub project: String,
    pub path: String,
    pub generator: String,
    pub roots: Vec<String>,
    pub packages: Vec<Package>,
    pub classes: Vec<ClassNode>,
    pub edges: Vec<Edge>,
    pub unresolved: Vec<Unresolved>,
    pub stats: Stats,
    pub verdict: Verdict,
    pub laravel: Laravel,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Package {
    pub name: String,
    pub kind: PackageKind,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    App,
    Vendor,
    Php,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassNode {
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub package: String,
    pub root: Option<String>,
    pub kind: &'static str,
    pub external: bool,
    pub layer: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub r#abstract: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub r#final: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cases: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<Property>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<Method>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
}

impl ClassNode {
    pub fn external(id: &str, package: &str) -> Self {
        let (namespace, name) = split_name(id);

        ClassNode {
            id: id.to_string(),
            name: name.to_string(),
            namespace: namespace.to_string(),
            package: package.to_string(),
            root: None,
            kind: "class",
            external: true,
            layer: "vendor",
            r#abstract: false,
            r#final: false,
            file: None,
            line: None,
            parent: None,
            interfaces: Vec::new(),
            traits: Vec::new(),
            constants: Vec::new(),
            cases: Vec::new(),
            properties: Vec::new(),
            methods: Vec::new(),
            relations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Property {
    pub name: String,
    pub visibility: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub r#static: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Method {
    pub name: String,
    pub visibility: &'static str,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub r#static: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub r#abstract: bool,
    pub params: Vec<Parameter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    pub line: u32,
    /// The classes the first parameter's type names, which is how Laravel discovers a listener's events.
    #[serde(skip)]
    pub first_parameter: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Parameter {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Relation {
    pub name: String,
    pub r#type: String,
    pub related: Option<String>,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Extends,
    Implements,
    UsesTrait,
    Injects,
    Property,
    Signature,
    Relation,
    New,
    Static,
    Facade,
    Reference,
    Route,
    Middleware,
    Listener,
    Observer,
    Policy,
    Binding,
    Schedule,
}

impl EdgeKind {
    /// Wiring edges are what Laravel connects at runtime: a route to its controller, an event to its
    /// listeners. They are drawn, but they are not dependencies in the code, so the rules, the cycles and
    /// the metrics leave them out.
    pub fn is_wiring(self) -> bool {
        matches!(
            self,
            EdgeKind::Route | EdgeKind::Middleware | EdgeKind::Listener | EdgeKind::Observer | EdgeKind::Policy | EdgeKind::Binding | EdgeKind::Schedule
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            EdgeKind::Extends => "extends",
            EdgeKind::Implements => "implements",
            EdgeKind::UsesTrait => "uses-trait",
            EdgeKind::Injects => "injects",
            EdgeKind::Property => "property",
            EdgeKind::Signature => "signature",
            EdgeKind::Relation => "relation",
            EdgeKind::New => "new",
            EdgeKind::Static => "static",
            EdgeKind::Facade => "facade",
            EdgeKind::Reference => "reference",
            EdgeKind::Route => "route",
            EdgeKind::Middleware => "middleware",
            EdgeKind::Listener => "listener",
            EdgeKind::Observer => "observer",
            EdgeKind::Policy => "policy",
            EdgeKind::Binding => "binding",
            EdgeKind::Schedule => "schedule",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Evidence {
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unresolved {
    pub kind: &'static str,
    pub detail: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct Stats {
    pub files: usize,
    pub files_with_parse_errors: usize,
    pub classes: usize,
    pub external_classes: usize,
    pub edges: usize,
    pub extract_ms: u128,
}

pub fn split_name(fqcn: &str) -> (&str, &str) {
    match fqcn.rfind('\\') {
        Some(index) => (&fqcn[..index], &fqcn[index + 1..]),
        None => ("", fqcn),
    }
}

/// What Laravel registers at boot, read from the code: each entry has the file:line it is written at.
#[derive(Debug, Default, Serialize)]
pub struct Laravel {
    pub routes: Vec<RouteFact>,
    pub listeners: Vec<ListenerFact>,
    pub observers: Vec<ObserverFact>,
    pub policies: Vec<PolicyFact>,
    pub bindings: Vec<BindingFact>,
    pub schedule: Vec<ScheduleFact>,
    pub middleware: Vec<MiddlewareFact>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteFact {
    /// The route file, or the service provider, that registers it.
    pub from: String,
    pub methods: Vec<String>,
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Class@method`, `view:name`, `redirect:/to`, `livewire:component` or `closure`.
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub middleware: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub excluded_middleware: Vec<String>,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ListenerFact {
    pub event: String,
    /// `Class@method`, or `closure`.
    pub listener: String,
    /// The listener's class; for a closure, the class it is written in.
    pub class: Option<String>,
    /// `discovered`, `listen` (Event::listen or $listen) or `subscribe`.
    pub how: &'static str,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObserverFact {
    pub model: String,
    pub observer: String,
    /// The model events the observer has a method for.
    pub events: Vec<String>,
    /// `observe`, `attribute` (#[ObservedBy]) or `property` ($observers).
    pub how: &'static str,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct PolicyFact {
    pub model: String,
    pub policy: String,
    /// `gate` (Gate::policy or $policies), `attribute` (#[UsePolicy]) or `convention` (the policy's name).
    pub how: &'static str,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct BindingFact {
    /// The class or key that is bound.
    pub key: String,
    /// The class the container builds for it; none when only the running code knows.
    pub concrete: Option<String>,
    /// `bind`, `singleton` or `scoped`.
    pub lifetime: &'static str,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScheduleFact {
    pub from: String,
    /// `command`, `job`, `call` or `exec`.
    pub task: &'static str,
    /// The command name, the job class, `closure`, or the shell command.
    pub target: String,
    pub class: Option<String>,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct MiddlewareFact {
    pub from: String,
    /// `global`, `group:web` or `alias:auth`.
    pub slot: String,
    pub class: String,
    pub file: String,
    pub line: u32,
}
