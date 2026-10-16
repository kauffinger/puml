use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;

use mago_allocator::LocalArena;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::class_like::ClassLikeMetadata;
use mago_codex::metadata::function_like::FunctionLikeMetadata;
use mago_codex::metadata::ttype::TypeMetadata;
use mago_codex::scanner::scan_program;
use mago_codex::symbol::SymbolKind;
use mago_codex::ttype::TType;
use mago_codex::ttype::TypeRef;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::reference::TReference;
use mago_codex::ttype::union::TUnion;
use mago_codex::visibility::Visibility;
use mago_database::file::File;
use mago_database::file::FileType;
use mago_names::ResolvedNames;
use mago_names::resolver::NameResolver;
use mago_php_version::PHPVersion;
use mago_span::HasPosition;
use mago_syntax::cst::*;
use mago_syntax::parser::parse_file;
use mago_syntax::walker::Walker;

use crate::graph::ClassNode;
use crate::graph::EdgeKind;
use crate::graph::Method as MethodNode;
use crate::graph::Parameter as ParameterNode;
use crate::graph::Property as PropertyNode;
use crate::graph::Relation;
use crate::graph::Unresolved;
use crate::graph::split_name;

const RELATION_METHODS: &[(&str, &str)] = &[
    ("hasone", "HasOne"),
    ("hasmany", "HasMany"),
    ("belongsto", "BelongsTo"),
    ("belongstomany", "BelongsToMany"),
    ("hasonethrough", "HasOneThrough"),
    ("hasmanythrough", "HasManyThrough"),
    ("morphone", "MorphOne"),
    ("morphmany", "MorphMany"),
    ("morphto", "MorphTo"),
    ("morphtomany", "MorphToMany"),
    ("morphedbymany", "MorphToMany"),
];

pub struct RawEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub label: Option<String>,
    pub file: String,
    pub line: u32,
    pub declared_at: u32,
}

#[derive(Default)]
pub struct FileFacts {
    pub classes: Vec<ClassNode>,
    pub edges: Vec<RawEdge>,
    pub unresolved: Vec<Unresolved>,
    pub has_parse_errors: bool,
}

pub fn collect_file(path: &Path, relative: &str) -> FileFacts {
    let Ok(contents) = std::fs::read(path) else {
        return FileFacts::default();
    };

    let file = File::new(
        Cow::Owned(relative.as_bytes().to_vec()),
        FileType::Host,
        Some(path.to_path_buf()),
        Cow::Owned(contents),
    );

    let arena = LocalArena::new();
    let program = parse_file(&arena, &file);
    let resolved_names = NameResolver::new(&arena).resolve(program);
    let codebase = scan_program(&arena, &file, program, &resolved_names, PHPVersion::LATEST);

    let mut context = WalkContext {
        file: &file,
        relative,
        resolved_names: &resolved_names,
        class_stack: Vec::new(),
        anonymous_depth: 0,
        relation_candidate: None,
        consumed_offsets: HashSet::new(),
        facts: FileFacts { has_parse_errors: program.has_errors(), ..FileFacts::default() },
    };

    EdgeWalker.walk_program(program, &mut context);

    let mut facts = context.facts;

    for class in &mut facts.classes {
        if let Some(metadata) = codebase.class_likes.get(&mago_word::word(class.id.to_ascii_lowercase())) {
            describe_class(class, metadata, &codebase, &file, &mut facts.edges);
        }
    }

    facts
}

struct WalkContext<'a, 'arena> {
    file: &'a File,
    relative: &'a str,
    resolved_names: &'a ResolvedNames<'arena>,
    class_stack: Vec<usize>,
    anonymous_depth: usize,
    relation_candidate: Option<String>,
    consumed_offsets: HashSet<u32>,
    facts: FileFacts,
}

impl WalkContext<'_, '_> {
    fn line(&self, offset: u32) -> u32 {
        self.file.line_number(offset) + 1
    }

    fn current_class(&self) -> Option<&str> {
        self.class_stack.last().map(|index| self.facts.classes[*index].id.as_str())
    }

