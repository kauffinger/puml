//! Plain-text architecture reports over a graph, for terminals and agents.
//!
//!   puml-extract report <project-dir>              verdict: rule violations, module cycles, metrics, outliers
//!   puml-extract show <project-dir> <id>           a module, namespace or class: contents, dependencies, violations
//!   puml-extract show <project-dir> <from> <to>    an edge: every class pair behind it with file:line evidence
//!
//! Output is sorted and stable, and every finding ends in a file:line.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;

use crate::graph::ClassNode;
use crate::graph::Edge;
use crate::graph::EdgeKind;
use crate::graph::Graph;
use crate::graph::Method;
use crate::graph::Property;
use crate::verdict::CYCLE_RULE;
use crate::verdict::RULES;
use crate::verdict::RULES_SCOPE;
use crate::verdict::module_of;

struct Index<'a> {
    graph: &'a Graph,
    classes: HashMap<&'a str, &'a ClassNode>,
    module_ids: HashSet<String>,
    violating: HashMap<usize, &'static str>,
    lines: Vec<String>,
}

impl<'a> Index<'a> {
    fn new(graph: &'a Graph) -> Self {
        Index {
            graph,
            classes: graph.classes.iter().map(|class| (class.id.as_str(), class)).collect(),
            module_ids: graph.classes.iter().map(module_of).collect(),
            violating: graph.verdict.violations.iter().map(|violation| (violation.edge, violation.rule)).collect(),
            lines: Vec::new(),
        }
    }

    fn out(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn finish(self) -> String {
        let mut text = self.lines.join("\n");
        text.push('\n');

        text
    }

    fn class(&self, id: &str) -> &'a ClassNode {
        self.classes[id]
    }

    fn rule_broken_by(&self, position: usize) -> Option<&'static str> {
        self.violating.get(&position).copied()
    }

    /// The graph's edges matching a filter, with their positions, which is how violations refer to them.
    fn edges_where(&self, filter: impl Fn(&Edge) -> bool) -> Vec<(usize, &'a Edge)> {
        self.graph.edges.iter().enumerate().filter(|(_, edge)| filter(edge)).collect()
    }
}

