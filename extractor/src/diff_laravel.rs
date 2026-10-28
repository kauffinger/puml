//! The routes and the rest of what Laravel wires up that a pull request adds, removes or changes, in
//! words. The text report writes full class names; the pull request comment writes short ones in code.

use crate::diff::Fact;
use crate::diff::FactChanges;
use crate::diff::LaravelChanges;
use crate::graph::BindingFact;
use crate::graph::ListenerFact;
use crate::graph::MiddlewareFact;
use crate::graph::ObserverFact;
use crate::graph::PolicyFact;
use crate::graph::RouteFact;
use crate::graph::ScheduleFact;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Text,
    Markdown,
}

pub struct Section {
    pub title: &'static str,
    pub one: &'static str,
    pub many: &'static str,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub lines: Vec<FactLine>,
}

pub struct FactLine {
    /// `added`, `changed` or `removed`.
    pub change: &'static str,
    pub text: String,
    pub file: String,
    pub line: u32,
    /// A removed fact is written in the base.
    pub at_base: bool,
}

impl Section {
    pub fn count(&self) -> usize {
        self.added + self.removed + self.changed
    }

    /// `2 routes: 1 added, 1 changed`.
    pub fn summary(&self) -> String {
        let counts: Vec<String> = [(self.added, "added"), (self.changed, "changed"), (self.removed, "removed")]
            .into_iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, word)| format!("{count} {word}"))
            .collect();

        format!("{} {}: {}", self.count(), if self.count() == 1 { self.one } else { self.many }, counts.join(", "))
    }
}

/// The kinds the pull request changes, routes first.
pub fn sections(changes: &LaravelChanges, style: Style) -> Vec<Section> {
    let words = Words { style };

    [
        section("Routes", "route", "routes", &changes.routes, |fact| words.route(fact), |before, after| words.route_change(before, after)),
        section("Listeners", "listener", "listeners", &changes.listeners, |fact| words.listener(fact), |_, after| words.listener(after)),
        section("Observers", "observer", "observers", &changes.observers, |fact| words.observer(fact), |before, after| words.observer_change(before, after)),
        section("Policies", "policy", "policies", &changes.policies, |fact| words.policy(fact), |before, after| words.policy_change(before, after)),
        section("Bindings", "binding", "bindings", &changes.bindings, |fact| words.binding(fact), |before, after| words.binding_change(before, after)),
        section("Schedule", "scheduled task", "scheduled tasks", &changes.schedule, |fact| words.task(fact), |_, after| words.task(after)),
        section("Middleware", "middleware", "middleware", &changes.middleware, |fact| words.middleware(fact), |before, after| words.middleware_change(before, after)),
    ]
    .into_iter()
    .filter(|section| section.count() > 0)
    .collect()
}

fn section<T: Fact>(
    title: &'static str,
    one: &'static str,
    many: &'static str,
    changes: &FactChanges<T>,
    describe: impl Fn(&T) -> String,
    change: impl Fn(&T, &T) -> String,
) -> Section {
    let line = |change: &'static str, text: String, fact: &T, at_base: bool| {
        let (file, line) = fact.place();

        FactLine { change, text, file: file.to_string(), line, at_base }
    };

    let mut lines: Vec<FactLine> = Vec::new();
    lines.extend(changes.added.iter().map(|fact| line("added", describe(fact), fact, false)));
    lines.extend(changes.changed.iter().map(|pair| line("changed", change(&pair.before, &pair.after), &pair.after, false)));
    lines.extend(changes.removed.iter().map(|fact| line("removed", describe(fact), fact, true)));

    Section {
        title,
        one,
        many,
        added: changes.added.len(),
        removed: changes.removed.len(),
        changed: changes.changed.len(),
        lines,
    }
}

struct Words {
    style: Style,
}

impl Words {
    fn code(&self, text: &str) -> String {
        match self.style {
            Style::Text => text.to_string(),
            Style::Markdown => format!("`{text}`"),
        }
    }

    fn class(&self, id: &str) -> String {
        match self.style {
            Style::Text => id.to_string(),
            Style::Markdown => format!("`{}`", short(id)),
        }
    }

