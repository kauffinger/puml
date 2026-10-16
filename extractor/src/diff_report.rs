//! `puml-extract diff` as text for terminals and agents, or as JSON: what the pull request does to the
//! verdict, the order in which to read its classes with what changed in each, and what else depends on
//! them. The Markdown for a pull request comment is in `diff_markdown`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::diff::Diff;
use crate::diff_markdown;
use crate::diff::Modified;
use crate::graph::Edge;
use crate::report::evidence;
use crate::report::plural;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Markdown,
    Json,
}

const NAMES_PER_LINE: usize = 8;

pub fn render(diff: &Diff, format: Format) -> String {
    match format {
        Format::Json => return serde_json::to_string_pretty(diff).expect("the diff serializes to JSON") + "\n",
        Format::Markdown => return diff_markdown::render(diff),
        Format::Text => {}
    }

    let mut writer = Writer { diff, lines: Vec::new() };
    writer.write();

    let mut text = writer.lines.join("\n");
    text.push('\n');

    text
}

struct Writer<'a> {
    diff: &'a Diff,
    lines: Vec<String>,
}

impl<'a> Writer<'a> {
    fn out(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn heading(&mut self, title: &str, note: &str) {
        self.out("");

        if note.is_empty() {
            self.out(title.to_uppercase());
        } else {
            self.out(format!("{}  ({note})", title.to_uppercase()));
        }
    }

    fn item(&mut self, depth: usize, text: impl AsRef<str>) {
        self.out(format!("{}{}", "  ".repeat(depth + 1), text.as_ref()));
    }

    fn write(&mut self) {
        self.header();
        self.violations();
        self.cycles();
        self.dangling();
        self.reading_order();
        self.removed();
        self.impact();
        self.modules();
        self.metrics();
        self.outliers();
        self.other_edges();
        self.other_files();
    }

    fn header(&mut self) {
        let diff = self.diff;
        let classes = &diff.classes;

        let base = format!("{} ({})", diff.base.reference, short(&diff.base.commit));
        let head = format!(
            "{} ({}{})",
            diff.head.reference,
            short(&diff.head.commit),
            if diff.head.uncommitted { " with uncommitted changes" } else { "" }
        );

        let counts = format!(
            "classes: {} added, {} modified, {} renamed, {} removed; class edges: {} added, {} removed; {} changed {} classes, {} other{} changed",
            classes.added.len(),
            classes.modified.len(),
            classes.renamed.len(),
            classes.removed.len(),
            diff.edges.added.len(),
            diff.edges.removed.len(),
            diff.files.len(),
            if diff.files.len() == 1 { "file declares" } else { "files declare" },
            diff.other_files.len(),
            if diff.other_files.len() == 1 { " file" } else { " files" },
        );

        let verdict = verdict_summary(diff);
        let verdict = if verdict.is_empty() { "unchanged: no rule violation, module cycle or removed class in use was added or fixed".to_string() } else { verdict.join(", ") };

        self.out(format!("puml diff: {}, base {base} -> head {head}, {}", diff.project, diff.generator));
        self.out(counts);
        self.out(format!("architecture: {verdict}"));

        for note in &diff.notes {
            self.out(format!("note: {note}"));
        }
    }

    fn violations(&mut self) {
        let violations = &self.diff.violations;

        if violations.added.is_empty() && violations.removed.is_empty() {
            return;
        }

        self.heading("Rule violations", "one per class edge; puml-extract report explains each rule");

        for (status, list) in [("new", &violations.added), ("fixed", &violations.removed)] {
            for violation in list.iter() {
                let line = format!("{status}  {}  {}", violation.rule, self.edge(&violation.edge));
                self.item(0, line);
            }
        }
    }

    fn cycles(&mut self) {
        if self.diff.cycles.is_empty() {
            return;
        }

        self.heading("Module cycles", "modules that, through some path, all depend on each other");

        for cycle in &self.diff.cycles {
            let modules = cycle.after.as_ref().unwrap_or(&cycle.left);
            let line = match cycle.status {
                "new" => format!("new cycle of {} modules: {}", modules.len(), self.list(modules)),
                "broken" => format!("cycle broken, its modules no longer depend on each other: {}", self.list(modules)),
                status => {
                    let mut parts = Vec::new();

                    if !cycle.joined.is_empty() {
                        parts.push(format!("joined by {}", self.list(&cycle.joined)));
                    }

                    if !cycle.left.is_empty() {
                        parts.push(format!("left by {}", self.list(&cycle.left)));
                    }

                    format!("cycle {status}, now {} modules: {}", modules.len(), parts.join("; "))
                }
            };

            self.item(0, line);

            if cycle.after.is_some() && cycle.status != "new" {
                self.item(1, format!("now: {}", self.list(modules)));
            }

            if let Some(cost) = &cycle.cost {
                let cut: Vec<String> = cycle.cut.iter().map(|edge| format!("{} -> {} ({})", edge.from, edge.to, edge.weight)).collect();

                self.item(
                    1,
                    format!(
                        "cheapest cut at the head: {} class edge{} in {} file{}: {}",
                        cost.class_edges,
                        plural(cost.class_edges as usize, "s"),
                        cost.files,
                        plural(cost.files, "s"),
                        cut.join(", ")
                    ),
                );
            }
        }
    }

    fn dangling(&mut self) {
        if self.diff.dangling.is_empty() {
            return;
        }

        self.heading("Removed classes still in use", "the head references classes it no longer declares");

        for dangling in &self.diff.dangling {
            let line = format!("{} is still used by", dangling.class);
            self.item(0, line);

            for edge in &dangling.used_by {
                let line = format!("{}  {}  {}", edge.from, edge.kind.name(), evidence(edge));
                self.item(1, line);
            }
        }
    }

    fn reading_order(&mut self) {
        let diff = self.diff;

        if diff.reading_order.is_empty() {
            return;
        }

        let modified: HashMap<&str, &Modified> = diff.classes.modified.iter().map(|modified| (modified.class.id.as_str(), modified)).collect();
        let renamed: HashMap<&str, &str> = diff.classes.renamed.iter().map(|renamed| (renamed.to.id.as_str(), renamed.from.id.as_str())).collect();
        let places: HashMap<&str, String> = diff
            .classes
            .added
            .iter()
            .chain(diff.classes.modified.iter().map(|modified| &modified.class))
            .chain(diff.classes.renamed.iter().map(|renamed| &renamed.to))
            .map(|class| (class.id.as_str(), format!("{}:{}", class.file, class.line)))
            .collect();
        let edges = self.edges_by_source();

        self.heading("Read in this order", "each class after the classes of this pull request it depends on");

        let mut number = 0;

        for group in &diff.reading_order {
            if group.len() > 1 {
                self.out(format!("      the next {} classes depend on each other; read them together:", group.len()));
            }

            for id in group {
                number += 1;

                let status = match (modified.get(id.as_str()), renamed.get(id.as_str())) {
                    (Some(_), _) => "modified".to_string(),
                    (_, Some(from)) => format!("renamed from {from}"),
                    _ => "added".to_string(),
                };
                let place = places.get(id.as_str()).cloned().unwrap_or_default();
                let line = format!("{id}  {status}  {place}");

                self.out(format!("  {number:>2}. {line}"));

                if let Some(modified) = modified.get(id.as_str()) {
                    for member in &modified.members {
                        let line = match (&member.before, &member.after) {
                            (Some(before), Some(after)) => format!("changed {before} -> {after}"),
                            (Some(before), None) => format!("removed {before}"),
                            (None, Some(after)) => format!("added {after}"),
                            (None, None) => continue,
                        };

                        self.item(1, line);
                    }
                }

                if let Some((added, removed)) = edges.get(id.as_str()) {
                    for (sign, edge) in added.iter().map(|edge| ('+', *edge)).chain(removed.iter().map(|edge| ('-', *edge))) {
                        let line = self.dependency(sign, edge);
                        self.item(1, line);
                    }
                }
            }
        }
    }

    fn dependency(&self, sign: char, edge: &Edge) -> String {
        let verb = if sign == '+' { "now depends on" } else { "no longer depends on" };
        let via = edge.label.as_ref().map(|label| format!(" via {label}")).unwrap_or_default();
        let rule = self
            .violated_rule(edge, sign == '+')
            .map(|rule| format!("  {} {}", if sign == '+' { "breaks" } else { "fixes" }, rule))
            .unwrap_or_default();

        format!("{sign} {verb} {} ({}{via})  {}{rule}", edge.to, edge.kind.name(), evidence(edge))
    }

    fn violated_rule(&self, edge: &Edge, added: bool) -> Option<&'static str> {
        let list = if added { &self.diff.violations.added } else { &self.diff.violations.removed };

        list.iter()
            .find(|violation| violation.edge.from == edge.from && violation.edge.to == edge.to && violation.edge.kind == edge.kind)
            .map(|violation| violation.rule)
    }

