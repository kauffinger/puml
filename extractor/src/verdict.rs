//! Architecture rules, module cycles and coupling metrics, judged once over the whole graph and written
//! into it, so that the viewer, `puml-extract report` and the tests all read the same verdict.
//!
//! The Laravel docs (Directory Structure, "The App Directory") describe Http and Console as "providing an
//! API into the core of your application. The HTTP protocol and CLI are both mechanisms to interact with
//! your application, but do not actually contain application logic." Every app class is therefore placed
//! in one layer by the first namespace segment under its PSR-4 root: `http` (Http, and the UI packages that
//! live beside it: Livewire, Filament, Nova, View), `console`, `providers`, `database` (the Database\*
//! roots), `tests`, and `core` for everything else. The rules say which layers may not depend on which,
//! and each violation is one class edge with its file:line. Vendor classes are never on either side of a
//! rule.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;

use serde::Serialize;

use crate::graph::ClassNode;
use crate::graph::Edge;
use crate::graph::EdgeKind;
use crate::graph::Package;
use crate::graph::PackageKind;

#[derive(Debug, Serialize)]
pub struct LayerSegments {
    pub layer: &'static str,
    pub segments: &'static [&'static str],
}

pub const LAYER_SEGMENTS: &[LayerSegments] = &[
    LayerSegments { layer: "http", segments: &["Http", "Livewire", "Filament", "Nova", "View"] },
    LayerSegments { layer: "console", segments: &["Console"] },
    LayerSegments { layer: "providers", segments: &["Providers"] },
];

#[derive(Debug, Serialize)]
pub struct Rule {
    pub id: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub why: &'static str,
}

pub const RULES: &[Rule] = &[
    Rule {
        id: "core-independent-of-http",
        from: "core",
        to: "http",
        why: "Http is an entry point into the core, not part of it: models, jobs, events and the rest must not know about controllers, requests, middleware or UI components.",
    },
    Rule {
        id: "core-independent-of-console",
        from: "core",
        to: "console",
        why: "Console is an entry point into the core, not part of it: the core must not know about Artisan commands.",
    },
    Rule {
        id: "http-independent-of-console",
        from: "http",
        to: "console",
        why: "Http and Console are two separate ways of issuing commands to the core; one must not go through the other.",
    },
    Rule {
        id: "console-independent-of-http",
        from: "console",
        to: "http",
        why: "Http and Console are two separate ways of issuing commands to the core; one must not go through the other.",
    },
    Rule {
        id: "events-independent-of-listeners",
        from: "segment:Events",
        to: "segment:Listeners",
        why: "Listeners receive events; an event that knows its listeners undoes the decoupling events exist for.",
    },
];

#[derive(Debug, Serialize)]
pub struct CycleRule {
    pub id: &'static str,
    pub why: &'static str,
}

pub const CYCLE_RULE: CycleRule = CycleRule {
    id: "no-module-cycles",
    why: "Modules that depend on each other in a cycle can only be understood, tested and changed together.",
};

pub const RULES_SCOPE: &str =
    "The rules describe a Laravel application, so they are only checked when the project depends on laravel/framework.";

/// The cheapest set of edges whose removal breaks a cycle is a minimum feedback arc set. Up to this many
/// modules it is found exactly; larger cycles get a greedy cut.
pub const EXACT_CUT_LIMIT: usize = 12;

#[derive(Debug, Serialize)]
pub struct Verdict {
    pub rules_apply: bool,
    pub rules_scope: &'static str,
    pub rules: &'static [Rule],
    pub cycle_rule: &'static CycleRule,
    pub layer_segments: &'static [LayerSegments],
    /// Sorted by rule, from, to and kind. `edge` is an index into the graph's edges.
    pub violations: Vec<Violation>,
    pub cycles: Vec<Cycle>,
    pub modules: Vec<ModuleMetrics>,
    /// Distinct app classes outside its own module that each app class depends on; zeros are left out.
    pub fan_out: BTreeMap<String, usize>,
    pub outliers: Outliers,
    pub layers: Vec<LayerEntry>,
}

