use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::collect::FileFacts;
use crate::collect::collect_file;
use crate::collect::RawEdge;
use crate::graph::ClassNode;
use crate::graph::Edge;
use crate::graph::EdgeKind;
use crate::graph::Evidence;
use crate::graph::FORMAT_VERSION;
use crate::graph::Graph;
use crate::graph::Package;
use crate::graph::PackageKind;
use crate::graph::Stats;
use crate::graph::Unresolved;
use crate::laravel::FacadeResolver;
use crate::laravel::FacadeTarget;
use crate::project::Project;

const PSEUDO_TYPES: &[&str] = &["self", "static", "parent", "$this"];

pub fn assemble(project: &Project, files: Vec<(PathBuf, FileFacts)>) -> Graph {
    let mut classes: Vec<ClassNode> = Vec::new();
    let mut sources: HashMap<String, PathBuf> = HashMap::new();
    let mut canonical: HashMap<String, String> = HashMap::new();
    let mut raw_edges: Vec<RawEdge> = Vec::new();
    let mut unresolved: Vec<Unresolved> = Vec::new();
    let mut dropped_declarations: HashSet<(String, u32)> = HashSet::new();
    let mut stats = Stats { files: files.len(), ..Stats::default() };

    for (path, facts) in files {
        stats.files_with_parse_errors += usize::from(facts.has_parse_errors);

        for mut class in facts.classes {
            let key = class.id.to_ascii_lowercase();

            if canonical.contains_key(&key) {
                unresolved.push(Unresolved {
                    kind: "duplicate-class",
                    detail: format!("{} is declared more than once; keeping the first declaration", class.id),
                    file: class.file.clone().unwrap_or_default(),
                    line: class.line.unwrap_or_default(),
                });

                dropped_declarations.insert((class.file.clone().unwrap_or_default(), class.line.unwrap_or_default()));

                continue;
            }

            class.package = project.name.clone();
            class.root = project.root_for(&class.id).map(str::to_string);
            canonical.insert(key.clone(), class.id.clone());
            sources.insert(key, path.clone());
            classes.push(class);
        }

        raw_edges.extend(facts.edges);
        unresolved.extend(facts.unresolved);
    }

    let mut facades = FacadeResolver::new(project);
    let mut reported_facades = BTreeSet::new();
    let mut grouped: BTreeMap<(String, String, EdgeKind, Option<String>), BTreeSet<Evidence>> = BTreeMap::new();

    for mut edge in raw_edges {
        if dropped_declarations.contains(&(edge.file.clone(), edge.declared_at)) {
            continue;
        }

        if edge.to.is_empty() || PSEUDO_TYPES.iter().any(|pseudo| edge.to.eq_ignore_ascii_case(pseudo)) {
            continue;
        }

        if let Some(facade) = facades.global_alias(&edge.to) {
            edge.to = facade.to_string();
        }

        edge.to = canonical.get(&edge.to.to_ascii_lowercase()).cloned().unwrap_or(edge.to);

        if edge.kind == EdgeKind::Static {
            let source = sources.get(&edge.to.to_ascii_lowercase()).cloned();

            match facades.resolve(&edge.to, source.as_deref()) {
                Some(FacadeTarget::Resolved(underlying)) => {
                    edge.label = Some(edge.to.clone());
                    edge.kind = EdgeKind::Facade;
                    edge.to = canonical.get(&underlying.to_ascii_lowercase()).cloned().unwrap_or(underlying.clone());
                }
                Some(FacadeTarget::Unresolved(reason)) => {
                    if reported_facades.insert(edge.to.clone()) {
                        unresolved.push(Unresolved {
                            kind: "facade",
                            detail: format!("{}: {reason}", edge.to),
                            file: edge.file.clone(),
                            line: edge.line,
                        });
                    }
                }
                None => {}
            }
        }

        if edge.from == edge.to {
            continue;
        }

        grouped
            .entry((edge.from, edge.to, edge.kind, edge.label))
            .or_default()
            .insert(Evidence { file: edge.file, line: edge.line });
    }

    let edges: Vec<Edge> = grouped
        .into_iter()
        .map(|((from, to, kind, label), evidence)| Edge { from, to, kind, label, evidence: evidence.into_iter().collect() })
        .collect();

    let mut packages: BTreeSet<Package> = BTreeSet::new();
    packages.insert(Package { name: project.name.clone(), kind: PackageKind::App });

    let mut externals: BTreeMap<String, ClassNode> = BTreeMap::new();

    for edge in &edges {
        if canonical.contains_key(&edge.to.to_ascii_lowercase()) || externals.contains_key(&edge.to) {
            continue;
        }

        let root = project.root_for(&edge.to).map(str::to_string);

        let (package, kind) = match project.vendor.package_for(&edge.to) {
            _ if root.is_some() => (project.name.clone(), PackageKind::App),
            Some(package) => (package.to_string(), PackageKind::Vendor),
            None if !edge.to.contains('\\') => ("php".to_string(), PackageKind::Php),
            None => (edge.to.split('\\').next().unwrap_or_default().to_string(), PackageKind::Unknown),
        };

        let mut node = ClassNode::external(&edge.to, &package);
        node.root = root;

        packages.insert(Package { name: package, kind });
        externals.insert(edge.to.clone(), node);
    }

    describe_vendor_traits(project, &edges, &mut externals, &mut unresolved);

    let index: HashMap<String, usize> = classes.iter().enumerate().map(|(index, class)| (class.id.clone(), index)).collect();

    for edge in &edges {
        let Some(&from) = index.get(&edge.from) else {
            continue;
        };

        let class = &mut classes[from];

        match edge.kind {
            EdgeKind::Extends if class.kind == "interface" => class.interfaces.push(edge.to.clone()),
            EdgeKind::Extends => class.parent = Some(edge.to.clone()),
            EdgeKind::Implements => class.interfaces.push(edge.to.clone()),
            EdgeKind::UsesTrait => class.traits.push(edge.to.clone()),
            _ => {}
        }
    }

    stats.classes = classes.len();
    stats.external_classes = externals.len();
    stats.edges = edges.len();

    classes.extend(externals.into_values());

    Graph {
        version: FORMAT_VERSION,
        project: project.name.clone(),
        path: project.root.to_string_lossy().into_owned(),
        generator: format!("puml-extract {} (mago 1.54.0)", env!("CARGO_PKG_VERSION")),
        roots: project.psr4.iter().map(|root| root.prefix.clone()).collect(),
        packages: packages.into_iter().collect(),
        classes,
        edges,
        unresolved,
        stats,
    }
}