    /// Added and removed class edges, by the class they leave.
    fn edges_by_source(&self) -> HashMap<&'a str, (Vec<&'a Edge>, Vec<&'a Edge>)> {
        let mut edges: HashMap<&str, (Vec<&Edge>, Vec<&Edge>)> = HashMap::new();

        for edge in &self.diff.edges.added {
            edges.entry(&edge.from).or_default().0.push(edge);
        }

        for edge in &self.diff.edges.removed {
            edges.entry(&edge.from).or_default().1.push(edge);
        }

        edges
    }

    fn removed(&mut self) {
        let removed = &self.diff.classes.removed;

        if removed.is_empty() {
            return;
        }

        self.heading("Removed", "");

        for class in removed {
            let line = format!("{}  {}", class.id, (format!("{}:{}", class.file, class.line)));
            self.item(0, line);
        }
    }

    fn impact(&mut self) {
        let impact = &self.diff.impact;

        if impact.classes.is_empty() {
            return;
        }

        self.heading("What else depends on it", "classes outside this pull request that use the classes it modifies");

        for entry in &impact.classes {
            let api = if entry.api_changed { ", whose public API changed," } else { "" };
            let names: Vec<String> = entry.dependents.iter().take(NAMES_PER_LINE).cloned().collect();
            let rest = entry.dependents.len().saturating_sub(NAMES_PER_LINE);
            let rest = if rest > 0 { format!(" and {rest} more") } else { String::new() };

            let line = format!(
                "{}{api} is used by {} class{}: {}{rest}",
                entry.class,
                entry.dependents.len(),
                plural(entry.dependents.len(), "es"),
                names.join(", ")
            );
            self.item(0, line);
        }


        let total: usize = impact.reach.values().sum();
        let mut modules: Vec<(&String, &usize)> = impact.reach.iter().collect();
        modules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

        let modules: Vec<String> = modules.into_iter().map(|(module, count)| format!("{} {count}", module)).collect();

        self.item(
            0,
            format!(
                "in all, {total} class{} outside this pull request depend on the modified classes, directly or through others: {}",
                plural(total, "es"),
                modules.join(", ")
            ),
        );
    }

    fn modules(&mut self) {
        if self.diff.modules.is_empty() {
            return;
        }

        self.heading("Module edges", "app modules only; weight in class edges, before -> after");

        for change in &self.diff.modules {
            let what = match (change.before, change.after) {
                (0, _) => "new",
                (_, 0) => "gone",
                (before, after) if after > before => "heavier",
                _ => "lighter",
            };
            let violations = if change.violations > 0 {
                format!(", {} breaking a rule", change.violations)
            } else {
                String::new()
            };

            let line = format!("{}  {} -> {} ({what}{violations})", (format!("{} -> {}", change.from, change.to)), change.before, change.after);
            self.item(0, line);
        }
    }

    fn metrics(&mut self) {
        if self.diff.metrics.is_empty() {
            return;
        }

        self.heading("Module metrics", "Ca: classes in other modules that use it, Ce: classes in other modules it uses, I = Ce/(Ca+Ce)");

        for change in &self.diff.metrics {
            let line = match (&change.before, &change.after) {
                (None, Some(after)) => format!(
                    "new module, {} class{}, Ca {}, Ce {}, I {}",
                    after.classes,
                    plural(after.classes, "es"),
                    after.afferent,
                    after.efferent,
                    instability(after.instability)
                ),
                (Some(_), None) => "module removed".to_string(),
                (Some(before), Some(after)) => format!(
                    "classes {} -> {}, Ca {} -> {}, Ce {} -> {}, I {} -> {}",
                    before.classes,
                    after.classes,
                    before.afferent,
                    after.afferent,
                    before.efferent,
                    after.efferent,
                    instability(before.instability),
                    instability(after.instability)
                ),
                (None, None) => continue,
            };

            let line = format!("{}  {line}", change.module);
            self.item(0, line);
        }
    }

    fn outliers(&mut self) {
        let outliers = &self.diff.outliers;

        if outliers.added.is_empty() && outliers.removed.is_empty() {
            return;
        }

        self.heading("Fan-out outliers", "classes that use far more app classes outside their module than the rest");

        for (status, list) in [("new", &outliers.added), ("no longer", &outliers.removed)] {
            for outlier in list.iter() {
                let line = format!("{status}  {}  fan-out {}", outlier.id, outlier.fan_out);
                self.item(0, line);
            }
        }
    }

    /// Edge changes that do not leave a class in the reading order, such as an edge that now ends at a
    /// class declared somewhere else. Edges of removed classes are left out: they went with the class.
    fn other_edges(&mut self) {
        let listed: BTreeSet<&str> = self.diff.reading_order.iter().flatten().map(String::as_str).collect();
        let gone: BTreeSet<&str> = self
            .diff
            .classes
            .removed
            .iter()
            .chain(self.diff.classes.renamed.iter().map(|renamed| &renamed.from))
            .map(|class| class.id.as_str())
            .collect();

        let edges: Vec<(char, &Edge)> = self
            .diff
            .edges
            .added
            .iter()
            .map(|edge| ('+', edge))
            .chain(self.diff.edges.removed.iter().map(|edge| ('-', edge)))
            .filter(|(_, edge)| !listed.contains(edge.from.as_str()) && !gone.contains(edge.from.as_str()))
            .collect();

        if edges.is_empty() {
            return;
        }

        self.heading("Other class edges", "edges of classes this pull request does not change, whose other end it does");

        for (sign, edge) in &edges {
            let line = format!("{}  {}", if *sign == '+' { "new" } else { "gone" }, self.edge(edge));
            self.item(0, line);
        }
    }

    fn other_files(&mut self) {
        let files = &self.diff.other_files;

        if files.is_empty() {
            return;
        }

        self.heading("Changed files the graph does not cover", "only classes under the PSR-4 roots are in the graph");

        for file in files {
            let from = file.from.as_ref().map(|from| format!(" from {}", from)).unwrap_or_default();
            let line = format!("{}  {}{from}", file.path, format!("{:?}", file.status).to_lowercase());
            self.item(0, line);
        }
    }

    fn edge(&self, edge: &Edge) -> String {
        let via = edge.label.as_ref().map(|label| format!(" via {label}")).unwrap_or_default();

        format!("{} ({}{via})  {}", (format!("{} -> {}", edge.from, edge.to)), edge.kind.name(), evidence(edge))
    }

    fn list(&self, ids: &[String]) -> String {
        ids.join(", ")
    }
}

fn verdict_summary(diff: &Diff) -> Vec<String> {
    let mut summary = Vec::new();
    let count = |count: usize, one: &str, many: &str| format!("{count} {}", if count == 1 { one } else { many });

    if !diff.violations.added.is_empty() {
        summary.push(count(diff.violations.added.len(), "new rule violation", "new rule violations"));
    }

    if !diff.violations.removed.is_empty() {
        summary.push(count(diff.violations.removed.len(), "rule violation fixed", "rule violations fixed"));
    }

    let mut cycles: BTreeMap<&str, usize> = BTreeMap::new();

    for cycle in &diff.cycles {
        *cycles.entry(cycle.status).or_default() += 1;
    }

    for (status, number) in cycles {
        let noun = if number == 1 { "module cycle" } else { "module cycles" };

        summary.push(match status {
            "new" => format!("{number} new {noun}"),
            status => format!("{number} {noun} {status}"),
        });
    }

    if !diff.dangling.is_empty() {
        summary.push(count(diff.dangling.len(), "removed class still in use", "removed classes still in use"));
    }

    summary
}

pub fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

pub fn instability(value: Option<f64>) -> String {
    value.map_or("-".to_string(), |value| format!("{value:.2}"))
}