    fn name_of(&self, identifier: &Identifier<'_>) -> String {
        let name = self.resolved_names.resolve(identifier).unwrap_or_else(|| identifier.value());

        String::from_utf8_lossy(name).trim_start_matches('\\').to_string()
    }

    fn edge(&mut self, to: String, kind: EdgeKind, label: Option<String>, offset: u32) {
        let Some(index) = self.class_stack.last().copied() else {
            return;
        };

        let from = self.facts.classes[index].id.clone();
        let declared_at = self.facts.classes[index].line.unwrap_or_default();
        let line = self.line(offset);

        self.facts.edges.push(RawEdge { from, to, kind, label, file: self.relative.to_string(), line, declared_at });
    }

    fn unresolved(&mut self, kind: &'static str, detail: String, offset: u32) {
        let line = self.line(offset);

        self.facts.unresolved.push(Unresolved { kind, detail, file: self.relative.to_string(), line });
    }

    fn declare(&mut self, name: &LocalIdentifier<'_>, kind: &'static str, modifiers: Option<&Sequence<'_, Modifier<'_>>>) {
        let id = String::from_utf8_lossy(self.resolved_names.resolve(name).unwrap_or(name.value)).into_owned();
        let (namespace, short) = split_name(&id);
        let mut class = ClassNode::external(&id, "app");

        class.name = short.to_string();
        class.namespace = namespace.to_string();
        class.kind = kind;
        class.external = false;
        class.file = Some(self.relative.to_string());
        class.line = Some(self.line(name.span.start.offset));

        if let Some(modifiers) = modifiers {
            class.r#abstract = modifiers.iter().any(|modifier| matches!(modifier, Modifier::Abstract(_)));
            class.r#final = modifiers.iter().any(|modifier| matches!(modifier, Modifier::Final(_)));
        }

        self.facts.classes.push(class);
        self.class_stack.push(self.facts.classes.len() - 1);
    }

    fn inherit(&mut self, types: &TokenSeparatedSequence<'_, Identifier<'_>>, kind: EdgeKind) {
        for identifier in types.iter() {
            let name = self.name_of(identifier);

            self.edge(name, kind, None, identifier.offset());
        }
    }

    /// Methods get their signature edges from mago's codebase metadata; closures and arrow functions are
    /// not in it, so their written hints are resolved here.
    fn signature(&mut self, parameters: &FunctionLikeParameterList<'_>, returns: Option<&FunctionLikeReturnTypeHint<'_>>) {
        let mut identifiers = Vec::new();

        for parameter in parameters.parameters.iter() {
            if let Some(hint) = &parameter.hint {
                hint_identifiers(hint, &mut identifiers);
            }
        }

        if let Some(returns) = returns {
            hint_identifiers(&returns.hint, &mut identifiers);
        }

        for identifier in identifiers {
            let name = self.name_of(identifier);

            self.edge(name, EdgeKind::Signature, None, identifier.offset());
        }
    }

    fn class_expression(&self, expression: &Expression<'_>) -> Option<String> {
        match expression {
            Expression::Identifier(identifier) => Some(self.name_of(identifier)),
            _ => None,
        }
    }
}

struct EdgeWalker;

impl<'ast, 'arena> Walker<'ast, 'arena, WalkContext<'_, 'arena>> for EdgeWalker {
    fn walk_in_class(&self, class: &'ast Class<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.declare(&class.name, "class", Some(&class.modifiers));

        if let Some(extends) = &class.extends {
            context.inherit(&extends.types, EdgeKind::Extends);
        }

        if let Some(implements) = &class.implements {
            context.inherit(&implements.types, EdgeKind::Implements);
        }
    }

    fn walk_out_class(&self, _: &'ast Class<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.class_stack.pop();
    }

    fn walk_in_interface(&self, interface: &'ast Interface<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.declare(&interface.name, "interface", None);

        if let Some(extends) = &interface.extends {
            context.inherit(&extends.types, EdgeKind::Extends);
        }
    }

    fn walk_out_interface(&self, _: &'ast Interface<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.class_stack.pop();
    }