#[derive(Debug, Serialize)]
pub struct Violation {
    pub rule: &'static str,
    pub edge: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleEdge {
    pub from: String,
    pub to: String,
    pub weight: u64,
    pub kinds: BTreeMap<EdgeKind, u64>,
    pub violations: u64,
    /// The class edges behind it, as indexes into the graph's edges. Only the edges of a cut carry them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct Cycle {
    pub modules: Vec<String>,
    /// Classes of the cycle's modules that are really parts of another module in it.
    pub folds: Vec<Fold>,
    /// The modules still in a cycle once the folds are counted where they belong; empty when nothing is.
    pub remaining: Vec<String>,
    /// Edges between the remaining modules, folds applied, lightest first.
    pub edges: Vec<ModuleEdge>,
    pub exact: bool,
    pub cut: Vec<ModuleEdge>,
    pub cost: Cost,
}

/// A trait, interface or abstract class whose every user (uses-trait, implements, extends) lives in one
/// other module is a part of that module, wherever its file is: a model trait in App\Concerns belongs to
/// App\Models. Cutting the edges between a module and its own parts breaks nothing worth breaking, so the
/// cut is chosen with each part counted in the module that uses it.
#[derive(Debug, Serialize)]
pub struct Fold {
    pub class: String,
    pub kind: &'static str,
    pub module: String,
    pub into: String,
    pub users: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct Cost {
    pub class_edges: u64,
    pub files: usize,
    pub violations: u64,
}

#[derive(Debug, Serialize)]
pub struct ModuleMetrics {
    pub id: String,
    pub classes: usize,
    pub abstractness: f64,
    pub afferent: usize,
    pub efferent: usize,
    pub instability: Option<f64>,
    pub distance: Option<f64>,
    pub vendor: usize,
    pub cycle: Option<usize>,
    pub busiest: Option<ClassFanOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassFanOut {
    pub id: String,
    pub fan_out: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct Outliers {
    pub threshold: Option<f64>,
    pub classes: Vec<ClassFanOut>,
}

#[derive(Debug, Serialize)]
pub struct LayerEntry {
    pub namespace: String,
    pub layer: &'static str,
    pub classes: usize,
}

pub struct Place<'a> {
    pub layer: &'static str,
    pub segment: Option<&'a str>,
}

pub fn is_vendor(class: &ClassNode) -> bool {
    class.external && class.root.is_none()
}

/// A module is a PSR-4 root plus the next namespace segment (App\Models), or one whole vendor package.
pub fn module_of(class: &ClassNode) -> String {
    if is_vendor(class) {
        return class.package.clone();
    }

    let root = class.root.as_deref().unwrap_or("");

    if class.namespace == root {
        return if root.is_empty() { "(global)".to_string() } else { root.to_string() };
    }

    let segment = under_root(&class.namespace, root).split('\\').next().unwrap_or("");

    if root.is_empty() { segment.to_string() } else { format!("{root}\\{segment}") }
}

pub fn place_of(class: &ClassNode) -> Place<'_> {
    if is_vendor(class) {
        return Place { layer: "vendor", segment: None };
    }

    let root = class.root.as_deref().unwrap_or("");
    let segment = under_root(&class.namespace, root).split('\\').next().filter(|segment| !segment.is_empty());

    if root.starts_with("Database") {
        return Place { layer: "database", segment };
    }

    if root == "Tests" || root.starts_with("Tests\\") {
        return Place { layer: "tests", segment };
    }

    for entry in LAYER_SEGMENTS {
        if segment.is_some_and(|segment| entry.segments.contains(&segment)) {
            return Place { layer: entry.layer, segment };
        }
    }

    Place { layer: "core", segment }
}

fn under_root<'a>(namespace: &'a str, root: &str) -> &'a str {
    if root.is_empty() {
        return namespace;
    }

    namespace.strip_prefix(root).and_then(|rest| rest.strip_prefix('\\')).unwrap_or("")
}

fn matches_selector(selector: &str, place: &Place<'_>) -> bool {
    match selector.strip_prefix("segment:") {
        Some(segment) => place.layer == "core" && place.segment == Some(segment),
        None => place.layer == selector,
    }
}

pub fn rule_between(from: &Place<'_>, to: &Place<'_>) -> Option<&'static Rule> {
    RULES.iter().find(|rule| matches_selector(rule.from, from) && matches_selector(rule.to, to))
}

pub fn rules_apply(packages: &[Package]) -> bool {
    packages.iter().any(|package| package.name == "laravel/framework" && package.kind == PackageKind::Vendor)
}

impl Verdict {
    pub fn of(packages: &[Package], classes: &[ClassNode], edges: &[Edge]) -> Verdict {
        let graph = Parts { classes, edges };
        let classes: HashMap<&str, &ClassNode> = classes.iter().map(|class| (class.id.as_str(), class)).collect();
        let rules_apply = rules_apply(packages);
        let mut violations = Vec::new();

        if rules_apply {
            for (index, edge) in edges.iter().enumerate() {
                if let Some(rule) = rule_between(&place_of(classes[edge.from.as_str()]), &place_of(classes[edge.to.as_str()])) {
                    violations.push(Violation { rule: rule.id, edge: index });
                }
            }
        }

        violations.sort_by(|a, b| {
            let (left, right) = (&edges[a.edge], &edges[b.edge]);

            (a.rule, &left.from, &left.to, left.kind.name()).cmp(&(b.rule, &right.from, &right.to, right.kind.name()))
        });

        let violating: HashSet<usize> = violations.iter().map(|violation| violation.edge).collect();
        let modules = app_modules(&graph, &classes, &violating);
        let cycles = module_cycles(&graph, &classes, &modules, &violating);
        let fan_out = class_fan_out(&graph, &classes);

        Verdict {
            rules_apply,
            rules_scope: RULES_SCOPE,
            rules: RULES,
            cycle_rule: &CYCLE_RULE,
            layer_segments: LAYER_SEGMENTS,
            modules: module_metrics(&graph, &classes, &modules, &cycles, &fan_out),
            outliers: fan_out_outliers(&fan_out),
            fan_out: fan_out.into_iter().filter(|(_, count)| *count > 0).map(|(id, count)| (id.to_string(), count)).collect(),
            layers: layers(&graph),
            violations,
            cycles,
        }
    }
}

struct Parts<'a> {
    classes: &'a [ClassNode],
    edges: &'a [Edge],
}

