//! What a pull request changes in the graph. The graph at the base is compared with the graph at the
//! head: classes and their public members, class edges, the module edges they add up to, and the verdict.
//! On top of that come the two things a reviewer cannot read off a text diff: the order in which to
//! read the changed classes, and the classes outside the pull request that depend on what it changes.
//!
//! Class edges are matched by source, target, kind and label, except that static and signature edges
//! are matched without their label, which only lists the members involved. Evidence lines are never
//! compared: an edge that moved down because lines were inserted above it has not changed.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use serde::Serialize;

use crate::git::ChangedFile;
use crate::git::Changes;
use crate::graph::ClassNode;
use crate::graph::Edge;
use crate::graph::EdgeKind;
use crate::graph::Graph;
use crate::graph::Method;
use crate::graph::Property;
use crate::verdict::ClassFanOut;
use crate::verdict::Cost;
use crate::verdict::ModuleEdge;
use crate::verdict::is_vendor;
use crate::verdict::module_of;
use crate::verdict::strongly_connected;

pub const DIFF_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct Diff {
    pub version: u32,
    pub project: String,
    pub generator: String,
    pub base: Revision,
    pub head: Revision,
    /// Changed files that declare a class at the base or at the head.
    pub files: Vec<ChangedFile>,
    /// Every other changed file in the project directory: routes, config, views, migrations, ...
    pub other_files: Vec<ChangedFile>,
    pub classes: ClassChanges,
    pub edges: EdgeChanges,
    /// Edges between app modules whose weight changed, in class edges.
    pub modules: Vec<ModuleChange>,
    pub violations: ViolationChanges,
    pub cycles: Vec<CycleChange>,
    pub metrics: Vec<MetricsChange>,
    pub outliers: OutlierChanges,
    /// Classes the pull request removes that are still used at the head.
    pub dangling: Vec<Dangling>,
    pub impact: Impact,
    /// The changed and added classes, each after the ones it depends on. Classes that depend on each
    /// other share a group.
    pub reading_order: Vec<Vec<String>>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Revision {
    pub reference: String,
    pub commit: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub uncommitted: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct ClassChanges {
    pub added: Vec<ClassRef>,
    pub removed: Vec<ClassRef>,
    pub renamed: Vec<Renamed>,
    /// Classes at both ends whose file changed, whether or not anything the graph records changed.
    pub modified: Vec<Modified>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassRef {
    pub id: String,
    pub kind: &'static str,
    pub module: String,
    pub file: String,
    pub line: u32,
}

#[derive(Debug, Serialize)]
pub struct Renamed {
    pub from: ClassRef,
    pub to: ClassRef,
}

#[derive(Debug, Serialize)]
pub struct Modified {
    pub class: ClassRef,
    /// Changes to the members other classes can see: everything but private methods and properties.
    pub members: Vec<MemberChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberChange {
    pub change: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct EdgeChanges {
    /// As at the head, with the head's evidence.
    pub added: Vec<Edge>,
    /// As at the base, with the base's evidence.
    pub removed: Vec<Edge>,
}

#[derive(Debug, Serialize)]
pub struct ModuleChange {
    pub from: String,
    pub to: String,
    pub before: u64,
    pub after: u64,
    /// Rule violations among the edge's class edges at the head.
    pub violations: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct ViolationChanges {
    pub added: Vec<ViolationRef>,
    pub removed: Vec<ViolationRef>,
}

#[derive(Debug, Serialize)]
pub struct ViolationRef {
    pub rule: &'static str,
    pub edge: Edge,
}

#[derive(Debug, Serialize)]
pub struct CycleChange {
    /// `new`, `grown`, `shrunk`, `changed` or `broken`.
    pub status: &'static str,
    /// The cycles at the base that share a module with this one. Modules are those still in a cycle
    /// once traits, interfaces and abstract classes are counted with their users, as in the verdict.
    pub before: Vec<Vec<String>>,
    pub after: Option<Vec<String>>,
    pub joined: Vec<String>,
    pub left: Vec<String>,
    /// The cheapest cut at the head.
    pub cut: Vec<CutEdge>,
    pub cost: Option<Cost>,
}

#[derive(Debug, Serialize)]
pub struct CutEdge {
    pub from: String,
    pub to: String,
    pub weight: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ModuleNumbers {
    pub classes: usize,
    pub afferent: usize,
    pub efferent: usize,
    pub instability: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct MetricsChange {
    pub module: String,
    pub before: Option<ModuleNumbers>,
    pub after: Option<ModuleNumbers>,
}

#[derive(Debug, Default, Serialize)]
pub struct OutlierChanges {
    pub added: Vec<ClassFanOut>,
    pub removed: Vec<ClassFanOut>,
}

#[derive(Debug, Serialize)]
pub struct Dangling {
    pub class: String,
    pub used_by: Vec<Edge>,
}

#[derive(Debug, Default, Serialize)]
pub struct Impact {
    /// Each modified class with dependents outside the pull request, public API changes first.
    pub classes: Vec<ClassImpact>,
    /// Every class outside the pull request that depends on a modified class, directly or through
    /// others, counted per module.
    pub reach: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub struct ClassImpact {
    pub class: String,
    pub api_changed: bool,
    pub dependents: Vec<String>,
}

type EdgeKey<'a> = (&'a str, &'a str, EdgeKind, Option<&'a str>);

fn key(edge: &Edge) -> EdgeKey<'_> {
    let label = match edge.kind {
        EdgeKind::Static | EdgeKind::Signature => None,
        _ => edge.label.as_deref(),
    };

    (&edge.from, &edge.to, edge.kind, label)
}

struct Side<'a> {
    graph: &'a Graph,
    classes: HashMap<&'a str, &'a ClassNode>,
    edges: HashMap<EdgeKey<'a>, &'a Edge>,
}

impl<'a> Side<'a> {
    fn new(graph: &'a Graph) -> Self {
        Side {
            graph,
            classes: graph.classes.iter().map(|class| (class.id.as_str(), class)).collect(),
            edges: graph.edges.iter().map(|edge| (key(edge), edge)).collect(),
        }
    }

    fn declared(&self, id: &str) -> Option<&'a ClassNode> {
        self.classes.get(id).copied().filter(|class| !class.external)
    }

    fn declared_classes(&self) -> impl Iterator<Item = &'a ClassNode> {
        self.graph.classes.iter().filter(|class| !class.external)
    }
}

impl Diff {
    pub fn of(base: &Graph, head: &Graph, changes: &Changes) -> Diff {
        let before = Side::new(base);
        let after = Side::new(head);

        let declaring: HashSet<&str> = before
            .declared_classes()
            .chain(after.declared_classes())
            .filter_map(|class| class.file.as_deref())
            .collect();
        let (files, other_files): (Vec<ChangedFile>, Vec<ChangedFile>) = changes
            .files
            .iter()
            .cloned()
            .partition(|file| [file.base_path(), file.head_path()].into_iter().flatten().any(|path| declaring.contains(path)));

        let changed_at_base: HashSet<&str> = files.iter().filter_map(ChangedFile::base_path).collect();
        let changed_at_head: HashSet<&str> = files.iter().filter_map(ChangedFile::head_path).collect();

        let classes = class_changes(&before, &after, &files, &changed_at_base, &changed_at_head);
        let in_pull_request: HashSet<&str> = after
            .declared_classes()
            .filter(|class| class.file.as_deref().is_some_and(|file| changed_at_head.contains(file)))
            .map(|class| class.id.as_str())
            .collect();

        Diff {
            version: DIFF_FORMAT_VERSION,
            project: head.project.clone(),
            generator: head.generator.clone(),
            base: Revision { reference: changes.base_ref.clone(), commit: changes.base_commit.clone(), uncommitted: false },
            head: Revision { reference: "HEAD".to_string(), commit: changes.head_commit.clone(), uncommitted: changes.uncommitted },
            edges: edge_changes(&before, &after),
            modules: module_changes(&before, &after),
            violations: violation_changes(base, head),
            cycles: cycle_changes(base, head),
            metrics: metrics_changes(base, head),
            outliers: outlier_changes(base, head),
            dangling: dangling(&before, &after),
            impact: impact(&after, &classes, &in_pull_request),
            reading_order: reading_order(&after, &in_pull_request),
            notes: notes(&other_files),
            classes,
            files,
            other_files,
        }
    }

    /// Whether the pull request adds a rule violation, a module to a cycle, or a use of a class it removes.
    pub fn worsens(&self) -> bool {
        !self.violations.added.is_empty() || self.cycles.iter().any(|cycle| !cycle.joined.is_empty()) || !self.dangling.is_empty()
    }
}

fn class_ref(class: &ClassNode) -> ClassRef {
    ClassRef {
        id: class.id.clone(),
        kind: class.kind,
        module: module_of(class),
        file: class.file.clone().unwrap_or_default(),
        line: class.line.unwrap_or_default(),
    }
}

fn class_changes(
    before: &Side<'_>,
    after: &Side<'_>,
    files: &[ChangedFile],
    changed_at_base: &HashSet<&str>,
    changed_at_head: &HashSet<&str>,
) -> ClassChanges {
    let mut changes = ClassChanges::default();

    let mut added: Vec<&ClassNode> = after.declared_classes().filter(|class| before.declared(&class.id).is_none()).collect();
    let mut removed: Vec<&ClassNode> = before.declared_classes().filter(|class| after.declared(&class.id).is_none()).collect();

    // A file git saw renamed that declared exactly one class before and declares exactly one other class
    // now holds a renamed class.
    for file in files.iter().filter(|file| file.from.is_some()) {
        let from = file.from.as_deref();
        let gone: Vec<usize> = removed.iter().enumerate().filter(|(_, class)| class.file.as_deref() == from).map(|(index, _)| index).collect();
        let new: Vec<usize> = added.iter().enumerate().filter(|(_, class)| class.file.as_deref() == Some(&file.path)).map(|(index, _)| index).collect();

        if let ([gone], [new]) = (gone.as_slice(), new.as_slice()) {
            changes.renamed.push(Renamed { from: class_ref(removed.remove(*gone)), to: class_ref(added.remove(*new)) });
        }
    }

    changes.added = added.into_iter().map(class_ref).collect();
    changes.removed = removed.into_iter().map(class_ref).collect();

    for class in after.declared_classes() {
        let Some(previous) = before.declared(&class.id) else {
            continue;
        };

        let touched = class.file.as_deref().is_some_and(|file| changed_at_head.contains(file))
            || previous.file.as_deref().is_some_and(|file| changed_at_base.contains(file));

        if touched {
            changes.modified.push(Modified { class: class_ref(class), members: member_changes(previous, class) });
        }
    }

    changes.added.sort_by(|a, b| a.id.cmp(&b.id));
    changes.removed.sort_by(|a, b| a.id.cmp(&b.id));
    changes.renamed.sort_by(|a, b| a.to.id.cmp(&b.to.id));
    changes.modified.sort_by(|a, b| a.class.id.cmp(&b.class.id));

    changes
}

/// The members other classes can use, keyed the way PHP looks them up: methods case-insensitively.
fn visible_members(class: &ClassNode) -> BTreeMap<String, String> {
    let mut members = BTreeMap::new();

    for method in class.methods.iter().filter(|method| method.visibility != "private") {
        members.insert(format!("method {}", method.name.to_ascii_lowercase()), method_signature(method));
    }

    for property in class.properties.iter().filter(|property| property.visibility != "private") {
        members.insert(format!("property {}", property.name), property_signature(property));
    }

    for constant in &class.constants {
        members.insert(format!("constant {constant}"), format!("const {constant}"));
    }

    for case in &class.cases {
        members.insert(format!("case {case}"), format!("case {case}"));
    }

    members
}

fn member_changes(before: &ClassNode, after: &ClassNode) -> Vec<MemberChange> {
    let (before, after) = (visible_members(before), visible_members(after));
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();

    keys.into_iter()
        .filter_map(|key| match (before.get(key), after.get(key)) {
            (Some(old), Some(new)) if old == new => None,
            (old, new) => Some(MemberChange {
                change: match (old, new) {
                    (None, _) => "added",
                    (_, None) => "removed",
                    _ => "changed",
                },
                before: old.cloned(),
                after: new.cloned(),
            }),
        })
        .collect()
}

pub fn method_signature(method: &Method) -> String {
    let params: Vec<String> = method
        .params
        .iter()
        .map(|param| match &param.r#type {
            Some(kind) => format!("{kind} ${}", param.name),
            None => format!("${}", param.name),
        })
        .collect();
    let modifiers = [Some(method.visibility), method.r#abstract.then_some("abstract"), method.r#static.then_some("static")];
    let modifiers: Vec<&str> = modifiers.into_iter().flatten().collect();
    let returns = method.returns.as_ref().map(|returns| format!(": {returns}")).unwrap_or_default();

    format!("{} function {}({}){returns}", modifiers.join(" "), method.name, params.join(", "))
}

fn property_signature(property: &Property) -> String {
    let modifiers = [Some(property.visibility), property.r#static.then_some("static"), property.readonly.then_some("readonly")];
    let modifiers: Vec<&str> = modifiers.into_iter().flatten().collect();
    let kind = property.r#type.as_ref().map(|kind| format!("{kind} ")).unwrap_or_default();

    format!("{} {kind}${}", modifiers.join(" "), property.name)
}

fn edge_changes(before: &Side<'_>, after: &Side<'_>) -> EdgeChanges {
    let only = |one: &Side<'_>, other: &Side<'_>| -> Vec<Edge> {
        one.graph.edges.iter().filter(|edge| !other.edges.contains_key(&key(edge))).cloned().collect()
    };

    EdgeChanges { added: only(after, before), removed: only(before, after) }
}

/// App-to-app module edges with their weight and the rule violations among their class edges.
fn module_weights(graph: &Graph) -> BTreeMap<(String, String), (u64, u64)> {
    let classes: HashMap<&str, &ClassNode> = graph.classes.iter().map(|class| (class.id.as_str(), class)).collect();
    let violating: HashSet<usize> = graph.verdict.violations.iter().map(|violation| violation.edge).collect();
    let mut weights: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();

    for (index, edge) in graph.edges.iter().enumerate() {
        let (from, to) = (classes[edge.from.as_str()], classes[edge.to.as_str()]);

        if is_vendor(from) || is_vendor(to) {
            continue;
        }

        let (from, to) = (module_of(from), module_of(to));

        if from != to {
            let weight = weights.entry((from, to)).or_default();
            weight.0 += 1;
            weight.1 += u64::from(violating.contains(&index));
        }
    }

    weights
}

fn module_changes(before: &Side<'_>, after: &Side<'_>) -> Vec<ModuleChange> {
    let (old, new) = (module_weights(before.graph), module_weights(after.graph));
    let pairs: BTreeSet<&(String, String)> = old.keys().chain(new.keys()).collect();

    pairs
        .into_iter()
        .filter_map(|pair| {
            let (weight_before, _) = old.get(pair).copied().unwrap_or_default();
            let (weight_after, violations) = new.get(pair).copied().unwrap_or_default();

            (weight_before != weight_after).then(|| ModuleChange {
                from: pair.0.clone(),
                to: pair.1.clone(),
                before: weight_before,
                after: weight_after,
                violations,
            })
        })
        .collect()
}

fn violation_changes(base: &Graph, head: &Graph) -> ViolationChanges {
    let (old, new) = (violations_by_key(base), violations_by_key(head));

    ViolationChanges { added: violations_only_in(&new, &old, head), removed: violations_only_in(&old, &new, base) }
}

fn violations_by_key(graph: &Graph) -> BTreeMap<(&'static str, EdgeKey<'_>), usize> {
    graph.verdict.violations.iter().map(|violation| ((violation.rule, key(&graph.edges[violation.edge])), violation.edge)).collect()
}

fn violations_only_in(
    one: &BTreeMap<(&'static str, EdgeKey<'_>), usize>,
    other: &BTreeMap<(&'static str, EdgeKey<'_>), usize>,
    graph: &Graph,
) -> Vec<ViolationRef> {
    one.iter()
        .filter(|(key, _)| !other.contains_key(*key))
        .map(|((rule, _), edge)| ViolationRef { rule, edge: graph.edges[*edge].clone() })
        .collect()
}

fn cycle_changes(base: &Graph, head: &Graph) -> Vec<CycleChange> {
    let old: Vec<BTreeSet<&str>> = base.verdict.cycles.iter().map(|cycle| cycle.remaining.iter().map(String::as_str).collect()).collect();
    let mut matched: HashSet<usize> = HashSet::new();
    let mut changes = Vec::new();

    for cycle in &head.verdict.cycles {
        let modules: BTreeSet<&str> = cycle.remaining.iter().map(String::as_str).collect();
        let overlapping: Vec<usize> = (0..old.len()).filter(|index| !old[*index].is_disjoint(&modules)).collect();
        matched.extend(&overlapping);

        let previous: BTreeSet<&str> = overlapping.iter().flat_map(|index| old[*index].iter().copied()).collect();

        if previous == modules && overlapping.len() == 1 {
            continue;
        }

        let joined: Vec<String> = modules.difference(&previous).map(|id| id.to_string()).collect();
        let left: Vec<String> = previous.difference(&modules).map(|id| id.to_string()).collect();

        changes.push(CycleChange {
            status: match (overlapping.is_empty(), joined.is_empty(), left.is_empty()) {
                (true, _, _) => "new",
                (false, false, true) => "grown",
                (false, true, false) => "shrunk",
                _ => "changed",
            },
            before: overlapping.iter().map(|index| base.verdict.cycles[*index].remaining.clone()).collect(),
            after: Some(cycle.remaining.clone()),
            joined,
            left,
            cut: cycle.cut.iter().map(|edge| CutEdge { from: edge.from.clone(), to: edge.to.clone(), weight: edge.weight }).collect(),
            cost: Some(cycle.cost.clone()),
        });
    }

    for (index, cycle) in base.verdict.cycles.iter().enumerate() {
        if !matched.contains(&index) {
            changes.push(CycleChange {
                status: "broken",
                before: vec![cycle.remaining.clone()],
                after: None,
                joined: Vec::new(),
                left: cycle.remaining.clone(),
                cut: Vec::new(),
                cost: None,
            });
        }
    }

    changes
}

fn metrics_changes(base: &Graph, head: &Graph) -> Vec<MetricsChange> {
    let numbers = |graph: &Graph| -> BTreeMap<String, ModuleNumbers> {
        graph
            .verdict
            .modules
            .iter()
            .map(|module| {
                let numbers = ModuleNumbers {
                    classes: module.classes,
                    afferent: module.afferent,
                    efferent: module.efferent,
                    instability: module.instability,
                };

                (module.id.clone(), numbers)
            })
            .collect()
    };

    let (old, new) = (numbers(base), numbers(head));
    let modules: BTreeSet<&String> = old.keys().chain(new.keys()).collect();

    modules
        .into_iter()
        .filter_map(|module| {
            let (before, after) = (old.get(module).copied(), new.get(module).copied());

            (before != after).then(|| MetricsChange { module: module.clone(), before, after })
        })
        .collect()
}

fn outlier_changes(base: &Graph, head: &Graph) -> OutlierChanges {
    let only = |one: &Graph, other: &Graph| -> Vec<ClassFanOut> {
        let ids: HashSet<&str> = other.verdict.outliers.classes.iter().map(|outlier| outlier.id.as_str()).collect();

        one.verdict.outliers.classes.iter().filter(|outlier| !ids.contains(outlier.id.as_str())).cloned().collect()
    };

    OutlierChanges { added: only(head, base), removed: only(base, head) }
}

/// A class declared at the base that the head still uses but no longer declares. The head's graph has
/// it as an undeclared class under an app root.
fn dangling(before: &Side<'_>, after: &Side<'_>) -> Vec<Dangling> {
    after
        .graph
        .classes
        .iter()
        .filter(|class| class.external && class.root.is_some() && before.declared(&class.id).is_some())
        .map(|class| Dangling {
            class: class.id.clone(),
            used_by: after.graph.edges.iter().filter(|edge| edge.to == class.id).cloned().collect(),
        })
        .collect()
}

fn impact(after: &Side<'_>, classes: &ClassChanges, in_pull_request: &HashSet<&str>) -> Impact {
    let mut users: HashMap<&str, BTreeSet<&str>> = HashMap::new();

    for edge in &after.graph.edges {
        if after.declared(&edge.from).is_some() {
            users.entry(&edge.to).or_default().insert(&edge.from);
        }
    }

    let mut entries: Vec<ClassImpact> = classes
        .modified
        .iter()
        .filter_map(|modified| {
            let dependents: Vec<String> = users
                .get(modified.class.id.as_str())
                .into_iter()
                .flatten()
                .filter(|id| !in_pull_request.contains(**id))
                .map(|id| id.to_string())
                .collect();

            (!dependents.is_empty()).then(|| ClassImpact { class: modified.class.id.clone(), api_changed: !modified.members.is_empty(), dependents })
        })
        .collect();

    entries.sort_by(|a, b| {
        (b.api_changed, b.dependents.len()).cmp(&(a.api_changed, a.dependents.len())).then_with(|| a.class.cmp(&b.class))
    });

    let mut seen: HashSet<&str> = classes.modified.iter().map(|modified| modified.class.id.as_str()).collect();
    let mut queue: VecDeque<&str> = seen.iter().copied().collect();
    let mut reach: BTreeMap<String, usize> = BTreeMap::new();

    while let Some(id) = queue.pop_front() {
        for &user in users.get(id).into_iter().flatten() {
            if !seen.insert(user) {
                continue;
            }

            queue.push_back(user);

            if !in_pull_request.contains(user) {
                *reach.entry(module_of(after.classes[user])).or_default() += 1;
            }
        }
    }

    Impact { classes: entries, reach }
}

fn reading_order(after: &Side<'_>, in_pull_request: &HashSet<&str>) -> Vec<Vec<String>> {
    let ids: Vec<String> = in_pull_request.iter().map(|id| id.to_string()).collect::<BTreeSet<_>>().into_iter().collect();
    let edges: Vec<ModuleEdge> = after
        .graph
        .edges
        .iter()
        .filter(|edge| in_pull_request.contains(edge.from.as_str()) && in_pull_request.contains(edge.to.as_str()))
        .map(|edge| ModuleEdge {
            from: edge.from.clone(),
            to: edge.to.clone(),
            weight: 1,
            kinds: BTreeMap::new(),
            violations: 0,
            edges: Vec::new(),
        })
        .collect();

    let mut groups = strongly_connected(&ids, &edges);

    for group in &mut groups {
        group.sort();
    }

    groups.sort();

    let group_of: HashMap<&str, usize> =
        groups.iter().enumerate().flat_map(|(index, group)| group.iter().map(move |id| (id.as_str(), index))).collect();
    let mut needs: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); groups.len()];

    for edge in &edges {
        let (from, to) = (group_of[edge.from.as_str()], group_of[edge.to.as_str()]);

        if from != to {
            needs[from].insert(to);
        }
    }

    let mut done: Vec<bool> = vec![false; groups.len()];
    let mut order: Vec<Vec<String>> = Vec::new();

    while let Some(next) = (0..groups.len()).find(|&index| !done[index] && needs[index].iter().all(|need| done[*need])) {
        done[next] = true;
        order.push(groups[next].clone());
    }

    order
}

fn notes(other_files: &[ChangedFile]) -> Vec<String> {
    let mut notes = Vec::new();

    if other_files.iter().any(|file| file.path == "composer.json") {
        notes.push("composer.json changed: the base graph uses the PSR-4 roots of the base composer.json.".to_string());
    }

    if other_files.iter().any(|file| file.path == "composer.lock") {
        notes.push("composer.lock changed: both graphs take their packages from the head, from vendor/ or else from its composer.lock.".to_string());
    }

    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FileStatus;
    use crate::graph::Evidence;
    use crate::graph::FORMAT_VERSION;
    use crate::graph::Package;
    use crate::graph::PackageKind;
    use crate::graph::Parameter;
    use crate::graph::Stats;
    use crate::verdict::Verdict;

    fn declare(id: &str) -> ClassNode {
        let mut class = ClassNode::external(id, "app");
        class.root = Some("App".to_string());
        class.external = false;
        class.file = Some(format!("app/{}.php", id.trim_start_matches("App\\").replace('\\', "/")));
        class.line = Some(3);
        class
    }

    fn edge(from: &str, to: &str, kind: EdgeKind, label: Option<&str>, line: u32) -> Edge {
        let file = format!("app/{}.php", from.trim_start_matches("App\\").replace('\\', "/"));

        Edge {
            from: from.to_string(),
            to: to.to_string(),
            kind,
            label: label.map(str::to_string),
            evidence: vec![Evidence { file, line }],
        }
    }

    fn reference(from: &str, to: &str) -> Edge {
        edge(from, to, EdgeKind::Reference, None, 5)
    }

    fn graph(mut classes: Vec<ClassNode>, edges: Vec<Edge>) -> Graph {
        let declared: HashSet<String> = classes.iter().map(|class| class.id.clone()).collect();

        for edge in &edges {
            if !declared.contains(&edge.to) && !classes.iter().any(|class| class.id == edge.to) {
                let mut node = ClassNode::external(&edge.to, "app");
                node.root = Some("App".to_string());
                classes.push(node);
            }
        }

        let packages = vec![
            Package { name: "app".to_string(), kind: PackageKind::App },
            Package { name: "laravel/framework".to_string(), kind: PackageKind::Vendor },
        ];

        Graph {
            version: FORMAT_VERSION,
            project: "app".to_string(),
            path: "/app".to_string(),
            generator: "test".to_string(),
            roots: vec!["App".to_string()],
            verdict: Verdict::of(&packages, &classes, &edges),
            packages,
            classes,
            edges,
            unresolved: Vec::new(),
            stats: Stats::default(),
        }
    }

    fn modified(paths: &[&str]) -> Changes {
        Changes {
            base_ref: "main".to_string(),
            base_commit: "base".to_string(),
            head_commit: "head".to_string(),
            uncommitted: false,
            files: paths.iter().map(|path| ChangedFile { path: path.to_string(), status: FileStatus::Modified, from: None }).collect(),
        }
    }

    fn classes(ids: &[&str]) -> Vec<ClassNode> {
        ids.iter().map(|id| declare(id)).collect()
    }

    #[test]
    fn a_graph_compared_with_itself_has_no_changes() {
        let ids = ["App\\Models\\Post", "App\\Http\\PostController", "App\\Jobs\\Publish"];
        let edges = || vec![reference("App\\Http\\PostController", "App\\Models\\Post"), reference("App\\Jobs\\Publish", "App\\Http\\PostController")];
        let diff = Diff::of(&graph(classes(&ids), edges()), &graph(classes(&ids), edges()), &modified(&[]));

        assert!(diff.files.is_empty() && diff.classes.added.is_empty() && diff.classes.removed.is_empty() && diff.classes.modified.is_empty());
        assert!(diff.edges.added.is_empty() && diff.edges.removed.is_empty() && diff.modules.is_empty());
        assert!(diff.violations.added.is_empty() && diff.violations.removed.is_empty() && diff.cycles.is_empty() && diff.metrics.is_empty());
        assert!(diff.dangling.is_empty() && diff.impact.classes.is_empty() && diff.reading_order.is_empty());
        assert!(!diff.worsens());
    }

    #[test]
    fn edges_that_only_moved_or_changed_members_are_the_same_edge() {
        let ids = ["App\\Http\\PostController", "App\\Models\\Post"];
        let base = graph(classes(&ids), vec![edge("App\\Http\\PostController", "App\\Models\\Post", EdgeKind::Static, Some("Post::find()"), 10)]);
        let head = graph(classes(&ids), vec![edge("App\\Http\\PostController", "App\\Models\\Post", EdgeKind::Static, Some("Post::find(), Post::query()"), 14)]);
        let diff = Diff::of(&base, &head, &modified(&["app/Http/PostController.php"]));

        assert!(diff.edges.added.is_empty() && diff.edges.removed.is_empty());
    }

    #[test]
    fn relations_are_told_apart_by_their_label() {
        let ids = ["App\\Models\\Post", "App\\Models\\User"];
        let base = graph(classes(&ids), vec![edge("App\\Models\\Post", "App\\Models\\User", EdgeKind::Relation, Some("author(): BelongsTo"), 9)]);
        let head = graph(classes(&ids), vec![edge("App\\Models\\Post", "App\\Models\\User", EdgeKind::Relation, Some("editor(): BelongsTo"), 9)]);
        let diff = Diff::of(&base, &head, &modified(&["app/Models/Post.php"]));

        assert_eq!(diff.edges.added.len(), 1);
        assert_eq!(diff.edges.removed.len(), 1);
    }

    #[test]
    fn dependencies_are_read_before_the_classes_that_use_them_and_cycles_are_read_together() {
        let ids = ["App\\Http\\Controller", "App\\Jobs\\Job", "App\\Models\\Post", "App\\Models\\User", "App\\Unrelated"];
        let edges = vec![
            reference("App\\Http\\Controller", "App\\Jobs\\Job"),
            reference("App\\Jobs\\Job", "App\\Models\\Post"),
            reference("App\\Models\\Post", "App\\Models\\User"),
            reference("App\\Models\\User", "App\\Models\\Post"),
        ];
        let base = graph(classes(&ids[1..]), edges[1..].to_vec());
        let head = graph(classes(&ids), edges);
        let mut changes = modified(&["app/Jobs/Job.php", "app/Models/Post.php", "app/Models/User.php", "app/Unrelated.php"]);
        changes.files.push(ChangedFile { path: "app/Http/Controller.php".to_string(), status: FileStatus::Added, from: None });

        let diff = Diff::of(&base, &head, &changes);

        assert_eq!(
            diff.reading_order,
            vec![
                vec!["App\\Models\\Post".to_string(), "App\\Models\\User".to_string()],
                vec!["App\\Jobs\\Job".to_string()],
                vec!["App\\Http\\Controller".to_string()],
                vec!["App\\Unrelated".to_string()],
            ]
        );
    }

    #[test]
    fn a_cycle_that_takes_in_a_module_has_grown() {
        let ids = ["App\\A\\One", "App\\B\\Two", "App\\C\\Three"];
        let base = graph(classes(&ids), vec![reference("App\\A\\One", "App\\B\\Two"), reference("App\\B\\Two", "App\\A\\One")]);
        let head = graph(
            classes(&ids),
            vec![
                reference("App\\A\\One", "App\\B\\Two"),
                reference("App\\B\\Two", "App\\A\\One"),
                reference("App\\B\\Two", "App\\C\\Three"),
                reference("App\\C\\Three", "App\\A\\One"),
            ],
        );
        let diff = Diff::of(&base, &head, &modified(&["app/B/Two.php", "app/C/Three.php"]));

        assert_eq!(diff.cycles.len(), 1);
        assert_eq!(diff.cycles[0].status, "grown");
        assert_eq!(diff.cycles[0].joined, vec!["App\\C".to_string()]);
        assert!(diff.worsens());

        let reverse = Diff::of(&head, &base, &modified(&["app/B/Two.php", "app/C/Three.php"]));

        assert_eq!(reverse.cycles[0].status, "shrunk");
        assert_eq!(reverse.cycles[0].left, vec!["App\\C".to_string()]);
        assert!(!reverse.worsens());
    }

    #[test]
    fn a_trait_counted_with_its_users_does_not_join_their_cycle() {
        let mut trait_node = declare("App\\Concerns\\HasTwo");
        trait_node.kind = "trait";

        let mut classes = classes(&["App\\A\\One", "App\\B\\Two"]);
        let base = graph(classes.clone(), vec![reference("App\\A\\One", "App\\B\\Two"), reference("App\\B\\Two", "App\\A\\One")]);

        classes.push(trait_node);

        let head = graph(
            classes,
            vec![
                reference("App\\A\\One", "App\\B\\Two"),
                reference("App\\B\\Two", "App\\A\\One"),
                edge("App\\A\\One", "App\\Concerns\\HasTwo", EdgeKind::UsesTrait, None, 5),
                reference("App\\Concerns\\HasTwo", "App\\B\\Two"),
            ],
        );

        assert!(head.verdict.cycles[0].modules.contains(&"App\\Concerns".to_string()));

        let diff = Diff::of(&base, &head, &modified(&["app/A/One.php", "app/Concerns/HasTwo.php"]));

        assert!(diff.cycles.is_empty());
        assert!(!diff.worsens());
    }

    #[test]
    fn a_cycle_with_no_module_left_in_a_cycle_is_broken() {
        let ids = ["App\\A\\One", "App\\B\\Two"];
        let base = graph(classes(&ids), vec![reference("App\\A\\One", "App\\B\\Two"), reference("App\\B\\Two", "App\\A\\One")]);
        let head = graph(classes(&ids), vec![reference("App\\A\\One", "App\\B\\Two")]);
        let diff = Diff::of(&base, &head, &modified(&["app/B/Two.php"]));

        assert_eq!(diff.cycles.len(), 1);
        assert_eq!(diff.cycles[0].status, "broken");
        assert_eq!(diff.modules[0].from, "App\\B");
        assert_eq!((diff.modules[0].before, diff.modules[0].after), (1, 0));
    }

    #[test]
    fn signature_changes_count_and_private_members_do_not() {
        let method = |name: &str, visibility: &'static str, param: Option<&str>| Method {
            name: name.to_string(),
            visibility,
            r#static: false,
            r#abstract: false,
            params: param.map(|kind| Parameter { name: "value".to_string(), r#type: Some(kind.to_string()) }).into_iter().collect(),
            returns: None,
            line: 4,
        };

        let mut before = declare("App\\Models\\Post");
        before.methods = vec![method("url", "public", None), method("slug", "private", None), method("old", "public", None)];

        let mut after = declare("App\\Models\\Post");
        after.methods = vec![method("URL", "public", Some("bool")), method("slug", "private", Some("int")), method("fresh", "protected", None)];

        assert_eq!(
            member_changes(&before, &after),
            vec![
                MemberChange { change: "added", before: None, after: Some("protected function fresh()".to_string()) },
                MemberChange { change: "removed", before: Some("public function old()".to_string()), after: None },
                MemberChange {
                    change: "changed",
                    before: Some("public function url()".to_string()),
                    after: Some("public function URL(bool $value)".to_string()),
                },
            ]
        );
    }

    #[test]
    fn a_removed_class_still_in_use_is_dangling() {
        let base = graph(classes(&["App\\Listeners\\Notify", "App\\Events\\Published"]), vec![reference("App\\Events\\Published", "App\\Listeners\\Notify")]);
        let head = graph(classes(&["App\\Events\\Published"]), vec![reference("App\\Events\\Published", "App\\Listeners\\Notify")]);
        let mut changes = modified(&[]);
        changes.files.push(ChangedFile { path: "app/Listeners/Notify.php".to_string(), status: FileStatus::Deleted, from: None });

        let diff = Diff::of(&base, &head, &changes);

        assert_eq!(diff.classes.removed.len(), 1);
        assert_eq!(diff.dangling.len(), 1);
        assert_eq!(diff.dangling[0].class, "App\\Listeners\\Notify");
        assert_eq!(diff.dangling[0].used_by[0].from, "App\\Events\\Published");
        assert!(diff.worsens());
    }

    #[test]
    fn impact_lists_the_users_outside_the_pull_request_and_counts_everything_upstream() {
        let ids = ["App\\Models\\Post", "App\\Jobs\\Publish", "App\\Http\\PostController", "App\\Http\\Admin", "App\\Console\\Prune"];
        let edges = || {
            vec![
                reference("App\\Jobs\\Publish", "App\\Models\\Post"),
                reference("App\\Console\\Prune", "App\\Models\\Post"),
                reference("App\\Http\\PostController", "App\\Jobs\\Publish"),
                reference("App\\Http\\Admin", "App\\Http\\PostController"),
            ]
        };
        let diff = Diff::of(&graph(classes(&ids), edges()), &graph(classes(&ids), edges()), &modified(&["app/Models/Post.php", "app/Jobs/Publish.php"]));

        let impact: Vec<(&str, Vec<String>)> =
            diff.impact.classes.iter().map(|entry| (entry.class.as_str(), entry.dependents.clone())).collect();

        assert_eq!(
            impact,
            vec![
                ("App\\Jobs\\Publish", vec!["App\\Http\\PostController".to_string()]),
                ("App\\Models\\Post", vec!["App\\Console\\Prune".to_string()]),
            ]
        );
        assert_eq!(diff.impact.reach, BTreeMap::from([("App\\Console".to_string(), 1), ("App\\Http".to_string(), 2)]));
    }
}