    fn walk_in_trait(&self, r#trait: &'ast Trait<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.declare(&r#trait.name, "trait", None);
    }

    fn walk_out_trait(&self, _: &'ast Trait<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.class_stack.pop();
    }

    fn walk_in_enum(&self, r#enum: &'ast Enum<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.declare(&r#enum.name, "enum", None);

        if let Some(implements) = &r#enum.implements {
            context.inherit(&implements.types, EdgeKind::Implements);
        }
    }

    fn walk_out_enum(&self, _: &'ast Enum<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.class_stack.pop();
    }

    fn walk_in_anonymous_class(&self, class: &'ast AnonymousClass<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.anonymous_depth += 1;

        if let Some(extends) = &class.extends {
            context.inherit(&extends.types, EdgeKind::New);
        }

        if let Some(implements) = &class.implements {
            context.inherit(&implements.types, EdgeKind::New);
        }
    }

    fn walk_out_anonymous_class(&self, _: &'ast AnonymousClass<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.anonymous_depth -= 1;
    }

    fn walk_in_trait_use(&self, trait_use: &'ast TraitUse<'arena>, context: &mut WalkContext<'_, 'arena>) {
        let kind = if context.anonymous_depth > 0 { EdgeKind::New } else { EdgeKind::UsesTrait };

        context.inherit(&trait_use.trait_names, kind);
    }

    fn walk_in_method(&self, method: &'ast Method<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.relation_candidate = method
            .parameter_list
            .parameters
            .is_empty()
            .then(|| String::from_utf8_lossy(method.name.value).into_owned());
    }

    fn walk_out_method(&self, _: &'ast Method<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.relation_candidate = None;
    }

    fn walk_in_instantiation(&self, instantiation: &'ast Instantiation<'arena>, context: &mut WalkContext<'_, 'arena>) {
        match context.class_expression(instantiation.class) {
            Some(name) => context.edge(name, EdgeKind::New, None, instantiation.class.offset()),
            None if !matches!(
                instantiation.class,
                Expression::Static(_) | Expression::Self_(_) | Expression::Parent(_) | Expression::AnonymousClass(_)
            ) =>
            {
                if context.current_class().is_some() {
                    context.unresolved("dynamic-new", "new with a runtime class name".to_string(), instantiation.class.offset());
                }
            }
            None => {}
        }
    }

    fn walk_in_static_method_call(&self, call: &'ast StaticMethodCall<'arena>, context: &mut WalkContext<'_, 'arena>) {
        if let Some(name) = context.class_expression(call.class) {
            context.edge(name, EdgeKind::Static, None, call.class.offset());
        }
    }

    fn walk_in_static_property_access(&self, access: &'ast StaticPropertyAccess<'arena>, context: &mut WalkContext<'_, 'arena>) {
        if let Some(name) = context.class_expression(access.class) {
            context.edge(name, EdgeKind::Static, None, access.class.offset());
        }
    }

    fn walk_in_class_constant_access(&self, access: &'ast ClassConstantAccess<'arena>, context: &mut WalkContext<'_, 'arena>) {
        let Some(name) = context.class_expression(access.class) else {
            return;
        };

        let offset = access.class.offset();

        if context.consumed_offsets.contains(&offset) {
            return;
        }

        let is_class_name = matches!(&access.constant, ClassLikeConstantSelector::Identifier(constant) if constant.value.eq_ignore_ascii_case(b"class"));

        context.edge(name, if is_class_name { EdgeKind::Reference } else { EdgeKind::Static }, None, offset);
    }

    fn walk_in_binary(&self, binary: &'ast Binary<'arena>, context: &mut WalkContext<'_, 'arena>) {
        if !matches!(binary.operator, BinaryOperator::Instanceof(_)) {
            return;
        }

        if let Some(name) = context.class_expression(binary.rhs) {
            context.edge(name, EdgeKind::Reference, None, binary.rhs.offset());
        }
    }

    fn walk_in_closure(&self, closure: &'ast Closure<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.signature(&closure.parameter_list, closure.return_type_hint.as_ref());
    }

    fn walk_in_arrow_function(&self, function: &'ast ArrowFunction<'arena>, context: &mut WalkContext<'_, 'arena>) {
        context.signature(&function.parameter_list, function.return_type_hint.as_ref());
    }

    fn walk_in_try_catch_clause(&self, clause: &'ast TryCatchClause<'arena>, context: &mut WalkContext<'_, 'arena>) {
        let mut identifiers = Vec::new();
        hint_identifiers(&clause.hint, &mut identifiers);

        for identifier in identifiers {
            let name = context.name_of(identifier);
            context.edge(name, EdgeKind::Reference, None, identifier.offset());
        }
    }

    fn walk_in_method_call(&self, call: &'ast MethodCall<'arena>, context: &mut WalkContext<'_, 'arena>) {
        let Expression::Variable(Variable::Direct(variable)) = call.object else {
            return;
        };

        let ClassLikeMemberSelector::Identifier(method) = &call.method else {
            return;
        };

        if variable.name != b"$this" {
            return;
        }

        let called = String::from_utf8_lossy(method.value).to_ascii_lowercase();

        let Some((_, relation_type)) = RELATION_METHODS.iter().find(|(name, _)| *name == called) else {
            return;
        };

        let Some(relation_name) = context.relation_candidate.clone() else {
            return;
        };

        let related = call.argument_list.arguments.first().and_then(|argument| {
            let Argument::Positional(argument) = argument else {
                return None;
            };

            let Expression::Access(Access::ClassConstant(access)) = argument.value else {
                return None;
            };

            let name = context.class_expression(access.class)?;
            context.consumed_offsets.insert(access.class.offset());

            Some(name)
        });

        let line = context.line(call.object.offset());

        if let Some(index) = context.class_stack.last().copied() {
            context.facts.classes[index].relations.push(Relation {
                name: relation_name.clone(),
                r#type: relation_type.to_string(),
                related: related.clone(),
                line,
            });
        }

        match related {
            Some(related) => {
                let label = format!("{relation_name}(): {relation_type}");
                context.edge(related, EdgeKind::Relation, Some(label), call.object.offset());
            }
            None if *relation_type != "MorphTo" => {
                context.unresolved(
                    "relation",
                    format!("{relation_name}(): {relation_type} without a literal ::class target"),
                    call.object.offset(),
                );
            }
            None => {}
        }
    }
}