struct ModuleGraph<'a> {
    nodes: Vec<(String, Vec<&'a str>)>,
    edges: Vec<ModuleEdge>,
}

/// Every app module and the class edges between them, aggregated: the module view without vendor
/// packages and without a node budget.
fn app_modules<'a>(graph: &Parts<'a>, classes: &HashMap<&str, &'a ClassNode>, violating: &HashSet<usize>) -> ModuleGraph<'a> {
    let mut nodes: BTreeMap<String, Vec<&'a str>> = BTreeMap::new();
    let mut edges: BTreeMap<(String, String), ModuleEdge> = BTreeMap::new();

    for class in graph.classes.iter().filter(|class| !class.external) {
        nodes.entry(module_of(class)).or_default().push(&class.id);
    }

    for (index, edge) in graph.edges.iter().enumerate() {
        let (from, to) = (classes[edge.from.as_str()], classes[edge.to.as_str()]);

        if is_vendor(from) || is_vendor(to) {
            continue;
        }

        let (from_module, to_module) = (module_of(from), module_of(to));

        if from_module == to_module {
            continue;
        }

        for (module, id) in [(&from_module, &from.id), (&to_module, &to.id)] {
            let members = nodes.entry(module.clone()).or_default();

            if !members.contains(&id.as_str()) {
                members.push(id);
            }
        }

        aggregate(&mut edges, from_module, to_module, index, edge.kind, violating);
    }

    ModuleGraph { nodes: nodes.into_iter().collect(), edges: edges.into_values().collect() }
}

fn aggregate(
    edges: &mut BTreeMap<(String, String), ModuleEdge>,
    from: String,
    to: String,
    index: usize,
    kind: EdgeKind,
    violating: &HashSet<usize>,
) {
    let aggregated = edges.entry((from.clone(), to.clone())).or_insert_with(|| ModuleEdge {
        from,
        to,
        weight: 0,
        kinds: BTreeMap::new(),
        violations: 0,
        edges: Vec::new(),
    });

    aggregated.weight += 1;
    *aggregated.kinds.entry(kind).or_default() += 1;
    aggregated.violations += u64::from(violating.contains(&index));
    aggregated.edges.push(index);
}

struct Home {
    module: String,
    users: usize,
}

fn is_part(class: &ClassNode) -> bool {
    class.kind == "trait" || class.kind == "interface" || class.r#abstract
}

/// The module each trait, interface and abstract class belongs to when it differs from the one its file
/// is in: the one module holding all of its users. Users are placed where they belong themselves, so a
/// trait used only by traits of the models counts as a model trait too.
fn homes<'a>(graph: &Parts<'a>, classes: &HashMap<&str, &'a ClassNode>) -> HashMap<&'a str, Home> {
    let mut users: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in graph.edges {
        let (from, to) = (classes[edge.from.as_str()], classes[edge.to.as_str()]);

        if matches!(edge.kind, EdgeKind::Extends | EdgeKind::Implements | EdgeKind::UsesTrait) && !is_vendor(from) && !is_vendor(to) {
            users.entry(&edge.to).or_default().push(&edge.from);
        }
    }

    let parts: Vec<&ClassNode> = graph.classes.iter().filter(|class| !is_vendor(class) && is_part(class)).collect();
    let mut homes: HashMap<&str, Home> = HashMap::new();

    loop {
        let mut next: HashMap<&str, Home> = HashMap::new();

        for part in &parts {
            let Some(using) = users.get(part.id.as_str()) else {
                continue;
            };

            let modules: BTreeSet<String> = using
                .iter()
                .map(|user| homes.get(user).map_or_else(|| module_of(classes[user]), |home| home.module.clone()))
                .collect();

            if modules.len() == 1 {
                let module = modules.into_iter().next().expect("one module");

                if module != module_of(part) {
                    next.insert(&part.id, Home { module, users: using.len() });
                }
            }
        }

        let settled = next.len() == homes.len() && next.iter().all(|(id, home)| homes.get(id).is_some_and(|previous| previous.module == home.module));
        homes = next;

        if settled {
            break;
        }
    }

    homes
}

/// Tarjan's strongly connected components, visiting nodes in the given order.
pub fn strongly_connected(node_ids: &[String], edges: &[ModuleEdge]) -> Vec<Vec<String>> {
    struct Search<'a> {
        successors: HashMap<&'a str, Vec<&'a str>>,
        order: HashMap<&'a str, usize>,
        lowlink: HashMap<&'a str, usize>,
        stack: Vec<&'a str>,
        on_stack: HashSet<&'a str>,
        components: Vec<Vec<String>>,
    }

    impl<'a> Search<'a> {
        fn visit(&mut self, id: &'a str) {
            let counter = self.order.len();
            self.order.insert(id, counter);
            self.lowlink.insert(id, counter);
            self.stack.push(id);
            self.on_stack.insert(id);

            for next in self.successors.get(id).cloned().unwrap_or_default() {
                if !self.order.contains_key(next) {
                    self.visit(next);
                    let lowest = self.lowlink[id].min(self.lowlink[next]);
                    self.lowlink.insert(id, lowest);
                } else if self.on_stack.contains(next) {
                    let lowest = self.lowlink[id].min(self.order[next]);
                    self.lowlink.insert(id, lowest);
                }
            }

            if self.lowlink[id] == self.order[id] {
                let mut component = Vec::new();

                loop {
                    let member = self.stack.pop().expect("the visited node is on the stack");
                    self.on_stack.remove(member);
                    component.push(member.to_string());

                    if member == id {
                        break;
                    }
                }

                self.components.push(component);
            }
        }
    }

    let mut successors: HashMap<&str, Vec<&str>> = node_ids.iter().map(|id| (id.as_str(), Vec::new())).collect();

    for edge in edges {
        if let Some(next) = successors.get_mut(edge.from.as_str()) {
            next.push(&edge.to);
        }
    }

    let mut search = Search {
        successors,
        order: HashMap::new(),
        lowlink: HashMap::new(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        components: Vec::new(),
    };

    for id in node_ids {
        if !search.order.contains_key(id.as_str()) {
            search.visit(id);
        }
    }

    search.components
}