pub fn report(graph: &Graph, source: &str) -> String {
    let mut index = Index::new(graph);
    let verdict = &graph.verdict;
    let violation_count = verdict.violations.len();

    let heading = [
        if verdict.rules_apply {
            format!("{violation_count} rule violation{}", plural(violation_count, "s"))
        } else {
            "rules not checked (not a Laravel application)".to_string()
        },
        format!("{} module cycle{}", verdict.cycles.len(), plural(verdict.cycles.len(), "s")),
        format!("{} fan-out outlier{}", verdict.outliers.classes.len(), plural(verdict.outliers.classes.len(), "s")),
    ];

    index.out(format!(
        "puml report: {} ({} classes, {} edges, {})",
        graph.project, graph.stats.classes, graph.stats.edges, graph.generator
    ));
    index.out(format!("verdict: {}", heading.join(", ")));
    index.out("");
    index.out("RULES  (one line per class edge that breaks a rule)");

    if !verdict.rules_apply {
        index.out(format!("  not checked: {RULES_SCOPE}"));
    }

    for rule in RULES {
        let broken: Vec<&Edge> = verdict
            .violations
            .iter()
            .filter(|violation| violation.rule == rule.id)
            .map(|violation| &graph.edges[violation.edge])
            .collect();

        let status = if broken.is_empty() { "ok".to_string() } else { format!("{} violation{}", broken.len(), plural(broken.len(), "s")) };

        index.out(format!("  {}: {status}", rule.id));
        index.out(format!("    {}", rule.why));

        for edge in broken {
            index.out(format!("    {}", edge_line(edge)));
        }
    }

    index.out("");
    index.out(format!("MODULE CYCLES  ({}: {})", CYCLE_RULE.id, CYCLE_RULE.why));

    if verdict.cycles.is_empty() {
        index.out("  none");
    }

    for (position, cycle) in verdict.cycles.iter().enumerate() {
        index.out(format!("  {}. {} modules: {}", position + 1, cycle.modules.len(), cycle.modules.join(", ")));
        index.out("     edges inside the cycle, lightest first (weight = class edges):");

        for edge in &cycle.edges {
            let broken = if edge.violations > 0 {
                format!("  ({} rule violation{})", edge.violations, plural(edge.violations as usize, "s"))
            } else {
                String::new()
            };

            index.out(format!("       {} -> {}  {}  {}{broken}", edge.from, edge.to, edge.weight, describe_kinds(&edge.kinds)));
        }

        let label = if cycle.exact {
            "cheapest cut".to_string()
        } else {
            format!("a cut that breaks it ({} modules is too many to find the cheapest)", cycle.modules.len())
        };

        let cut: Vec<String> = cycle.cut.iter().map(|edge| format!("{} -> {} ({})", edge.from, edge.to, edge.weight)).collect();

        index.out(format!("     {label}: {}", cut.join(", ")));
    }

    index.out("");
    index.out("MODULES  (app modules only; Ca, Ce = classes in other app modules using/used by it; I = Ce/(Ca+Ce);");
    index.out("          A = abstract classes and interfaces / classes; D = |A+I-1|; vendor = vendor classes used)");

    let rows: Vec<Vec<String>> = verdict
        .modules
        .iter()
        .map(|module| {
            vec![
                module.id.clone(),
                module.classes.to_string(),
                module.afferent.to_string(),
                module.efferent.to_string(),
                metric(module.instability),
                fixed(module.abstractness, 2),
                metric(module.distance),
                module.vendor.to_string(),
                module.cycle.map(|cycle| format!("cycle {cycle}")).unwrap_or_default(),
                module.busiest.as_ref().map(|busiest| format!("{} ({})", busiest.id, busiest.fan_out)).unwrap_or_default(),
            ]
        })
        .collect();

    table(
        &mut index,
        &["module", "classes", "Ca", "Ce", "I", "A", "D", "vendor", "cycle", "busiest class (fan-out)"],
        &rows,
    );

    index.out("");

    let threshold = verdict
        .outliers
        .threshold
        .map(|threshold| format!("; threshold mean + 2 sd = {}", fixed(threshold, 1)))
        .unwrap_or_default();

    index.out(format!("FAN-OUT OUTLIERS  (distinct app classes outside the class's own module{threshold})"));

    if verdict.outliers.classes.is_empty() {
        index.out("  none");
    }

    for outlier in &verdict.outliers.classes {
        let class = index.class(&outlier.id);

        index.out(format!("  {:>3}  {}  {}", outlier.fan_out, outlier.id, declared_at(class)));
    }

    index.out("");
    index.out("LAYERS  (first namespace segment under each PSR-4 root decides the layer the rules apply to)");

    for layer in ["http", "console", "providers", "core", "database", "tests"] {
        let namespaces: Vec<String> = verdict
            .layers
            .iter()
            .filter(|entry| entry.layer == layer)
            .map(|entry| format!("{} ({})", entry.namespace, entry.classes))
            .collect();

        if !namespaces.is_empty() {
            index.out(format!("  {layer}: {}", namespaces.join(", ")));
        }
    }

    if !graph.unresolved.is_empty() {
        index.out("");
        index.out(format!(
            "UNRESOLVED  ({} reference{} the extractor could not resolve statically; never guessed)",
            graph.unresolved.len(),
            plural(graph.unresolved.len(), "s")
        ));

        for note in &graph.unresolved {
            index.out(format!("  {}  {}  {}:{}", note.kind, note.detail, note.file, note.line));
        }
    }

    index.out("");
    index.out(format!(
        "drill down: puml-extract show {source} <module|namespace|class>   or   puml-extract show {source} <from> <to>"
    ));

    index.finish()
}

/// Prints a module, namespace or class, or with `to` the edge between two of them. Returns the text and
/// whether anything was found.
pub fn show(graph: &Graph, target: &str, to: Option<&str>) -> (String, bool) {
    let mut index = Index::new(graph);

    let found = match to {
        Some(to) => print_edge(&mut index, target, to),
        None => print_target(&mut index, target),
    };

    (index.finish(), found)
}

