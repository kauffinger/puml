use serde::Serialize;

pub const FORMAT_VERSION: u32 = 1;

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