fn has_cycle(node_ids: &[String], edges: &[ModuleEdge]) -> bool {
    strongly_connected(node_ids, edges).iter().any(|component| component.len() > 1)
}

fn lightest_first(a: &ModuleEdge, b: &ModuleEdge) -> std::cmp::Ordering {
    (a.weight, &a.from, &a.to).cmp(&(b.weight, &b.from, &b.to))
}

fn module_cycles<'a>(
    graph: &Parts<'a>,
    classes: &HashMap<&str, &'a ClassNode>,
    modules: &ModuleGraph<'_>,
    violating: &HashSet<usize>,
) -> Vec<Cycle> {
    let node_ids: Vec<String> = modules.nodes.iter().map(|(id, _)| id.clone()).collect();
    let homes = homes(graph, classes);
    let home_of = |class: &ClassNode| homes.get(class.id.as_str()).map_or_else(|| module_of(class), |home| home.module.clone());
    let mut cycles = Vec::new();

    for component in strongly_connected(&node_ids, &modules.edges) {
        if component.len() < 2 {
            continue;
        }

        let members: HashSet<&str> = component.iter().map(String::as_str).collect();
        let mut sorted = component.clone();
        sorted.sort();

        let mut folds: Vec<Fold> = graph
            .classes
            .iter()
            .filter(|class| !is_vendor(class) && members.contains(module_of(class).as_str()))
            .filter_map(|class| {
                let home = homes.get(class.id.as_str())?;

                Some(Fold {
                    class: class.id.clone(),
                    kind: if class.kind == "class" { "abstract class" } else { class.kind },
                    module: module_of(class),
                    into: home.module.clone(),
                    users: home.users,
                })
            })
            .collect();

        folds.sort_by(|a, b| (&a.into, &a.module, &a.class).cmp(&(&b.into, &b.module, &b.class)));

        let mut folded: BTreeMap<(String, String), ModuleEdge> = BTreeMap::new();

        for (index, edge) in graph.edges.iter().enumerate() {
            let (from, to) = (classes[edge.from.as_str()], classes[edge.to.as_str()]);

            if is_vendor(from) || is_vendor(to) {
                continue;
            }

            let (from_module, to_module) = (home_of(from), home_of(to));

            if from_module != to_module && members.contains(from_module.as_str()) && members.contains(to_module.as_str()) {
                aggregate(&mut folded, from_module, to_module, index, edge.kind, violating);
            }
        }

        let folded: Vec<ModuleEdge> = folded.into_values().collect();
        let mut remaining: Vec<String> = strongly_connected(&sorted, &folded)
            .into_iter()
            .filter(|component| component.len() > 1)
            .flatten()
            .collect();
        remaining.sort();

        if remaining.is_empty() {
            continue;
        }

        let still: HashSet<&str> = remaining.iter().map(String::as_str).collect();
        let mut edges: Vec<ModuleEdge> =
            folded.into_iter().filter(|edge| still.contains(edge.from.as_str()) && still.contains(edge.to.as_str())).collect();

        let exact = remaining.len() <= EXACT_CUT_LIMIT;
        let cut = if exact { exact_cut(&remaining, &edges) } else { pruned_greedy_cut(&remaining, &edges) };

        let files: HashSet<&str> = cut
            .iter()
            .flat_map(|edge| &edge.edges)
            .flat_map(|index| &graph.edges[*index].evidence)
            .map(|place| place.file.as_str())
            .collect();
        let cost = Cost {
            class_edges: cut.iter().map(|edge| edge.weight).sum(),
            files: files.len(),
            violations: cut.iter().map(|edge| edge.violations).sum(),
        };

        edges.sort_by(lightest_first);

        for edge in &mut edges {
            edge.edges.clear();
        }

        cycles.push(Cycle { modules: sorted, folds, remaining, edges, exact, cut, cost });
    }

    cycles.sort_by(|a, b| b.modules.len().cmp(&a.modules.len()).then_with(|| a.modules[0].cmp(&b.modules[0])));

    cycles
}