fn print_target(index: &mut Index<'_>, id: &str) -> bool {
    if let Some(class) = index.classes.get(id).copied().filter(|class| !class.external) {
        print_class(index, class);

        return true;
    }

    let graph = index.graph;
    let members: Vec<&ClassNode> = graph.classes.iter().filter(|class| !class.external && in_namespace(&class.namespace, id)).collect();

    if members.is_empty() {
        let needle = id.to_lowercase();
        let candidates: Vec<&str> = graph
            .classes
            .iter()
            .filter(|class| !class.external && class.id.to_lowercase().contains(&needle))
            .take(10)
            .map(|class| class.id.as_str())
            .collect();

        index.out(format!("puml: nothing declared at {id}"));

        if !candidates.is_empty() {
            index.out(format!("did you mean: {}", candidates.join(", ")));
        }

        return false;
    }

    let is_module = index.module_ids.contains(id);
    let metrics = if is_module { graph.verdict.modules.iter().find(|module| module.id == id) } else { None };

    index.out(format!(
        "{} {id}: {} class{}, layer {}",
        if is_module { "module" } else { "namespace" },
        members.len(),
        plural(members.len(), "es"),
        members[0].layer
    ));

    if let Some(metrics) = metrics {
        let cycle = metrics.cycle.map(|cycle| format!(", in module cycle {cycle}")).unwrap_or_default();

        index.out(format!(
            "metrics: Ca {}, Ce {}, I {}, A {}, D {}, vendor classes used {}{cycle}",
            metrics.afferent,
            metrics.efferent,
            metric(metrics.instability),
            fixed(metrics.abstractness, 2),
            metric(metrics.distance),
            metrics.vendor
        ));
    }

    index.out("");
    index.out("CONTENTS");

    let mut children: BTreeMap<String, usize> = BTreeMap::new();

    for member in &members {
        let key = if member.namespace == id {
            member.id.clone()
        } else {
            let child = member.namespace[id.len() + 1..].split('\\').next().unwrap_or("");

            format!("{id}\\{child}\\")
        };

        *children.entry(key).or_default() += 1;
    }

    for (key, count) in children {
        let line = match index.classes.get(key.as_str()) {
            Some(member) => format!("  {:<9} {key}  {}", member.kind, declared_at(member)),
            None => format!("  namespace {key}  ({count} class{})", plural(count, "es")),
        };

        index.out(line);
    }

    let inside: HashSet<&str> = members.iter().map(|member| member.id.as_str()).collect();

    dependency_lists(index, &inside, id);

    true
}

