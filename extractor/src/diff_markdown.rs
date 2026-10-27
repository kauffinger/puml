//! `puml-extract diff --format markdown`, the pull request comment. It opens with a scoreboard and the
//! findings a reviewer has to act on, then draws the classes the pull request changes as a Mermaid graph,
//! which GitHub renders, and folds the rest into `<details>`. A pull request without findings gets one
//! line with everything else folded away, so that a comment on every pull request does not teach
//! reviewers to skip it. With `--repository`, every `file:line` links to its commit on GitHub.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::diff::CycleChange;
use crate::diff::Diff;
use crate::diff::ModuleNumbers;
use crate::diff_report::instability;
use crate::diff_report::short;
use crate::git::Changes;
use crate::graph::Edge;
use crate::report::plural;

/// Lists are cut here. A pull request comment holds at most 65,536 characters.
const LIMIT: usize = 40;
/// A class graph with more classes gives way to the module graph. One with more than `LIMIT` classes is
/// folded: its labels stay readable, but at 90 classes it is some 14,000 pixels tall.
const GRAPH_LIMIT: usize = 100;
const NAMES_PER_LINE: usize = 8;

const GREEN: &str = "#1a7f37";
const RED: &str = "#cf222e";
const PURPLE: &str = "#8250df";
const GREY: &str = "#8c959f";

pub fn render(diff: &Diff, links: Option<&Links>) -> String {
    let mut comment = Comment { diff, links, lines: Vec::new() };
    comment.write();

    comment.lines.join("\n") + "\n"
}

/// Where a `file:line` links to on GitHub: the commit it was read from, in the project directory. The
/// head has no link while it has uncommitted changes, which no commit on GitHub holds.
pub struct Links {
    base: String,
    head: Option<String>,
}

impl Links {
    pub fn new(repository: &str, changes: &Changes) -> Links {
        let blob = |commit: &str| format!("{}/blob/{commit}/{}", repository.trim_end_matches('/'), changes.prefix);

        Links { base: blob(&changes.base_commit), head: (!changes.uncommitted).then(|| blob(&changes.head_commit)) }
    }
}

/// The side a place was read from: removed edges and fixed violations point into the base.
#[derive(Clone, Copy)]
enum At {
    Base,
    Head,
}

struct Comment<'a> {
    diff: &'a Diff,
    links: Option<&'a Links>,
    lines: Vec<String>,
}

/// The class edges between two classes that the pull request adds, removes, or leaves pointing at a
/// class it removes, drawn as one arrow.
#[derive(Default)]
struct Link<'a> {
    kinds: BTreeSet<&'a str>,
    breaks_rule: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Change {
    Added,
    Removed,
    Dangling,
}

