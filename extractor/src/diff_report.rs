//! `puml-extract diff` as text for terminals and agents, as Markdown for a pull request comment, or as
//! JSON. The text and the Markdown say the same things in the same order: what the pull request does to
//! the verdict, the order in which to read its classes with what changed in each, and what else depends
//! on them.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::diff::Diff;
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

/// Lists in Markdown are cut here; a pull request comment holds at most 65,536 characters.
const MARKDOWN_LIMIT: usize = 40;
const EDGES_PER_CLASS: usize = 8;
const NAMES_PER_LINE: usize = 8;

pub fn render(diff: &Diff, format: Format) -> String {
    if format == Format::Json {
        return serde_json::to_string_pretty(diff).expect("the diff serializes to JSON") + "\n";
    }

    let mut writer = Writer { diff, markdown: format == Format::Markdown, lines: Vec::new() };
    writer.write();

    let mut text = writer.lines.join("\n");
    text.push('\n');

    text
}

struct Writer<'a> {
    diff: &'a Diff,
    markdown: bool,
    lines: Vec<String>,
}

impl<'a> Writer<'a> {
    fn out(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn code(&self, text: &str) -> String {
        if self.markdown { format!("`{text}`") } else { text.to_string() }
    }

    fn heading(&mut self, title: &str, note: &str) {
        self.out("");

        if self.markdown {
            self.out(format!("### {title}"));

            if !note.is_empty() {
                self.out("");
                self.out(format!("_{note}_"));
            }

            self.out("");
        } else if note.is_empty() {
            self.out(title.to_uppercase());
        } else {
            self.out(format!("{}  ({note})", title.to_uppercase()));
        }
    }

    fn item(&mut self, depth: usize, text: impl AsRef<str>) {
        let text = text.as_ref();

        if self.markdown {
            self.out(format!("{}- {text}", "   ".repeat(depth)));
        } else {
            self.out(format!("{}{text}", "  ".repeat(depth + 1)));
        }
    }

    fn numbered(&mut self, number: usize, text: impl AsRef<str>) {
        let text = text.as_ref();

        if self.markdown {
            self.out(format!("{number}. {text}"));
        } else {
            self.out(format!("  {number:>2}. {text}"));
        }
    }

    /// How many entries of a list to print: all of them in text, at most `MARKDOWN_LIMIT` in Markdown.
    fn shown(&self, count: usize) -> usize {
        if self.markdown { count.min(MARKDOWN_LIMIT) } else { count }
    }

    fn more(&mut self, depth: usize, hidden: usize) {
        if hidden > 0 {
            self.item(depth, format!("... {hidden} more: run {} locally for all of them", self.code("puml-extract diff")));
        }
    }

    fn write(&mut self) {
        self.header();

        if self.markdown {
            self.diagram();
        }

        self.violations();
        self.cycles();
        self.dangling();
        self.reading_order();
        self.removed();
        self.impact();

        if self.markdown {
            self.out("");
            self.out("<details><summary>Module edges, metrics and everything else</summary>");
        }

        self.modules();
        self.metrics();
        self.outliers();
        self.other_edges();
        self.other_files();

        if self.markdown {
            self.out("");
            self.out("</details>");
        }
    }

    fn header(&mut self) {
        let diff = self.diff;
        let classes = &diff.classes;

        let base = format!("{} ({})", self.code(&diff.base.reference), short(&diff.base.commit));
        let head = format!(
            "{} ({}{})",
            self.code(&diff.head.reference),
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

        if self.markdown {
            self.out("## puml: what this pull request changes in the architecture");
            self.out("");
            self.out(format!("{base} → {head}. {}.", capitalize(&counts)));
            self.out("");
            self.out(format!("**Architecture:** {verdict}."));
        } else {
            self.out(format!("puml diff: {}, base {base} -> head {head}, {}", diff.project, diff.generator));
            self.out(counts);
            self.out(format!("architecture: {verdict}"));
        }

        for note in &diff.notes {
            if self.markdown {
                self.out("");
                self.out(format!("> {note}"));
            } else {
                self.out(format!("note: {note}"));
            }
        }
    }

    /// The module edges whose weight changed, as a Mermaid flowchart, which GitHub draws in a comment.
    fn diagram(&mut self) {
        let modules = &self.diff.modules;

        if modules.is_empty() {
            return;
        }

        let shown = &modules[..modules.len().min(MARKDOWN_LIMIT)];
        let ids: BTreeSet<&str> = shown.iter().flat_map(|change| [change.from.as_str(), change.to.as_str()]).collect();
        let node: HashMap<&str, String> = ids.iter().enumerate().map(|(index, id)| (*id, format!("m{index}"))).collect();
        let joined: BTreeSet<&str> = self.diff.cycles.iter().flat_map(|cycle| cycle.joined.iter().map(String::as_str)).collect();

        self.heading(
            "Module edges this pull request changes",
            "green: new, red and dashed: gone, grey: heavier or lighter, purple: breaks a rule, red border: newly in a module cycle",
        );
        self.out("```mermaid");
        self.out("flowchart LR");

        for id in &ids {
            self.out(format!("  {}[\"{id}\"]", node[id]));
        }

        for (position, change) in shown.iter().enumerate() {
            let (arrow, label) = match (change.before, change.after) {
                (0, after) => ("-->", format!("new: {after}")),
                (before, 0) => ("-.->", format!("gone: was {before}")),
                (before, after) => ("-->", format!("{before} → {after}")),
            };
            let label = if change.violations > 0 {
                format!("{label}, {} violation{}", change.violations, plural(change.violations as usize, "s"))
            } else {
                label
            };
            let color = match (change.before, change.after, change.violations) {
                (_, _, violations) if violations > 0 => "#8250df",
                (0, _, _) => "#1a7f37",
                (_, 0, _) => "#cf222e",
                _ => "#6e7781",
            };

            self.out(format!("  {} {arrow}|\"{label}\"| {}", node[change.from.as_str()], node[change.to.as_str()]));
            self.out(format!("  linkStyle {position} stroke:{color},stroke-width:2px"));
        }

        let cycling: Vec<&str> = ids.iter().filter(|id| joined.contains(*id)).map(|id| node[id].as_str()).collect();

        if !cycling.is_empty() {
            self.out("  classDef cycle stroke:#cf222e,stroke-width:3px");
            self.out(format!("  class {} cycle", cycling.join(",")));
        }

        self.out("```");

        if shown.len() < modules.len() {
            self.out(format!("{} more module edges changed; see the list below.", modules.len() - shown.len()));
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
                let line = format!("{status}  {}  {}", self.code(violation.rule), self.edge(&violation.edge));
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
                let cut: Vec<String> = cycle.cut.iter().map(|edge| self.code(&format!("{} -> {} ({})", edge.from, edge.to, edge.weight))).collect();

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
            let line = format!("{} is still used by", self.code(&dangling.class));
            self.item(0, line);

            for edge in &dangling.used_by {
                let line = format!("{}  {}  {}", self.code(&edge.from), edge.kind.name(), self.code(&evidence(edge)));
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

        let total: usize = diff.reading_order.iter().map(Vec::len).sum();
        let shown = self.shown(total);

        self.heading("Read in this order", "each class after the classes of this pull request it depends on");

        let mut number = 0;

        'groups: for group in &diff.reading_order {
            if group.len() > 1 {
                let line = format!("the next {} classes depend on each other; read them together", group.len());

                if self.markdown {
                    self.out("");
                    self.out(format!("_{line}:_"));
                    self.out("");
                } else {
                    self.out(format!("      {line}:"));
                }
            }

            for id in group {
                if number == shown {
                    break 'groups;
                }

                number += 1;

                let status = match (modified.get(id.as_str()), renamed.get(id.as_str())) {
                    (Some(_), _) => "modified".to_string(),
                    (_, Some(from)) => format!("renamed from {}", self.code(from)),
                    _ => "added".to_string(),
                };
                let place = places.get(id.as_str()).cloned().unwrap_or_default();
                let line = format!("{}  {status}  {}", self.code(id), self.code(&place));

                self.numbered(number, line);

                if let Some(modified) = modified.get(id.as_str()) {
                    for member in &modified.members {
                        let line = match (&member.before, &member.after) {
                            (Some(before), Some(after)) => format!("changed {} -> {}", self.code(before), self.code(after)),
                            (Some(before), None) => format!("removed {}", self.code(before)),
                            (None, Some(after)) => format!("added {}", self.code(after)),
                            (None, None) => continue,
                        };

                        self.item(1, line);
                    }
                }

                if let Some((added, removed)) = edges.get(id.as_str()) {
                    let all: Vec<(char, &Edge)> = added.iter().map(|edge| ('+', *edge)).chain(removed.iter().map(|edge| ('-', *edge))).collect();
                    let limit = if self.markdown { EDGES_PER_CLASS } else { all.len() };

                    for (sign, edge) in all.iter().take(limit) {
                        let line = self.dependency(*sign, edge);
                        self.item(1, line);
                    }

                    self.more(1, all.len().saturating_sub(limit));
                }
            }
        }

        self.more(0, total - number);
    }

    fn dependency(&self, sign: char, edge: &Edge) -> String {
        let verb = if sign == '+' { "now depends on" } else { "no longer depends on" };
        let via = edge.label.as_ref().map(|label| format!(" via {}", self.code(label))).unwrap_or_default();
        let rule = self
            .violated_rule(edge, sign == '+')
            .map(|rule| format!("  {} {}", if sign == '+' { "breaks" } else { "fixes" }, self.code(rule)))
            .unwrap_or_default();

        let sign = if self.markdown { String::new() } else { format!("{sign} ") };

        format!("{sign}{verb} {} ({}{via})  {}{rule}", self.code(&edge.to), edge.kind.name(), self.code(&evidence(edge)))
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
            let line = format!("{}  {}", self.code(&class.id), self.code(&format!("{}:{}", class.file, class.line)));
            self.item(0, line);
        }
    }

    fn impact(&mut self) {
        let impact = &self.diff.impact;

        if impact.classes.is_empty() {
            return;
        }

        self.heading("What else depends on it", "classes outside this pull request that use the classes it modifies");

        let shown = self.shown(impact.classes.len());

        for entry in &impact.classes[..shown] {
            let api = if entry.api_changed { ", whose public API changed," } else { "" };
            let names: Vec<String> = entry.dependents.iter().take(NAMES_PER_LINE).map(|id| self.code(id)).collect();
            let rest = entry.dependents.len().saturating_sub(NAMES_PER_LINE);
            let rest = if rest > 0 { format!(" and {rest} more") } else { String::new() };

            let line = format!(
                "{}{api} is used by {} class{}: {}{rest}",
                self.code(&entry.class),
                entry.dependents.len(),
                plural(entry.dependents.len(), "es"),
                names.join(", ")
            );
            self.item(0, line);
        }

        self.more(0, impact.classes.len() - shown);

        let total: usize = impact.reach.values().sum();
        let mut modules: Vec<(&String, &usize)> = impact.reach.iter().collect();
        modules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

        let modules: Vec<String> = modules.into_iter().map(|(module, count)| format!("{} {count}", self.code(module))).collect();

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

            let line = format!("{}  {} -> {} ({what}{violations})", self.code(&format!("{} -> {}", change.from, change.to)), change.before, change.after);
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

            let line = format!("{}  {line}", self.code(&change.module));
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
                let line = format!("{status}  {}  fan-out {}", self.code(&outlier.id), outlier.fan_out);
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

        let shown = self.shown(edges.len());

        for (sign, edge) in &edges[..shown] {
            let line = format!("{}  {}", if *sign == '+' { "new" } else { "gone" }, self.edge(edge));
            self.item(0, line);
        }

        self.more(0, edges.len() - shown);
    }

    fn other_files(&mut self) {
        let files = &self.diff.other_files;

        if files.is_empty() {
            return;
        }

        self.heading("Changed files the graph does not cover", "only classes under the PSR-4 roots are in the graph");

        let shown = self.shown(files.len());

        for file in &files[..shown] {
            let from = file.from.as_ref().map(|from| format!(" from {}", self.code(from))).unwrap_or_default();
            let line = format!("{}  {}{from}", self.code(&file.path), format!("{:?}", file.status).to_lowercase());
            self.item(0, line);
        }

        self.more(0, files.len() - shown);
    }

    fn edge(&self, edge: &Edge) -> String {
        let via = edge.label.as_ref().map(|label| format!(" via {}", self.code(label))).unwrap_or_default();

        format!("{} ({}{via})  {}", self.code(&format!("{} -> {}", edge.from, edge.to)), edge.kind.name(), self.code(&evidence(edge)))
    }

    fn list(&self, ids: &[String]) -> String {
        ids.iter().map(|id| self.code(id)).collect::<Vec<_>>().join(", ")
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

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(7)]
}

fn instability(value: Option<f64>) -> String {
    value.map_or("-".to_string(), |value| format!("{value:.2}"))
}

fn capitalize(text: &str) -> String {
    let mut characters = text.chars();

    characters.next().map(|first| first.to_uppercase().chain(characters).collect()).unwrap_or_default()
}