    fn arrow(&self) -> &'static str {
        match self.style {
            Style::Text => "->",
            Style::Markdown => "→",
        }
    }

    fn list(&self, items: &[String]) -> String {
        if items.is_empty() {
            return "none".to_string();
        }

        items.iter().map(|item| self.code(item)).collect::<Vec<_>>().join(", ")
    }

    /// `Class@method`, `view:name`, `redirect:/to`, `livewire:component` or `closure`.
    fn action(&self, action: &str) -> String {
        match (self.style, action.split_once('@')) {
            (Style::Markdown, Some((class, method))) => self.code(&format!("{}@{method}", short(class))),
            _ => self.code(action),
        }
    }

    fn route_key(&self, route: &RouteFact) -> String {
        let domain = route.domain.as_ref().map(|domain| format!(" on {}", self.code(domain))).unwrap_or_default();

        format!("{}{domain}", self.code(&format!("{} {}", route.methods.join("|"), route.uri)))
    }

    fn route(&self, route: &RouteFact) -> String {
        let mut text = format!("{} {} {}", self.route_key(route), self.arrow(), self.action(&route.action));

        if let Some(name) = &route.name {
            text += &format!(", named {}", self.code(name));
        }

        if !route.middleware.is_empty() {
            text += &format!(", middleware {}", self.list(&route.middleware));
        }

        if !route.excluded_middleware.is_empty() {
            text += &format!(", without {}", self.list(&route.excluded_middleware));
        }

        text
    }

    fn route_change(&self, before: &RouteFact, after: &RouteFact) -> String {
        let arrow = self.arrow();
        let name = |route: &RouteFact| route.name.as_ref().map_or("no name".to_string(), |name| format!("named {}", self.code(name)));
        let mut parts = Vec::new();

        if before.action != after.action {
            parts.push(format!("{} {arrow} {}", self.action(&before.action), self.action(&after.action)));
        }

        if before.name != after.name {
            parts.push(format!("{} {arrow} {}", name(before), name(after)));
        }

        if before.middleware != after.middleware {
            parts.push(format!("middleware {} {arrow} {}", self.list(&before.middleware), self.list(&after.middleware)));
        }

        if before.excluded_middleware != after.excluded_middleware {
            parts.push(format!("without {} {arrow} {}", self.list(&before.excluded_middleware), self.list(&after.excluded_middleware)));
        }

        format!("{}: {}", self.route_key(after), parts.join("; "))
    }

    fn listener(&self, listener: &ListenerFact) -> String {
        let handler = match (&listener.class, listener.listener.as_str()) {
            (Some(class), "closure") => format!("a closure in {}", self.class(class)),
            _ => self.action(&listener.listener),
        };

        format!("{} {} {handler}", self.class(&listener.event), self.arrow())
    }

    fn observer(&self, observer: &ObserverFact) -> String {
        format!("{} observed by {} on {}", self.class(&observer.model), self.class(&observer.observer), observer.events.join(", "))
    }

    fn observer_change(&self, before: &ObserverFact, after: &ObserverFact) -> String {
        format!(
            "{} observed by {}: on {} {} on {}",
            self.class(&after.model),
            self.class(&after.observer),
            before.events.join(", "),
            self.arrow(),
            after.events.join(", ")
        )
    }

    fn policy(&self, policy: &PolicyFact) -> String {
        format!("{} authorized by {}", self.class(&policy.model), self.class(&policy.policy))
    }

    fn policy_change(&self, before: &PolicyFact, after: &PolicyFact) -> String {
        format!("{}: authorized by {} {} {}", self.class(&after.model), self.class(&before.policy), self.arrow(), self.class(&after.policy))
    }

    fn concrete(&self, binding: &BindingFact) -> String {
        let concrete = binding.concrete.as_ref().map_or("what a closure returns".to_string(), |class| self.class(class));

        format!("{concrete} as {}", binding.lifetime)
    }

    fn binding(&self, binding: &BindingFact) -> String {
        format!("{} bound to {}", self.class(&binding.key), self.concrete(binding))
    }

    fn binding_change(&self, before: &BindingFact, after: &BindingFact) -> String {
        format!("{}: bound to {} {} {}", self.class(&after.key), self.concrete(before), self.arrow(), self.concrete(after))
    }

    fn task(&self, task: &ScheduleFact) -> String {
        match task.task {
            "job" => format!("job {}", self.class(&task.target)),
            "call" if task.target == "closure" => "a closure".to_string(),
            kind => format!("{kind} {}", self.code(&task.target)),
        }
    }

    fn slot(&self, slot: &str) -> String {
        match slot.split_once(':') {
            Some(("alias", alias)) => format!("as alias {}", self.code(alias)),
            Some(("group", group)) => format!("in the {} group", self.code(group)),
            _ => format!("in the {slot} stack"),
        }
    }

    fn middleware(&self, middleware: &MiddlewareFact) -> String {
        format!("{} {}", self.class(&middleware.class), self.slot(&middleware.slot))
    }

    fn middleware_change(&self, before: &MiddlewareFact, after: &MiddlewareFact) -> String {
        format!("{}: {} {} {}", self.slot(&after.slot), self.class(&before.class), self.arrow(), self.class(&after.class))
    }
}

pub fn short(id: &str) -> &str {
    id.rsplit('\\').next().unwrap_or(id)
}