/// Relations declared in vendor traits (Notifiable, HasApiTokens, ...) belong to every app model that
/// uses them, so the traits an app class uses are read from vendor, following nested trait uses.
fn describe_vendor_traits(
    project: &Project,
    edges: &[Edge],
    externals: &mut BTreeMap<String, ClassNode>,
    unresolved: &mut Vec<Unresolved>,
) {
    let mut pending: Vec<String> = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::UsesTrait && externals.contains_key(&edge.to))
        .map(|edge| edge.to.clone())
        .collect();

    let mut described: HashSet<String> = HashSet::new();

    while let Some(id) = pending.pop() {
        if !described.insert(id.to_ascii_lowercase()) {
            continue;
        }

        let Some(path) = project.vendor.file_for(&id) else {
            continue;
        };

        let relative = project.relative(&path);
        let facts = collect_file(&path, &relative);

        let Some(declared) = facts.classes.into_iter().find(|class| class.id.eq_ignore_ascii_case(&id)) else {
            continue;
        };

        let traits: Vec<String> = facts
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::UsesTrait && edge.from.eq_ignore_ascii_case(&declared.id))
            .map(|edge| edge.to.clone())
            .collect();

        for used in &traits {
            if !externals.contains_key(used) {
                let package = project.vendor.package_for(used).unwrap_or("unknown").to_string();
                externals.insert(used.clone(), ClassNode::external(used, &package));
            }

            pending.push(used.clone());
        }

        unresolved.extend(facts.unresolved.into_iter().filter(|note| note.kind == "relation"));

        let node = externals.get_mut(&id).expect("pending traits are external nodes");
        node.kind = declared.kind;
        node.file = declared.file;
        node.line = declared.line;
        node.traits = traits;
        node.relations = declared.relations;
    }
}
