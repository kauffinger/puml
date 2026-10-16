use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use mago_allocator::LocalArena;
use mago_database::file::File;
use mago_database::file::FileType;
use mago_names::ResolvedNames;
use mago_names::resolver::NameResolver;
use mago_syntax::cst::*;
use mago_syntax::parser::parse_file;
use mago_syntax::walker::Walker;

use crate::project::Project;

const FACADE_BASE: &str = "illuminate\\support\\facades\\facade";
const REAL_TIME_FACADE_PREFIX: &str = "Facades\\";

pub enum FacadeTarget {
    Resolved(String),
    Unresolved(String),
}

pub struct FacadeResolver<'a> {
    project: &'a Project,
    container_aliases: HashMap<String, String>,
    global_aliases: HashMap<String, String>,
    facades: HashMap<String, Option<FacadeTarget>>,
}

impl<'a> FacadeResolver<'a> {
    pub fn new(project: &'a Project) -> Self {
        let mut resolver = FacadeResolver {
            project,
            container_aliases: HashMap::new(),
            global_aliases: HashMap::new(),
            facades: HashMap::new(),
        };

        if let Some(path) = project.file_for("Illuminate\\Foundation\\Application") {
            resolver.container_aliases =
                string_keyed_classes(&path, "Illuminate\\Foundation\\Application", "registerCoreContainerAliases");
        }

        if let Some(path) = project.file_for("Illuminate\\Support\\Facades\\Facade") {
            resolver.global_aliases = string_keyed_classes(&path, "Illuminate\\Support\\Facades\\Facade", "defaultAliases")
                .into_iter()
                .map(|(alias, class)| (alias.to_ascii_lowercase(), class))
                .collect();
        }

        resolver
    }

    pub fn global_alias(&self, name: &str) -> Option<&str> {
        if name.contains('\\') {
            return None;
        }

        self.global_aliases.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn resolve(&mut self, class: &str, source: Option<&Path>) -> Option<&FacadeTarget> {
        let key = class.to_ascii_lowercase();

        if !self.facades.contains_key(&key) {
            let target = self.inspect(class, source);
            self.facades.insert(key.clone(), target);
        }

        self.facades.get(&key).and_then(Option::as_ref)
    }

    fn inspect(&mut self, class: &str, source: Option<&Path>) -> Option<FacadeTarget> {
        if let Some(underlying) = class.strip_prefix(REAL_TIME_FACADE_PREFIX) {
            return Some(FacadeTarget::Resolved(underlying.to_string()));
        }

        let path = match source {
            Some(path) => path.to_path_buf(),
            None => self.project.file_for(class)?,
        };

        let facade = FacadeSource::read(&path, class)?;

        let is_facade = match &facade.parent {
            Some(parent) if parent.eq_ignore_ascii_case(FACADE_BASE) => true,
            Some(parent) => matches!(self.resolve(&parent.clone(), None), Some(_)),
            None => false,
        };

        if !is_facade {
            return None;
        }

        Some(match facade.accessor {
            Some(Accessor::Class(class)) => FacadeTarget::Resolved(class),
            Some(Accessor::Key(key)) => match self.container_aliases.get(&key) {
                Some(class) => FacadeTarget::Resolved(class.clone()),
                None => FacadeTarget::Unresolved(format!("container key '{key}'")),
            },
            None => match &facade.parent {
                Some(parent) if !parent.eq_ignore_ascii_case(FACADE_BASE) => {
                    match self.resolve(&parent.clone(), None) {
                        Some(FacadeTarget::Resolved(class)) => FacadeTarget::Resolved(class.clone()),
                        _ => FacadeTarget::Unresolved("getFacadeAccessor() is not a literal".to_string()),
                    }
                }
                _ => FacadeTarget::Unresolved("getFacadeAccessor() is not a literal".to_string()),
            },
        })
    }
}

enum Accessor {
    Class(String),
    Key(String),
}

struct FacadeSource {
    parent: Option<String>,
    accessor: Option<Accessor>,
}

impl FacadeSource {
    fn read(path: &Path, class: &str) -> Option<Self> {
        let contents = std::fs::read(path).ok()?;
        let file = File::new(Cow::Borrowed(b"facade.php"), FileType::Vendored, None, Cow::Owned(contents));
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        let resolved_names = NameResolver::new(&arena).resolve(program);

        let mut context = FacadeContext {
            resolved_names: &resolved_names,
            class: class.to_ascii_lowercase(),
            in_class: false,
            in_accessor: false,
            found: false,
            parent: None,
            accessor: None,
        };

        FacadeWalker.walk_program(program, &mut context);

        context.found.then_some(FacadeSource { parent: context.parent, accessor: context.accessor })
    }
}

struct FacadeContext<'a, 'arena> {
    resolved_names: &'a ResolvedNames<'arena>,
    class: String,
    in_class: bool,
    in_accessor: bool,
    found: bool,
    parent: Option<String>,
    accessor: Option<Accessor>,
}

struct FacadeWalker;

impl<'ast, 'arena> Walker<'ast, 'arena, FacadeContext<'_, 'arena>> for FacadeWalker {
    fn walk_in_class(&self, class: &'ast Class<'arena>, context: &mut FacadeContext<'_, 'arena>) {
        let name = context.resolved_names.resolve(&class.name).unwrap_or(class.name.value);

        context.in_class = String::from_utf8_lossy(name).eq_ignore_ascii_case(&context.class);

        if !context.in_class {
            return;
        }

        context.found = true;
        context.parent = class
            .extends
            .as_ref()
            .and_then(|extends| extends.types.first())
            .map(|identifier| resolved(context.resolved_names, identifier));
    }