fn hint_identifiers<'ast, 'arena>(hint: &'ast Hint<'arena>, identifiers: &mut Vec<&'ast Identifier<'arena>>) {
    match hint {
        Hint::Identifier(identifier) => identifiers.push(identifier),
        Hint::Parenthesized(hint) => hint_identifiers(hint.hint, identifiers),
        Hint::Nullable(hint) => hint_identifiers(hint.hint, identifiers),
        Hint::Union(hint) => {
            hint_identifiers(hint.left, identifiers);
            hint_identifiers(hint.right, identifiers);
        }
        Hint::Intersection(hint) => {
            hint_identifiers(hint.left, identifiers);
            hint_identifiers(hint.right, identifiers);
        }
        _ => {}
    }
}

fn describe_class(
    class: &mut ClassNode,
    metadata: &ClassLikeMetadata,
    codebase: &CodebaseMetadata,
    file: &File,
    edges: &mut Vec<RawEdge>,
) {
    let line_of = |type_metadata: &TypeMetadata| file.line_number(type_metadata.span.start.offset) + 1;
    let written = |type_metadata: &TypeMetadata| written_type(file, type_metadata);
    let relative = String::from_utf8_lossy(&file.name).into_owned();

    class.kind = match metadata.kind {
        SymbolKind::Class => "class",
        SymbolKind::Interface => "interface",
        SymbolKind::Trait => "trait",
        SymbolKind::Enum => "enum",
    };

    class.r#abstract |= metadata.flags.is_abstract() && metadata.kind == SymbolKind::Class;
    class.r#final |= metadata.flags.is_final();

    let mut constants: Vec<_> = metadata.constants.values().map(|constant| constant.name.to_string()).collect();
    constants.sort();
    class.constants = constants;

    let mut cases: Vec<_> = metadata.enum_cases.values().map(|case| case.name.to_string()).collect();
    cases.sort();
    class.cases = cases;

    let mut type_edge = |to: String, kind: EdgeKind, line: u32| {
        edges.push(RawEdge {
            from: class.id.clone(),
            to,
            kind,
            label: None,
            file: relative.clone(),
            line,
            declared_at: class.line.unwrap_or_default(),
        });
    };

    let mut properties: Vec<_> = metadata.properties.values().collect();
    properties.sort_by_key(|property| property.span.map(|span| span.start.offset));

    for property in properties {
        let type_metadata = property.type_metadata.as_ref().or(property.type_declaration_metadata.as_ref());

        if let Some(type_metadata) = type_metadata {
            for name in class_names(&type_metadata.type_union) {
                type_edge(name, EdgeKind::Property, line_of(type_metadata));
            }
        }

        class.properties.push(PropertyNode {
            name: property.name.0.to_string().trim_start_matches('$').to_string(),
            visibility: visibility(property.read_visibility),
            r#type: type_metadata.map(written),
            r#static: property.flags.is_static(),
            readonly: property.flags.is_readonly(),
        });
    }

    let mut methods: Vec<&FunctionLikeMetadata> = codebase
        .function_likes
        .iter()
        .filter(|((scope, _), function)| *scope == metadata.name && function.method_metadata.is_some())
        .map(|(_, function)| function.as_ref())
        .collect();

    methods.sort_by_key(|method| method.span.start.offset);

    for method in methods {
        let method_metadata = method.method_metadata.as_ref().expect("filtered on method metadata");
        let parameter_kind = if method_metadata.is_constructor { EdgeKind::Injects } else { EdgeKind::Signature };

        let params = method
            .parameters
            .iter()
            .map(|parameter| {
                let type_metadata = parameter.type_metadata.as_ref().or(parameter.type_declaration_metadata.as_ref());

                if let Some(type_metadata) = type_metadata {
                    for name in class_names(&type_metadata.type_union) {
                        type_edge(name, parameter_kind, line_of(type_metadata));
                    }
                }

                ParameterNode {
                    name: parameter.name.0.to_string().trim_start_matches('$').to_string(),
                    r#type: type_metadata.map(written),
                }
            })
            .collect();

        let returns = method.return_type_metadata.as_ref().or(method.return_type_declaration_metadata.as_ref());

        if let Some(type_metadata) = returns {
            for name in class_names(&type_metadata.type_union) {
                type_edge(name, EdgeKind::Signature, line_of(type_metadata));
            }
        }

        class.methods.push(MethodNode {
            name: method.original_name.to_string(),
            visibility: visibility(method_metadata.visibility),
            r#static: method_metadata.is_static,
            r#abstract: method_metadata.is_abstract,
            params,
            returns: returns.map(written),
            line: file.line_number(method.span.start.offset) + 1,
        });
    }
}

/// Members are drawn with their types as written in the source (`?OrderStatus`, `list<Product>`), since
/// mago's type ids are internal (`unknown-ref(App\\Models\\User)`); edges carry the resolved classes.
fn written_type(file: &File, type_metadata: &TypeMetadata) -> String {
    let span = type_metadata.span;
    let text = String::from_utf8_lossy(&file.contents[span.start.offset as usize..span.end.offset as usize]);

    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn class_names(union: &TUnion) -> Vec<String> {
    let mut names = Vec::new();

    for node in union.get_all_child_nodes() {
        let TypeRef::Atomic(atomic) = node else {
            continue;
        };

        let name = match atomic {
            TAtomic::Object(TObject::Named(named)) if !named.is_this && !named.is_static => named.name,
            TAtomic::Object(TObject::Enum(r#enum)) => r#enum.name,
            TAtomic::Reference(TReference::Symbol { name, .. }) => *name,
            TAtomic::Reference(TReference::Member { class_like_name, .. }) => *class_like_name,
            _ => continue,
        };

        names.push(name.to_string().trim_start_matches('\\').to_string());
    }

    names
}

fn visibility(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}