fn print_class(index: &mut Index<'_>, class: &ClassNode) {
    let fan_out = index.graph.verdict.fan_out.get(&class.id).copied().unwrap_or(0);

    index.out(format!(
        "{}{}{} {}  {}",
        if class.r#abstract { "abstract " } else { "" },
        if class.r#final { "final " } else { "" },
        class.kind,
        class.id,
        declared_at(class)
    ));
    index.out(format!(
        "module {}, layer {}, fan-out {fan_out} app class{} outside its module",
        module_of(class),
        class.layer,
        plural(fan_out, "es")
    ));

    let file = class.file.as_deref().unwrap_or("");

    let sections: [(&str, Vec<String>); 8] = [
        ("EXTENDS", class.parent.iter().cloned().collect()),
        ("IMPLEMENTS", class.interfaces.clone()),
        ("USES", class.traits.clone()),
        (
            "RELATIONS",
            class
                .relations
                .iter()
                .map(|relation| {
                    let related = relation.related.as_deref().unwrap_or("(runtime)");

                    format!("{}  {}  {related}  {file}:{}", relation.name, relation.r#type, relation.line)
                })
                .collect(),
        ),
        ("CASES", class.cases.clone()),
        ("CONSTANTS", class.constants.clone()),
        ("PROPERTIES", class.properties.iter().map(format_property).collect()),
        ("METHODS", class.methods.iter().map(|method| format!("{}  {file}:{}", format_method(method), method.line)).collect()),
    ];

    for (title, items) in sections {
        if items.is_empty() {
            continue;
        }

        index.out("");
        index.out(title);

        for item in items {
            index.out(format!("  {item}"));
        }
    }

    dependency_lists(index, &HashSet::from([class.id.as_str()]), &class.id);
}

fn dependency_lists(index: &mut Index<'_>, inside: &HashSet<&str>, id: &str) {
    let outgoing = index.edges_where(|edge| inside.contains(edge.from.as_str()) && !inside.contains(edge.to.as_str()));
    let incoming = index.edges_where(|edge| inside.contains(edge.to.as_str()) && !inside.contains(edge.from.as_str()));

    index.out("");
    index.out(format!(
        "DEPENDS ON  ({} class edge{} leaving {id}, grouped by target)",
        outgoing.len(),
        plural(outgoing.len(), "s")
    ));
    grouped_edges(index, &outgoing, |edge| &edge.to);
    index.out("");
    index.out(format!(
        "USED BY  ({} class edge{} entering {id}, grouped by source)",
        incoming.len(),
        plural(incoming.len(), "s")
    ));
    grouped_edges(index, &incoming, |edge| &edge.from);

    let mut broken: Vec<(&Edge, &str)> = outgoing
        .iter()
        .chain(&incoming)
        .filter_map(|(position, edge)| index.rule_broken_by(*position).map(|rule| (*edge, rule)))
        .collect();

    broken.sort_by(|(a, _), (b, _)| (&a.from, &a.to).cmp(&(&b.from, &b.to)));

    let count = if broken.is_empty() { "none".to_string() } else { broken.len().to_string() };

    index.out("");
    index.out(format!("RULE VIOLATIONS TOUCHING {id}: {count}"));

    for (edge, rule) in broken {
        index.out(format!("  {rule}  {}", edge_line(edge)));
    }
}

fn grouped_edges(index: &mut Index<'_>, edges: &[(usize, &Edge)], other_side: fn(&Edge) -> &String) {
    if edges.is_empty() {
        index.out("  none");

        return;
    }

    let mut groups: BTreeMap<String, Vec<&Edge>> = BTreeMap::new();

    for (_, edge) in edges {
        groups.entry(module_of(index.class(other_side(edge)))).or_default().push(edge);
    }

    let mut ordered: Vec<(String, Vec<&Edge>)> = groups.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

    for (key, mut group) in ordered {
        index.out(format!("  {key}  {}", group.len()));

        group.sort_by(|a, b| (&a.from, &a.to, a.kind.name()).cmp(&(&b.from, &b.to, b.kind.name())));

        for edge in group {
            index.out(format!("    {}", edge_line(edge)));
        }
    }
}

fn print_edge(index: &mut Index<'_>, from: &str, to: &str) -> bool {
    let inside = |id: &str, prefix: &str| {
        let class = index.class(id);

        if class.external && class.root.is_none() {
            return class.package == prefix;
        }

        id == prefix || in_namespace(&class.namespace, prefix)
    };

    let edges = index.edges_where(|edge| inside(&edge.from, from) && inside(&edge.to, to));

    if edges.is_empty() {
        index.out(format!("puml: no class edges from {from} to {to}"));

        return false;
    }

    let mut pairs: BTreeMap<String, Vec<(usize, &Edge)>> = BTreeMap::new();

    for &(position, edge) in &edges {
        pairs.entry(format!("{} -> {}", edge.from, edge.to)).or_default().push((position, edge));
    }

    index.out(format!(
        "edge {from} -> {to}: {} class edge{} between {} class pair{}",
        edges.len(),
        plural(edges.len(), "s"),
        pairs.len(),
        plural(pairs.len(), "s")
    ));

    let mut ordered: Vec<(String, Vec<(usize, &Edge)>)> = pairs.into_iter().collect();
    ordered.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

    for (key, mut group) in ordered {
        index.out("");
        index.out(format!("{key}  {}", group.len()));

        group.sort_by_key(|(_, edge)| edge.kind.name());

        for (position, edge) in group {
            let via = edge.label.as_ref().map(|label| format!("via {label}  ")).unwrap_or_default();
            let broken = index.rule_broken_by(position).map(|rule| format!("  VIOLATES {rule}")).unwrap_or_default();

            index.out(format!("  {:<11} {via}{}{broken}", edge.kind.name(), evidence(edge)));
        }
    }

    true
}

fn in_namespace(namespace: &str, prefix: &str) -> bool {
    namespace == prefix || namespace.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('\\'))
}