/// Orders the modules so that the edges pointing backwards weigh the least (dynamic programming over the
/// subsets of modules already placed); the backward edges are the cut. Among cuts of the same weight, the
/// one with fewer edges wins: one place to change rather than two.
fn exact_cut(node_ids: &[String], edges: &[ModuleEdge]) -> Vec<ModuleEdge> {
    let count = node_ids.len();
    let position: HashMap<&str, usize> = node_ids.iter().enumerate().map(|(index, id)| (id.as_str(), index)).collect();
    let scale = edges.len() as u64 + 1;
    let mut weight = vec![vec![0u64; count]; count];

    for edge in edges {
        weight[position[edge.from.as_str()]][position[edge.to.as_str()]] += edge.weight * scale + 1;
    }

    let mut cost = vec![u64::MAX; 1 << count];
    let mut last = vec![usize::MAX; 1 << count];
    cost[0] = 0;

    for placed in 0..cost.len() {
        if cost[placed] == u64::MAX {
            continue;
        }

        for next in (0..count).filter(|next| placed & (1 << next) == 0) {
            let backward: u64 = (0..count).filter(|earlier| placed & (1 << earlier) != 0).map(|earlier| weight[next][earlier]).sum();
            let extended = placed | (1 << next);

            if cost[placed] + backward < cost[extended] {
                cost[extended] = cost[placed] + backward;
                last[extended] = next;
            }
        }
    }

    let mut rank = vec![0; count];
    let mut placed = cost.len() - 1;

    for order in (1..=count).rev() {
        rank[last[placed]] = order;
        placed &= !(1 << last[placed]);
    }

    let mut cut: Vec<ModuleEdge> = edges
        .iter()
        .filter(|edge| rank[position[edge.from.as_str()]] > rank[position[edge.to.as_str()]])
        .cloned()
        .collect();

    cut.sort_by(lightest_first);

    cut
}

/// Removes the lightest edges, one at a time, until the component falls apart, then puts back every edge
/// that turns out not to be needed. Not the minimum cut, but a short list of places to start.
fn pruned_greedy_cut(node_ids: &[String], edges: &[ModuleEdge]) -> Vec<ModuleEdge> {
    let mut remaining = edges.to_vec();
    remaining.sort_by(lightest_first);
    let mut removed: Vec<ModuleEdge> = Vec::new();

    loop {
        let in_cycle: HashSet<String> = strongly_connected(node_ids, &remaining)
            .into_iter()
            .filter(|component| component.len() > 1)
            .flatten()
            .collect();

        if in_cycle.is_empty() {
            break;
        }

        let position = remaining
            .iter()
            .position(|edge| in_cycle.contains(&edge.from) && in_cycle.contains(&edge.to))
            .expect("a cycle has an edge inside it");

        removed.push(remaining.remove(position));
    }

    for index in (0..removed.len()).rev() {
        let mut restored = remaining.clone();
        restored.push(removed[index].clone());

        if !has_cycle(node_ids, &restored) {
            remaining = restored;
            removed.remove(index);
        }
    }

    removed.sort_by(lightest_first);

    removed
}

fn class_fan_out<'a>(graph: &Parts<'a>, classes: &HashMap<&str, &ClassNode>) -> Vec<(&'a str, usize)> {
    let mut outgoing: HashMap<&str, BTreeSet<&str>> = HashMap::new();

    for edge in graph.edges {
        let (from, to) = (classes[edge.from.as_str()], classes[edge.to.as_str()]);

        if !is_vendor(to) && module_of(to) != module_of(from) {
            outgoing.entry(&edge.from).or_default().insert(&edge.to);
        }
    }

    graph
        .classes
        .iter()
        .filter(|class| !is_vendor(class))
        .map(|class| (class.id.as_str(), outgoing.get(class.id.as_str()).map_or(0, BTreeSet::len)))
        .collect()
}