impl<'a> Comment<'a> {
    fn out(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn write(&mut self) {
        self.title();

        if !has_findings(self.diff) {
            self.quietly();
            return;
        }

        self.scoreboard();
        self.findings();
        self.everything();
    }

    fn quietly(&mut self) {
        let start = self.lines.len();
        self.details(&format!("<b>Classes</b>: {} · <b>used by</b> {}", or(self.changed_classes(), "none changed"), self.used_by()));
        let opened = self.lines.len();

        self.everything();

        if self.lines.len() == opened {
            self.lines.truncate(start);
        } else {
            self.end_details();
        }
    }

    fn everything(&mut self) {
        for note in &self.diff.notes {
            self.out("");
            self.out(format!("> {note}"));
        }

        if !self.class_graph() {
            self.module_graph();
        }

        self.reading_order();
        self.cycles();
        self.impact();
        self.everything_else();
    }

    fn title(&mut self) {
        let diff = self.diff;

        let verdict = if diff.worsens() {
            "🔴 this pull request makes the architecture worse"
        } else if improves(diff) {
            "🟢 this pull request improves the architecture"
        } else {
            "✅ nothing in the architecture got worse"
        };

        self.out(format!("## puml: {verdict}"));
        self.out("");
        self.out(format!(
            "`{}` ({}) → `{}` ({}{})",
            diff.base.reference,
            short(&diff.base.commit),
            diff.head.reference,
            short(&diff.head.commit),
            if diff.head.uncommitted { " with uncommitted changes" } else { "" }
        ));
    }

    fn scoreboard(&mut self) {
        let diff = self.diff;

        let violations = joined(&[
            counted(diff.violations.added.len(), "🔴", "new"),
            counted(diff.violations.removed.len(), "🟢", "fixed"),
        ]);

        let mut cycles = Vec::new();

        for (status, icon, word) in [("new", "🔴", "new"), ("grown", "🔴", "grew"), ("changed", "🟡", "changed"), ("shrunk", "🟢", "shrank"), ("broken", "🟢", "broken")] {
            cycles.push(counted(diff.cycles.iter().filter(|cycle| cycle.status == status).count(), icon, word));
        }

        let used_by = self.used_by();

        self.out("");
        self.out("| Rule violations | Module cycles | Removed classes in use | Classes | Used by |");
        self.out("|:-:|:-:|:-:|:-:|:-:|");
        self.out(format!(
            "| {} | {} | {} | {} | {used_by} |",
            or(violations, "✅ none new"),
            or(joined(&cycles), "✅ none new"),
            or(counted(diff.dangling.len(), "🔴", ""), "✅ none"),
            or(self.changed_classes(), "none changed"),
        ));
    }

    fn changed_classes(&self) -> String {
        let classes = &self.diff.classes;

        joined(&[
            counted(classes.added.len(), "", "added"),
            counted(classes.modified.len(), "", "modified"),
            counted(classes.renamed.len(), "", "renamed"),
            counted(classes.removed.len(), "", "removed"),
        ])
    }

    fn used_by(&self) -> String {
        let impact = &self.diff.impact;
        let direct: BTreeSet<&String> = impact.classes.iter().flat_map(|entry| &entry.dependents).collect();
        let reach: usize = impact.reach.values().sum();

        if reach == 0 { "none".to_string() } else { format!("{} directly, {reach} in all", direct.len()) }
    }

    /// One line for each thing that got worse or better, so nobody has to read on to know what to fix.
    fn findings(&mut self) {
        let diff = self.diff;
        let mut lines = Vec::new();

        for (icon, title, verb, list, at) in [
            ("🔴", "New rule violation", "breaks", &diff.violations.added, At::Head),
            ("🟢", "Rule violation fixed", "no longer breaks", &diff.violations.removed, At::Base),
        ] {
            for violation in list {
                lines.push(format!(
                    "{icon} **{title}** {} {verb} `{}` · {}",
                    arrow(&violation.edge.from, &violation.edge.to),
                    violation.rule,
                    self.evidence(&violation.edge, at)
                ));
            }
        }

        for cycle in &diff.cycles {
            lines.push(cycle_finding(cycle));
        }

        for dangling in &diff.dangling {
            let users: Vec<String> = dangling.used_by.iter().map(|edge| format!("`{}` · {}", class_name(&edge.from), self.evidence(edge, At::Head))).collect();

            lines.push(format!("🔴 **Removed but still used** `{}` by {}", class_name(&dangling.class), users.join(", ")));
        }

        if lines.is_empty() {
            return;
        }

        self.out("");

        let hidden = lines.len().saturating_sub(LIMIT);

        for line in lines.into_iter().take(LIMIT) {
            self.out(format!("- {line}"));
        }

        if hidden > 0 {
            self.out(format!("- … and {hidden} more; run `puml-extract diff` locally for all of them"));
        }
    }

    /// The classes the pull request adds, modifies, renames and removes, the class edges it adds and
    /// removes between them and other classes, grouped by namespace. False if there are too many classes
    /// to draw.
    fn class_graph(&mut self) -> bool {
        let diff = self.diff;

        let mut status: BTreeMap<&str, &str> = BTreeMap::new();
        let mut renamed_from: HashMap<&str, &str> = HashMap::new();

        for class in &diff.classes.added {
            status.insert(&class.id, "added");
        }

        for modified in &diff.classes.modified {
            status.insert(&modified.class.id, "modified");
        }

        for renamed in &diff.classes.renamed {
            status.insert(&renamed.to.id, "renamed");
            renamed_from.insert(&renamed.to.id, &renamed.from.id);
        }

        for class in &diff.classes.removed {
            status.insert(&class.id, "removed");
        }

        let gone: BTreeSet<&str> =
            diff.classes.removed.iter().chain(diff.classes.renamed.iter().map(|renamed| &renamed.from)).map(|class| class.id.as_str()).collect();

        let mut links: BTreeMap<(&str, &str, Change), Link> = BTreeMap::new();

        for (change, edges) in [(Change::Added, &diff.edges.added), (Change::Removed, &diff.edges.removed)] {
            for edge in edges.iter().filter(|edge| !gone.contains(edge.from.as_str())) {
                let link = links.entry((&edge.from, &edge.to, change)).or_default();
                link.kinds.insert(edge.kind.name());
                link.breaks_rule |= change == Change::Added && self.breaks_rule(edge);
            }
        }

        for dangling in &diff.dangling {
            for edge in &dangling.used_by {
                links.entry((&edge.from, &edge.to, Change::Dangling)).or_default().kinds.insert(edge.kind.name());
            }

            status.entry(&dangling.class).or_insert("removed");
        }

        if status.is_empty() {
            return true;
        }

        let classes: BTreeSet<&str> = status.keys().copied().chain(links.keys().flat_map(|(from, to, _)| [*from, *to])).collect();

        if classes.len() > GRAPH_LIMIT {
            return false;
        }

        let node: HashMap<&str, String> = classes.iter().enumerate().map(|(index, id)| (*id, format!("c{index}"))).collect();
        let mut namespaces: BTreeMap<&str, Vec<&str>> = BTreeMap::new();

        for id in &classes {
            namespaces.entry(namespace(id)).or_default().push(id);
        }

        let folded = classes.len() > LIMIT;

        if folded {
            self.details(&format!("<b>The classes it changes</b>: {} classes", classes.len()));
        } else {
            self.out("");
            self.out("### The classes it changes");
            self.out("");
        }

        self.out("```mermaid");
        self.out("flowchart LR");

        for (index, (namespace, ids)) in namespaces.iter().enumerate() {
            self.out(format!("  subgraph n{index}[\"{}\"]", if namespace.is_empty() { "(global)" } else { namespace }));

            for id in ids {
                let label = match renamed_from.get(id) {
                    Some(from) => format!("{}<br/>was {}", class_name(id), class_name(from)),
                    None => class_name(id).to_string(),
                };

                self.out(format!("    {}[\"{label}\"]", node[id]));
            }

            self.out("  end");
        }

        let mut styles: Vec<(&str, u8)> = Vec::new();

        for ((from, to, change), link) in &links {
            let kinds = link.kinds.iter().copied().collect::<Vec<_>>().join(", ");
            let (arrow, color, width) = match change {
                Change::Added if link.breaks_rule => ("-->", PURPLE, 2),
                Change::Added => ("-->", GREEN, 2),
                Change::Removed => ("-.->", RED, 2),
                Change::Dangling => ("==>", RED, 4),
            };

            self.out(format!("  {} {arrow}|\"{kinds}\"| {}", node[from], node[to]));
            styles.push((color, width));
        }

        for (index, entry) in diff.impact.classes.iter().enumerate() {
            let Some(class) = node.get(entry.class.as_str()) else { continue };
            let label = if entry.api_changed { "|\"API changed\"|" } else { "" };

            self.out(format!(
                "  u{index}([\"{} {} it\"]) -.->{label} {class}",
                entry.dependents.len(),
                if entry.dependents.len() == 1 { "class uses" } else { "classes use" }
            ));
            styles.push((GREY, 2));
        }

        for (index, (color, width)) in styles.iter().enumerate() {
            self.out(format!("  linkStyle {index} stroke:{color},stroke-width:{width}px"));
        }

        let namespaces: Vec<String> = (0..namespaces.len()).map(|index| format!("n{index}")).collect();

        self.out("  classDef namespace fill:#f6f8fa,stroke:#d0d7de,color:#57606a");
        self.out(format!("  class {} namespace", namespaces.join(",")));
        self.out("  classDef unchanged fill:#ffffff,stroke:#8c959f,color:#1f2328");

        self.out("  classDef added fill:#dafbe1,stroke:#1a7f37,color:#1f2328");
        self.out("  classDef modified fill:#fff8c5,stroke:#9a6700,color:#1f2328");
        self.out("  classDef renamed fill:#ddf4ff,stroke:#0969da,color:#1f2328");
        self.out("  classDef removed fill:#ffebe9,stroke:#cf222e,stroke-dasharray:4 3,color:#1f2328");
        self.out("  classDef users fill:none,stroke:#8c959f,stroke-dasharray:2 2");

        for kind in ["unchanged", "added", "modified", "renamed", "removed"] {
            let ids: Vec<&str> = classes.iter().filter(|id| status.get(*id).copied().unwrap_or("unchanged") == kind).map(|id| node[id].as_str()).collect();

            if !ids.is_empty() {
                self.out(format!("  class {} {kind}", ids.join(",")));
            }
        }

        if !diff.impact.classes.is_empty() {
            let users: Vec<String> = (0..diff.impact.classes.len()).map(|index| format!("u{index}")).collect();
            self.out(format!("  class {} users", users.join(",")));
        }

        self.out("```");
        self.out("");
        self.out(
            "<sub>Boxes: green added, yellow modified, blue renamed, red removed. Arrows: green new dependency, purple breaks a rule, \
             red dashed dependency removed, thick red uses a removed class.</sub>",
        );

        if folded {
            self.end_details();
        }

        true
    }

    /// The module edges whose weight changed, for pull requests too large to draw class by class.
    fn module_graph(&mut self) {
        let modules = &self.diff.modules;

        if modules.is_empty() {
            return;
        }

        let shown = &modules[..modules.len().min(LIMIT)];
        let ids: BTreeSet<&str> = shown.iter().flat_map(|change| [change.from.as_str(), change.to.as_str()]).collect();
        let node: HashMap<&str, String> = ids.iter().enumerate().map(|(index, id)| (*id, format!("m{index}"))).collect();
        let joined: BTreeSet<&str> = self.diff.cycles.iter().flat_map(|cycle| cycle.joined.iter().map(String::as_str)).collect();

        self.out("");
        self.out("### The module edges it changes");
        self.out("");
        self.out("```mermaid");
        self.out("flowchart LR");

        for id in &ids {
            self.out(format!("  {}[\"{id}\"]", node[id]));
        }

        for (position, change) in shown.iter().enumerate() {
            let (arrow, label, color) = match (change.before, change.after) {
                (0, after) => ("-->", format!("new: {after}"), GREEN),
                (before, 0) => ("-.->", format!("gone: was {before}"), RED),
                (before, after) => ("-->", format!("{before} → {after}"), GREY),
            };
            let (label, color) = if change.violations > 0 {
                (format!("{label}, {} breaking a rule", change.violations), PURPLE)
            } else {
                (label, color)
            };

            self.out(format!("  {} {arrow}|\"{label}\"| {}", node[change.from.as_str()], node[change.to.as_str()]));
            self.out(format!("  linkStyle {position} stroke:{color},stroke-width:2px"));
        }

        let all: Vec<&str> = ids.iter().map(|id| node[id].as_str()).collect();
        self.out("  classDef module fill:#ffffff,stroke:#8c959f,color:#1f2328");
        self.out(format!("  class {} module", all.join(",")));

        let cycling: Vec<&str> = ids.iter().filter(|id| joined.contains(*id)).map(|id| node[id].as_str()).collect();

        if !cycling.is_empty() {
            self.out("  classDef cycle fill:#ffffff,stroke:#cf222e,stroke-width:3px,color:#1f2328");
            self.out(format!("  class {} cycle", cycling.join(",")));
        }

        self.out("```");
        self.out("");

        let more = if shown.len() < modules.len() { format!(" {} more module edges changed.", modules.len() - shown.len()) } else { String::new() };

        self.out(format!(
            "<sub>Too many classes to draw one by one. Arrows in class edges: green new, red dashed gone, grey heavier or lighter, \
             purple breaks a rule; a red border marks a module newly in a cycle.{more}</sub>"
        ));
    }

    fn reading_order(&mut self) {
        let diff = self.diff;

        if diff.reading_order.is_empty() {
            return;
        }

        let modified: HashMap<&str, _> = diff.classes.modified.iter().map(|modified| (modified.class.id.as_str(), modified)).collect();
        let renamed: HashMap<&str, &str> = diff.classes.renamed.iter().map(|renamed| (renamed.to.id.as_str(), renamed.from.id.as_str())).collect();
        let places: HashMap<&str, (&str, u32)> = diff
            .classes
            .added
            .iter()
            .chain(diff.classes.modified.iter().map(|modified| &modified.class))
            .chain(diff.classes.renamed.iter().map(|renamed| &renamed.to))
            .map(|class| (class.id.as_str(), (class.file.as_str(), class.line)))
            .collect();

        let total: usize = diff.reading_order.iter().map(Vec::len).sum();

        self.details(&format!("<b>Read in this order</b>: {total} class{}, each after the ones it depends on", plural(total, "es")));

        let mut number = 0;

        'groups: for group in &diff.reading_order {
            if group.len() > 1 {
                self.out("");
                self.out(format!("_The next {} depend on each other; read them together:_", group.len()));
                self.out("");
            }

            for id in group {
                if number == LIMIT {
                    break 'groups;
                }

                number += 1;

                let status = match (modified.contains_key(id.as_str()), renamed.get(id.as_str())) {
                    (true, _) => "modified".to_string(),
                    (_, Some(from)) => format!("renamed from `{from}`"),
                    _ => "added".to_string(),
                };
                let place = places.get(id.as_str()).map(|(file, line)| format!(" · {}", self.place(file, *line, At::Head))).unwrap_or_default();

                self.out(format!("{number}. `{id}` {status}{place}"));

                if let Some(modified) = modified.get(id.as_str()) {
                    for member in &modified.members {
                        let line = match (&member.before, &member.after) {
                            (Some(before), Some(after)) => format!("`{before}` → `{after}`"),
                            (Some(before), None) => format!("removed `{before}`"),
                            (None, Some(after)) => format!("added `{after}`"),
                            (None, None) => continue,
                        };

                        self.out(format!("   - {line}"));
                    }
                }
            }
        }

        if total > number {
            self.out(format!("- … and {} more; run `puml-extract diff` locally for all of them", total - number));
        }

        self.end_details();
    }