    fn walk_out_class(&self, _: &'ast Class<'arena>, context: &mut FacadeContext<'_, 'arena>) {
        context.in_class = false;
    }

    fn walk_in_method(&self, method: &'ast Method<'arena>, context: &mut FacadeContext<'_, 'arena>) {
        context.in_accessor = context.in_class && method.name.value.eq_ignore_ascii_case(b"getFacadeAccessor");
    }

    fn walk_out_method(&self, _: &'ast Method<'arena>, context: &mut FacadeContext<'_, 'arena>) {
        context.in_accessor = false;
    }

    fn walk_in_return(&self, r#return: &'ast Return<'arena>, context: &mut FacadeContext<'_, 'arena>) {
        if !context.in_accessor || context.accessor.is_some() {
            return;
        }

        context.accessor = match r#return.value {
            Some(Expression::Literal(Literal::String(string))) => {
                string.value.map(|value| Accessor::Key(String::from_utf8_lossy(value).into_owned()))
            }
            Some(Expression::Access(Access::ClassConstant(access))) => match access.class {
                Expression::Identifier(identifier) => Some(Accessor::Class(resolved(context.resolved_names, identifier))),
                _ => None,
            },
            _ => None,
        };
    }
}

fn string_keyed_classes(path: &Path, class: &str, method: &str) -> HashMap<String, String> {
    let Ok(contents) = std::fs::read(path) else {
        return HashMap::new();
    };

    let file = File::new(Cow::Borrowed(b"aliases.php"), FileType::Vendored, None, Cow::Owned(contents));
    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    let resolved_names = NameResolver::new(&arena).resolve(program);

    let mut context = AliasContext {
        resolved_names: &resolved_names,
        class: class.to_string(),
        method: method.as_bytes().to_vec(),
        inside: false,
        aliases: HashMap::new(),
    };

    AliasWalker.walk_program(program, &mut context);

    context.aliases
}

struct AliasContext<'a, 'arena> {
    resolved_names: &'a ResolvedNames<'arena>,
    class: String,
    method: Vec<u8>,
    inside: bool,
    aliases: HashMap<String, String>,
}

struct AliasWalker;

impl<'ast, 'arena> Walker<'ast, 'arena, AliasContext<'_, 'arena>> for AliasWalker {
    fn walk_in_method(&self, method: &'ast Method<'arena>, context: &mut AliasContext<'_, 'arena>) {
        context.inside = method.name.value.eq_ignore_ascii_case(&context.method);
    }

    fn walk_out_method(&self, _: &'ast Method<'arena>, context: &mut AliasContext<'_, 'arena>) {
        context.inside = false;
    }

    fn walk_in_key_value_array_element(
        &self,
        element: &'ast KeyValueArrayElement<'arena>,
        context: &mut AliasContext<'_, 'arena>,
    ) {
        if !context.inside {
            return;
        }

        let Expression::Literal(Literal::String(key)) = element.key else {
            return;
        };

        let Some(key) = key.value else {
            return;
        };

        let class = match element.value {
            Expression::Array(array) => array.elements.first().and_then(|first| match first {
                ArrayElement::Value(value) => class_constant(context, value.value),
                _ => None,
            }),
            value => class_constant(context, value),
        };

        if let Some(class) = class {
            context.aliases.entry(String::from_utf8_lossy(key).into_owned()).or_insert(class);
        }
    }
}

fn class_constant(context: &AliasContext<'_, '_>, expression: &Expression<'_>) -> Option<String> {
    let Expression::Access(Access::ClassConstant(access)) = expression else {
        return None;
    };

    match access.class {
        Expression::Identifier(identifier) => Some(resolved(context.resolved_names, identifier)),
        Expression::Self_(_) | Expression::Static(_) => Some(context.class.clone()),
        _ => None,
    }
}

fn resolved(resolved_names: &ResolvedNames<'_>, identifier: &Identifier<'_>) -> String {
    let name = resolved_names.resolve(identifier).unwrap_or_else(|| identifier.value());

    String::from_utf8_lossy(name).trim_start_matches('\\').to_string()
}