fn edge_line(edge: &Edge) -> String {
    let via = edge.label.as_ref().map(|label| format!("  via {label}")).unwrap_or_default();

    format!("{} -> {}  {}{via}  {}", edge.from, edge.to, edge.kind.name(), evidence(edge))
}

fn evidence(edge: &Edge) -> String {
    edge.evidence.iter().map(|place| format!("{}:{}", place.file, place.line)).collect::<Vec<_>>().join(" ")
}

fn declared_at(class: &ClassNode) -> String {
    format!("{}:{}", class.file.as_deref().unwrap_or_default(), class.line.unwrap_or_default())
}

fn describe_kinds(kinds: &BTreeMap<EdgeKind, u64>) -> String {
    let mut entries: Vec<(&str, u64)> = kinds.iter().map(|(kind, count)| (kind.name(), *count)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));

    entries
        .into_iter()
        .map(|(kind, count)| if count > 1 { format!("{kind} x{count}") } else { kind.to_string() })
        .collect::<Vec<_>>()
        .join(", ")
}

fn table(index: &mut Index<'_>, headers: &[&str], rows: &[Vec<String>]) {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(column, header)| rows.iter().map(|row| row[column].len()).chain([header.len()]).max().unwrap_or(0))
        .collect();

    let line = |cells: Vec<&str>| {
        let padded: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                if (1..=7).contains(&column) {
                    format!("{cell:>width$}", width = widths[column])
                } else {
                    format!("{cell:<width$}", width = widths[column])
                }
            })
            .collect();

        format!("  {}", padded.join("  ")).trim_end().to_string()
    };

    index.out(line(headers.to_vec()));

    for row in rows {
        index.out(line(row.iter().map(String::as_str).collect()));
    }
}

fn visibility(visibility: &str) -> &'static str {
    match visibility {
        "public" => "+",
        "protected" => "#",
        "private" => "-",
        _ => "",
    }
}

fn format_property(property: &Property) -> String {
    let kind = property.r#type.as_ref().map(|kind| format!(": {kind}")).unwrap_or_default();
    let modifiers: Vec<&str> = [property.r#static.then_some("static"), property.readonly.then_some("readonly")].into_iter().flatten().collect();
    let modifiers = if modifiers.is_empty() { String::new() } else { format!(" {}", modifiers.join(" ")) };

    format!("{}{}{kind}{modifiers}", visibility(property.visibility), property.name)
}

fn format_method(method: &Method) -> String {
    let params: Vec<String> = method
        .params
        .iter()
        .map(|param| match &param.r#type {
            Some(kind) => format!("{}: {kind}", param.name),
            None => param.name.clone(),
        })
        .collect();
    let returns = method.returns.as_ref().map(|returns| format!(": {returns}")).unwrap_or_default();

    format!("{}{}({}){returns}", visibility(method.visibility), method.name, params.join(", "))
}

/// Rounds half away from zero like JavaScript's `toFixed`, which the viewer uses for the same numbers.
fn fixed(value: f64, digits: i32) -> String {
    let scale = 10f64.powi(digits);

    format!("{:.*}", digits as usize, (value * scale).round() / scale)
}

fn metric(value: Option<f64>) -> String {
    value.map_or("-".to_string(), |value| fixed(value, 2))
}

fn plural(count: usize, suffix: &str) -> &str {
    if count == 1 { "" } else { suffix }
}