    fn cycles(&mut self) {
        let cycles: Vec<&CycleChange> = self.diff.cycles.iter().filter(|cycle| cycle.after.is_some()).collect();

        for cycle in cycles {
            let modules = cycle.after.as_deref().unwrap_or_default();
            let summary = match &cycle.cost {
                Some(cost) => format!(
                    "<b>Module cycle of {} modules</b>: cheapest cut {} class edge{} in {} file{}",
                    modules.len(),
                    cost.class_edges,
                    plural(cost.class_edges as usize, "s"),
                    cost.files,
                    plural(cost.files, "s")
                ),
                None => format!("<b>Module cycle of {} modules</b>", modules.len()),
            };

            self.details(&summary);
            self.out(format!("{}.", modules.iter().map(|module| format!("`{module}`")).collect::<Vec<_>>().join(", ")));

            if !cycle.cut.is_empty() {
                self.out("");
                self.out("| Cut this module edge | Class edges |");
                self.out("|:--|--:|");

                for edge in &cycle.cut {
                    self.out(format!("| `{}` → `{}` | {} |", edge.from, edge.to, edge.weight));
                }
            }

            self.end_details();
        }
    }

    fn impact(&mut self) {
        let impact = &self.diff.impact;

        if impact.classes.is_empty() {
            return;
        }

        let total: usize = impact.reach.values().sum();

        self.details(&format!("<b>Used by</b>: {total} class{} outside this pull request, directly or through others", plural(total, "es")));

        for entry in impact.classes.iter().take(LIMIT) {
            let api = if entry.api_changed { ", whose public API changed," } else { "" };
            let names: Vec<String> = entry.dependents.iter().take(NAMES_PER_LINE).map(|id| format!("`{}`", class_name(id))).collect();
            let rest = entry.dependents.len().saturating_sub(NAMES_PER_LINE);
            let rest = if rest > 0 { format!(" and {rest} more") } else { String::new() };

            self.out(format!(
                "- `{}`{api} is used by {} class{}: {}{rest}",
                entry.class,
                entry.dependents.len(),
                plural(entry.dependents.len(), "es"),
                names.join(", ")
            ));
        }

        let mut modules: Vec<(&String, &usize)> = impact.reach.iter().collect();
        modules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

        self.out("");
        self.out("| Module | Classes that depend on this pull request |");
        self.out("|:--|--:|");

        for (module, count) in modules {
            self.out(format!("| `{module}` | {count} |"));
        }

        self.end_details();
    }