fn module_metrics(
    graph: &Parts<'_>,
    classes: &HashMap<&str, &ClassNode>,
    modules: &ModuleGraph<'_>,
    cycles: &[Cycle],
    fan_out: &[(&str, usize)],
) -> Vec<ModuleMetrics> {
    let in_cycle: HashMap<&str, usize> = cycles
        .iter()
        .enumerate()
        .flat_map(|(position, cycle)| cycle.remaining.iter().map(move |id| (id.as_str(), position + 1)))
        .collect();
    let fan_out: HashMap<&str, usize> = fan_out.iter().copied().collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut incoming: HashMap<&str, Vec<&str>> = HashMap::new();

    for edge in graph.edges {
        outgoing.entry(&edge.from).or_default().push(&edge.to);
        incoming.entry(&edge.to).or_default().push(&edge.from);
    }

    let mut metrics = Vec::new();

    for (id, members) in &modules.nodes {
        let members: Vec<&ClassNode> = members.iter().map(|member| classes[member]).collect();
        let abstract_count = members.iter().filter(|class| class.r#abstract || class.kind == "interface").count();
        let mut afferent = HashSet::new();
        let mut efferent = HashSet::new();
        let mut vendor = HashSet::new();
        let mut busiest: Option<ClassFanOut> = None;

        for class in &members {
            for &target in outgoing.get(class.id.as_str()).into_iter().flatten() {
                let target = classes[target];

                if is_vendor(target) {
                    vendor.insert(&target.id);
                } else if &module_of(target) != id {
                    efferent.insert(&target.id);
                }
            }

            for &source in incoming.get(class.id.as_str()).into_iter().flatten() {
                let source = classes[source];

                if !is_vendor(source) && &module_of(source) != id {
                    afferent.insert(&source.id);
                }
            }

            let out = fan_out.get(class.id.as_str()).copied().unwrap_or(0);

            if out > 0 && busiest.as_ref().is_none_or(|busiest| out > busiest.fan_out) {
                busiest = Some(ClassFanOut { id: class.id.clone(), fan_out: out });
            }
        }

        let (ca, ce) = (afferent.len(), efferent.len());
        let instability = if ca + ce == 0 { None } else { Some(ce as f64 / (ca + ce) as f64) };
        let abstractness = if members.is_empty() { 0.0 } else { abstract_count as f64 / members.len() as f64 };

        metrics.push(ModuleMetrics {
            id: id.clone(),
            classes: members.len(),
            abstractness,
            afferent: ca,
            efferent: ce,
            instability,
            distance: instability.map(|instability| (abstractness + instability - 1.0).abs()),
            vendor: vendor.len(),
            cycle: in_cycle.get(id.as_str()).copied(),
            busiest,
        });
    }

    metrics
}

/// Classes whose fan-out is at least the mean plus two standard deviations, and at least 5.
fn fan_out_outliers(fan_out: &[(&str, usize)]) -> Outliers {
    let fanning: Vec<(&str, usize)> = fan_out.iter().copied().filter(|(_, count)| *count > 0).collect();

    if fanning.len() < 4 {
        return Outliers::default();
    }

    let count = fanning.len() as f64;
    let mean = fanning.iter().map(|(_, out)| *out as f64).sum::<f64>() / count;
    let deviation = (fanning.iter().map(|(_, out)| (*out as f64 - mean).powi(2)).sum::<f64>() / count).sqrt();
    let threshold = (mean + 2.0 * deviation).max(5.0);

    let mut classes: Vec<ClassFanOut> = fanning
        .into_iter()
        .filter(|(_, out)| *out as f64 >= threshold)
        .map(|(id, out)| ClassFanOut { id: id.to_string(), fan_out: out })
        .collect();

    classes.sort_by(|a, b| b.fan_out.cmp(&a.fan_out).then_with(|| a.id.cmp(&b.id)));

    Outliers { threshold: Some(threshold), classes }
}

/// How many classes each root\segment holds, and the layer it is placed in.
fn layers(graph: &Parts<'_>) -> Vec<LayerEntry> {
    let mut layers: BTreeMap<String, LayerEntry> = BTreeMap::new();

    for class in graph.classes {
        let place = place_of(class);

        if place.layer == "vendor" {
            continue;
        }

        let namespace = [class.root.as_deref().unwrap_or(""), place.segment.unwrap_or("")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\\");

        layers.entry(namespace.clone()).or_insert(LayerEntry { namespace, layer: place.layer, classes: 0 }).classes += 1;
    }

    layers.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::Evidence;

    fn declare(id: &str, root: &str) -> ClassNode {
        let mut class = ClassNode::external(id, "app");
        class.root = Some(root.to_string());
        class.external = false;
        class
    }

    fn vendor(id: &str) -> ClassNode {
        ClassNode::external(id, "vendor/pkg")
    }

    fn reference(from: &str, to: &str, line: u32) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            kind: EdgeKind::Reference,
            label: None,
            evidence: vec![Evidence { file: "app.php".to_string(), line }],
        }
    }

    fn laravel() -> Vec<Package> {
        vec![
            Package { name: "app".to_string(), kind: PackageKind::App },
            Package { name: "laravel/framework".to_string(), kind: PackageKind::Vendor },
        ]
    }

    #[test]
    fn layers_come_from_the_first_segment_under_the_psr4_root() {
        let layer = |class: ClassNode| place_of(&class).layer;

        assert_eq!(layer(declare("App\\Models\\User", "App")), "core");
        assert_eq!(layer(declare("App\\Crawler", "App")), "core");
        assert_eq!(layer(declare("App\\Http\\Controllers\\UserController", "App")), "http");
        assert_eq!(layer(declare("App\\Livewire\\Profile", "App")), "http");
        assert_eq!(layer(declare("App\\Console\\Commands\\Prune", "App")), "console");
        assert_eq!(layer(declare("App\\Providers\\AppServiceProvider", "App")), "providers");
        assert_eq!(layer(declare("Database\\Seeders\\UserSeeder", "Database\\Seeders")), "database");
        assert_eq!(layer(declare("Tests\\Feature\\UserTest", "Tests")), "tests");
        assert_eq!(layer(vendor("Illuminate\\Http\\Request")), "vendor");
    }

    #[test]
    fn rules_apply_between_app_layers_only() {
        let classes: HashMap<String, ClassNode> = [
            declare("App\\Models\\User", "App"),
            declare("App\\Http\\Controllers\\UserController", "App"),
            declare("App\\Livewire\\Profile", "App"),
            declare("App\\Console\\Commands\\Prune", "App"),
            declare("App\\Providers\\AppServiceProvider", "App"),
            declare("App\\Events\\Registered", "App"),
            declare("App\\Listeners\\SendWelcome", "App"),
            declare("App\\Crawler", "App"),
            declare("Database\\Seeders\\UserSeeder", "Database\\Seeders"),
            declare("Tests\\Feature\\UserTest", "Tests"),
            vendor("Illuminate\\Http\\Request"),
        ]
        .into_iter()
        .map(|class| (class.id.clone(), class))
        .collect();

        let broken = |from: &str, to: &str| {
            rule_between(&place_of(&classes[from]), &place_of(&classes[to])).map(|rule| rule.id)
        };

        assert_eq!(broken("App\\Models\\User", "App\\Http\\Controllers\\UserController"), Some("core-independent-of-http"));
        assert_eq!(broken("App\\Models\\User", "App\\Livewire\\Profile"), Some("core-independent-of-http"));
        assert_eq!(broken("App\\Crawler", "App\\Console\\Commands\\Prune"), Some("core-independent-of-console"));
        assert_eq!(
            broken("App\\Http\\Controllers\\UserController", "App\\Console\\Commands\\Prune"),
            Some("http-independent-of-console")
        );
        assert_eq!(broken("App\\Console\\Commands\\Prune", "App\\Livewire\\Profile"), Some("console-independent-of-http"));
        assert_eq!(broken("App\\Events\\Registered", "App\\Listeners\\SendWelcome"), Some("events-independent-of-listeners"));

        assert_eq!(broken("App\\Listeners\\SendWelcome", "App\\Events\\Registered"), None);
        assert_eq!(broken("App\\Http\\Controllers\\UserController", "App\\Models\\User"), None);
        assert_eq!(broken("App\\Http\\Controllers\\UserController", "App\\Providers\\AppServiceProvider"), None);
        assert_eq!(broken("App\\Providers\\AppServiceProvider", "App\\Http\\Controllers\\UserController"), None);
        assert_eq!(broken("Database\\Seeders\\UserSeeder", "App\\Http\\Controllers\\UserController"), None);
        assert_eq!(broken("Tests\\Feature\\UserTest", "App\\Console\\Commands\\Prune"), None);
        assert_eq!(broken("App\\Models\\User", "Illuminate\\Http\\Request"), None);
    }

    #[test]
    fn rules_are_only_checked_in_laravel_applications() {
        let classes = vec![declare("App\\Models\\User", "App"), declare("App\\Http\\Controllers\\UserController", "App")];
        let edges = vec![reference("App\\Models\\User", "App\\Http\\Controllers\\UserController", 1)];

        assert_eq!(Verdict::of(&laravel(), &classes, &edges).violations.len(), 1);
        assert!(Verdict::of(&laravel()[..1], &classes, &edges).violations.is_empty());
    }

    #[test]
    fn the_cheapest_cut_is_the_minimum_not_the_first_lightest_edges_that_happen_to_work() {
        // The shape of the smaller laravel.io cycle: removing the lightest edges one by one costs 12,
        // removing the single edge Jobs -> Http costs 7.
        let weighted = [
            ("App\\Actions", "App\\Jobs", 1),
            ("App\\Http", "App\\Actions", 2),
            ("App\\Http", "App\\Providers", 4),
            ("App\\Filament", "App\\Jobs", 6),
            ("App\\Providers", "App\\Filament", 6),
            ("App\\Jobs", "App\\Http", 7),
            ("App\\Http", "App\\Jobs", 36),
        ];
        let modules: BTreeSet<&str> = weighted.iter().flat_map(|(from, to, _)| [*from, *to]).collect();
        let classes: Vec<ClassNode> = modules.iter().map(|module| declare(&format!("{module}\\A"), "App")).collect();
        let edges: Vec<Edge> = weighted
            .iter()
            .flat_map(|(from, to, weight)| (1..=*weight).map(move |line| reference(&format!("{from}\\A"), &format!("{to}\\A"), line)))
            .collect();

        let verdict = Verdict::of(&laravel(), &classes, &edges);
        let cycle = &verdict.cycles[0];

        assert_eq!(verdict.cycles.len(), 1);
        assert!(cycle.exact);
        assert_eq!(
            cycle.cut.iter().map(|edge| format!("{} -> {}", edge.from, edge.to)).collect::<Vec<_>>(),
            ["App\\Jobs -> App\\Http"]
        );
    }

    fn structural(from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge { kind, ..reference(from, to, 1) }
    }

    fn cut_of(cycle: &Cycle) -> Vec<String> {
        cycle.cut.iter().map(|edge| format!("{} -> {} ({})", edge.from, edge.to, edge.weight)).collect()
    }

    #[test]
    fn a_trait_used_by_one_module_only_is_counted_there_and_its_edges_back_are_not_a_cycle() {
        let mut thread = declare("App\\Models\\Thread", "App");
        thread.kind = "class";
        let mut has_author = declare("App\\Concerns\\HasAuthor", "App");
        has_author.kind = "trait";
        let classes = vec![thread, has_author, declare("App\\Models\\User", "App")];
        let edges = vec![
            structural("App\\Models\\Thread", "App\\Concerns\\HasAuthor", EdgeKind::UsesTrait),
            reference("App\\Concerns\\HasAuthor", "App\\Models\\User", 5),
        ];

        let verdict = Verdict::of(&laravel(), &classes, &edges);

        assert!(verdict.cycles.is_empty());
        assert!(verdict.modules.iter().all(|module| module.cycle.is_none()));
    }

    #[test]
    fn a_cycle_that_survives_the_folding_names_the_folds_and_the_modules_still_in_it() {
        let mut has_author = declare("App\\Concerns\\HasAuthor", "App");
        has_author.kind = "trait";
        let classes = vec![
            declare("App\\Models\\Thread", "App"),
            declare("App\\Models\\User", "App"),
            has_author,
            declare("App\\Policies\\UserPolicy", "App"),
        ];
        let edges = vec![
            structural("App\\Models\\Thread", "App\\Concerns\\HasAuthor", EdgeKind::UsesTrait),
            reference("App\\Concerns\\HasAuthor", "App\\Models\\User", 5),
            reference("App\\Models\\User", "App\\Policies\\UserPolicy", 6),
            reference("App\\Policies\\UserPolicy", "App\\Models\\User", 7),
        ];

        let verdict = Verdict::of(&laravel(), &classes, &edges);
        let cycle = &verdict.cycles[0];

        assert_eq!(cycle.modules, ["App\\Concerns", "App\\Models", "App\\Policies"]);
        assert_eq!(cycle.folds.len(), 1);
        assert_eq!((cycle.folds[0].class.as_str(), cycle.folds[0].kind, cycle.folds[0].into.as_str(), cycle.folds[0].users), ("App\\Concerns\\HasAuthor", "trait", "App\\Models", 1));
        assert_eq!(cycle.remaining, ["App\\Models", "App\\Policies"]);
        assert_eq!(cycle.cut.len(), 1);
        assert_eq!(cycle.cost.class_edges, 1);
        let concerns = verdict.modules.iter().find(|module| module.id == "App\\Concerns").unwrap();
        assert_eq!(concerns.cycle, None);
    }

    #[test]
    fn a_trait_used_by_two_modules_stays_where_it_is() {
        let mut has_author = declare("App\\Concerns\\HasAuthor", "App");
        has_author.kind = "trait";
        let classes = vec![declare("App\\Models\\Thread", "App"), declare("App\\Jobs\\CreateThread", "App"), has_author];
        let edges = vec![
            structural("App\\Models\\Thread", "App\\Concerns\\HasAuthor", EdgeKind::UsesTrait),
            structural("App\\Jobs\\CreateThread", "App\\Concerns\\HasAuthor", EdgeKind::UsesTrait),
            reference("App\\Concerns\\HasAuthor", "App\\Models\\Thread", 5),
        ];

        let verdict = Verdict::of(&laravel(), &classes, &edges);
        let cycle = &verdict.cycles[0];

        assert!(cycle.folds.is_empty());
        assert_eq!(cycle.remaining, ["App\\Concerns", "App\\Models"]);
        assert_eq!(cycle.cut.len(), 1);
        assert_eq!(cycle.cost.class_edges, 1);
    }

    #[test]
    fn among_cuts_of_the_same_weight_the_one_with_fewer_edges_wins() {
        // The shape of the laravel.io cycle among Models, Enums and Notifications once the model traits
        // are counted with the models: Enums -> Notifications (2) costs the same as Models -> Enums (1)
        // plus Notifications -> Enums (1), but is one place to change.
        let weighted = [
            ("App\\Models", "App\\Enums", 1),
            ("App\\Notifications", "App\\Enums", 1),
            ("App\\Enums", "App\\Notifications", 2),
            ("App\\Notifications", "App\\Models", 19),
        ];
        let modules: BTreeSet<&str> = weighted.iter().flat_map(|(from, to, _)| [*from, *to]).collect();
        let classes: Vec<ClassNode> = modules.iter().map(|module| declare(&format!("{module}\\A"), "App")).collect();
        let edges: Vec<Edge> = weighted
            .iter()
            .flat_map(|(from, to, weight)| (1..=*weight).map(move |line| reference(&format!("{from}\\A"), &format!("{to}\\A"), line)))
            .collect();

        let verdict = Verdict::of(&laravel(), &classes, &edges);
        let cycle = &verdict.cycles[0];

        assert_eq!(cut_of(cycle), ["App\\Enums -> App\\Notifications (2)"]);
        assert_eq!(cycle.cut[0].edges.len(), 2);
        assert_eq!(cycle.cut[0].edges.iter().map(|index| edges[*index].to.as_str()).collect::<Vec<_>>(), ["App\\Notifications\\A"; 2]);
        assert_eq!((cycle.cost.class_edges, cycle.cost.files, cycle.cost.violations), (2, 1, 0));
    }

    #[test]
    fn the_cost_of_a_cut_counts_the_rule_violations_it_fixes() {
        let classes = vec![declare("App\\Jobs\\CreateThread", "App"), declare("App\\Http\\Requests\\ThreadRequest", "App")];
        let edges = vec![
            reference("App\\Jobs\\CreateThread", "App\\Http\\Requests\\ThreadRequest", 1),
            reference("App\\Http\\Requests\\ThreadRequest", "App\\Jobs\\CreateThread", 2),
            reference("App\\Http\\Requests\\ThreadRequest", "App\\Jobs\\CreateThread", 3),
        ];

        let verdict = Verdict::of(&laravel(), &classes, &edges);
        let cycle = &verdict.cycles[0];

        assert_eq!(cut_of(cycle), ["App\\Jobs -> App\\Http (1)"]);
        assert_eq!(cycle.cut[0].edges, [0]);
        assert_eq!((cycle.cost.class_edges, cycle.cost.files, cycle.cost.violations), (1, 1, 1));
    }
}