    fn everything_else(&mut self) {
        let diff = self.diff;
        let other_edges = self.other_edges();

        if diff.modules.is_empty() && diff.metrics.is_empty() && diff.outliers.added.is_empty() && diff.outliers.removed.is_empty() && other_edges.is_empty() && diff.other_files.is_empty() {
            return;
        }

        self.details("Module edges, metrics and other files");

        if !diff.modules.is_empty() {
            self.out("| Module edge | Class edges | Breaking a rule |");
            self.out("|:--|:-:|:-:|");

            for change in &diff.modules {
                self.out(format!("| `{}` → `{}` | {} → {} | {} |", change.from, change.to, change.before, change.after, change.violations));
            }

            self.out("");
        }

        if !diff.metrics.is_empty() {
            self.out("| Module | Classes | Ca | Ce | I |");
            self.out("|:--|:-:|:-:|:-:|:-:|");

            for change in &diff.metrics {
                let (before, after) = (change.before.as_ref(), change.after.as_ref());
                let cell = |value: fn(&ModuleNumbers) -> String| match (before.map(value), after.map(value)) {
                    (Some(before), Some(after)) if before == after => after,
                    (Some(before), Some(after)) => format!("{before} → {after}"),
                    (None, Some(after)) => format!("new: {after}"),
                    (Some(before), None) => format!("{before} → gone"),
                    (None, None) => String::new(),
                };

                self.out(format!(
                    "| `{}` | {} | {} | {} | {} |",
                    change.module,
                    cell(|numbers| numbers.classes.to_string()),
                    cell(|numbers| numbers.afferent.to_string()),
                    cell(|numbers| numbers.efferent.to_string()),
                    cell(|numbers| instability(numbers.instability)),
                ));
            }

            self.out("");
            self.out("<sub>Ca: classes in other modules that use it. Ce: classes in other modules it uses. I = Ce / (Ca + Ce).</sub>");
            self.out("");
        }

        for (status, list) in [("New", &diff.outliers.added), ("No longer", &diff.outliers.removed)] {
            for outlier in list {
                self.out(format!("- {status} a fan-out outlier: `{}` uses {} app classes outside its module", outlier.id, outlier.fan_out));
            }
        }

        let hidden = other_edges.len().saturating_sub(LIMIT);

        for line in other_edges.into_iter().take(LIMIT) {
            self.out(format!("- {line}"));
        }

        if hidden > 0 {
            self.out(format!("- … and {hidden} more class edges"));
        }

        for file in diff.other_files.iter().take(LIMIT) {
            let from = file.from.as_ref().map(|from| format!(" from `{from}`")).unwrap_or_default();
            self.out(format!("- `{}` {}{from}, not in the graph", file.path, format!("{:?}", file.status).to_lowercase()));
        }

        if diff.other_files.len() > LIMIT {
            self.out(format!("- … and {} more files", diff.other_files.len() - LIMIT));
        }

        self.end_details();
    }

    /// Edge changes of classes this pull request does not change, whose other end it does.
    fn other_edges(&self) -> Vec<String> {
        let listed: BTreeSet<&str> = self.diff.reading_order.iter().flatten().map(String::as_str).collect();
        let gone: BTreeSet<&str> = self
            .diff
            .classes
            .removed
            .iter()
            .chain(self.diff.classes.renamed.iter().map(|renamed| &renamed.from))
            .map(|class| class.id.as_str())
            .collect();

        let added = self.diff.edges.added.iter().map(|edge| ("New", edge, At::Head));
        let removed = self.diff.edges.removed.iter().map(|edge| ("Gone", edge, At::Base));

        added
            .chain(removed)
            .filter(|(_, edge, _)| !listed.contains(edge.from.as_str()) && !gone.contains(edge.from.as_str()))
            .map(|(status, edge, at)| format!("{status} class edge `{}` → `{}` ({}) · {}", edge.from, edge.to, edge.kind.name(), self.evidence(edge, at)))
            .collect()
    }

    fn evidence(&self, edge: &Edge, at: At) -> String {
        edge.evidence.iter().map(|place| self.place(&place.file, place.line, at)).collect::<Vec<_>>().join(" ")
    }

    fn place(&self, file: &str, line: u32, at: At) -> String {
        let text = format!("`{file}:{line}`");
        let blob = match at {
            At::Base => self.links.map(|links| &links.base),
            At::Head => self.links.and_then(|links| links.head.as_ref()),
        };

        match blob {
            Some(blob) => format!("[{text}]({blob}{}#L{line})", file.replace(' ', "%20")),
            None => text,
        }
    }

    fn details(&mut self, summary: &str) {
        self.out("");
        self.out(format!("<details><summary>{summary}</summary>"));
        self.out("");
    }

    fn end_details(&mut self) {
        self.out("");
        self.out("</details>");
    }

    fn breaks_rule(&self, edge: &Edge) -> bool {
        self.diff.violations.added.iter().any(|violation| violation.edge.from == edge.from && violation.edge.to == edge.to && violation.edge.kind == edge.kind)
    }
}

fn has_findings(diff: &Diff) -> bool {
    !diff.violations.added.is_empty() || !diff.violations.removed.is_empty() || !diff.cycles.is_empty() || !diff.dangling.is_empty()
}

fn improves(diff: &Diff) -> bool {
    !diff.violations.removed.is_empty() || diff.cycles.iter().any(|cycle| !cycle.left.is_empty())
}

fn cycle_finding(cycle: &CycleChange) -> String {
    let list = |modules: &[String]| modules.iter().map(|module| format!("`{module}`")).collect::<Vec<_>>().join(", ");
    let size = cycle.after.as_ref().map_or(0, Vec::len);

    match cycle.status {
        "new" => format!("🔴 **New module cycle** of {size} modules: {}", list(&cycle.joined)),
        "grown" => format!("🔴 **Module cycle grew** to {size} modules: {} joined", list(&cycle.joined)),
        "shrunk" => format!("🟢 **Module cycle shrank** to {size} modules: {} left", list(&cycle.left)),
        "broken" => format!("🟢 **Module cycle broken**: {} no longer depend on each other", list(&cycle.left)),
        _ => format!("🟡 **Module cycle changed**, now {size} modules: {} joined, {} left", list(&cycle.joined), list(&cycle.left)),
    }
}

fn counted(count: usize, icon: &str, word: &str) -> String {
    if count == 0 {
        return String::new();
    }

    [icon, &count.to_string(), word].iter().filter(|part| !part.is_empty()).copied().collect::<Vec<_>>().join(" ")
}

fn joined(parts: &[String]) -> String {
    parts.iter().filter(|part| !part.is_empty()).cloned().collect::<Vec<_>>().join(", ")
}

fn or(text: String, otherwise: &str) -> String {
    if text.is_empty() { otherwise.to_string() } else { text }
}

fn arrow(from: &str, to: &str) -> String {
    format!("`{}` → `{}`", class_name(from), class_name(to))
}

fn class_name(id: &str) -> &str {
    id.rsplit('\\').next().unwrap_or(id)
}

fn namespace(id: &str) -> &str {
    id.rsplit_once('\\').map_or("", |(namespace, _)| namespace)
}
