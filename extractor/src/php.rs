//! Reading the values Laravel's configuration code is written in: string literals, `Foo::class`, arrays
//! of them and the path expressions route files are loaded with. Anything else is not a value puml can
//! know without running the code, and the callers report it as unresolved.

use mago_database::file::File;
use mago_names::ResolvedNames;
use mago_span::HasPosition;
use mago_span::HasSpan;
use mago_syntax::cst::*;

pub struct Source<'a, 'arena> {
    pub file: &'a File,
    pub relative: &'a str,
    pub names: &'a ResolvedNames<'arena>,
}

impl<'arena> Source<'_, 'arena> {
    pub fn line(&self, offset: u32) -> u32 {
        self.file.line_number(offset) + 1
    }

    pub fn name_of(&self, identifier: &Identifier<'_>) -> String {
        let name = self.names.resolve(identifier).unwrap_or_else(|| identifier.value());

        String::from_utf8_lossy(name).trim_start_matches('\\').to_string()
    }

    pub fn text(&self, span: impl HasSpan) -> String {
        let span = span.span();

        String::from_utf8_lossy(&self.file.contents[span.start.offset as usize..span.end.offset as usize]).into_owned()
    }

    /// `Foo::class`, resolved against the file's imports.
    pub fn class_constant(&self, expression: &Expression<'_>) -> Option<String> {
        let Expression::Access(Access::ClassConstant(access)) = expression else {
            return None;
        };

        let ClassLikeConstantSelector::Identifier(constant) = &access.constant else {
            return None;
        };

        if !constant.value.eq_ignore_ascii_case(b"class") {
            return None;
        }

        match access.class {
            Expression::Identifier(identifier) => Some(self.name_of(identifier)),
            _ => None,
        }
    }

    /// A class written as `Foo::class` or as a string holding its name.
    pub fn class_name(&self, expression: &Expression<'_>) -> Option<String> {
        self.class_constant(expression)
            .or_else(|| self.string(expression).map(|name| name.trim_start_matches('\\').to_string()))
    }

    /// A string literal, or literals joined with `.`.
    pub fn string(&self, expression: &Expression<'_>) -> Option<String> {
        match expression {
            Expression::Literal(Literal::String(string)) => string.value.map(|value| String::from_utf8_lossy(value).into_owned()),
            Expression::Parenthesized(inner) => self.string(inner.expression),
            Expression::Binary(binary) if matches!(binary.operator, BinaryOperator::StringConcat(_)) => {
                Some(self.string(binary.lhs)? + &self.string(binary.rhs)?)
            }
            _ => None,
        }
    }

    /// A string, a class name, or an array of them, as middleware and listener lists are written.
    pub fn strings(&self, expression: &Expression<'_>) -> Option<Vec<String>> {
        match array_elements(expression) {
            Some(elements) => elements.iter().map(|element| self.class_name(element.value()?)).collect(),
            None => self.class_name(expression).map(|value| vec![value]),
        }
    }

    /// A file path relative to the project root: a string, `base_path('…')`, `app_path('…')` or
    /// `__DIR__.'/…'`, with `..` folded away.
    pub fn path(&self, expression: &Expression<'_>) -> Option<String> {
        let raw = self.path_text(expression)?;
        let mut parts: Vec<&str> = Vec::new();

        for part in raw.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop()?;
                }
                part => parts.push(part),
            }
        }

        Some(parts.join("/"))
    }

    fn path_text(&self, expression: &Expression<'_>) -> Option<String> {
        match expression {
            Expression::Literal(Literal::String(_)) => self.string(expression),
            Expression::Parenthesized(inner) => self.path_text(inner.expression),
            Expression::MagicConstant(MagicConstant::Directory(_)) => Some(self.directory()),
            Expression::Binary(binary) if matches!(binary.operator, BinaryOperator::StringConcat(_)) => {
                Some(self.path_text(binary.lhs)? + &self.string(binary.rhs)?)
            }
            Expression::Call(Call::Function(call)) => {
                let Expression::Identifier(function) = call.function else {
                    return None;
                };

                let name = String::from_utf8_lossy(function.value()).to_ascii_lowercase();
                let argument = argument(&call.argument_list, 0, "path");
                let inner = || argument.map_or(Some(String::new()), |argument| self.path_text(argument));

                match name.trim_start_matches('\\') {
                    "base_path" => inner(),
                    "app_path" => Some(format!("app/{}", inner()?)),
                    "dirname" => {
                        let path = self.path_text(argument?)?;

                        Some(path.rsplit_once('/').map_or_else(String::new, |(parent, _)| parent.to_string()))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn directory(&self) -> String {
        self.relative.rsplit_once('/').map_or_else(String::new, |(directory, _)| directory.to_string())
    }
}

pub trait ElementValue<'arena> {
    fn value(&self) -> Option<&Expression<'arena>>;
}

impl<'arena> ElementValue<'arena> for ArrayElement<'arena> {
    fn value(&self) -> Option<&Expression<'arena>> {
        match self {
            ArrayElement::Value(element) => Some(element.value),
            _ => None,
        }
    }
}

pub fn array_elements<'ast, 'arena>(expression: &'ast Expression<'arena>) -> Option<Vec<&'ast ArrayElement<'arena>>> {
    match expression {
        Expression::Array(array) => Some(array.elements.iter().collect()),
        Expression::LegacyArray(array) => Some(array.elements.iter().collect()),
        _ => None,
    }
}

/// The key-value pairs of an array literal, keys as written.
pub fn array_pairs<'ast, 'arena>(expression: &'ast Expression<'arena>) -> Option<Vec<(&'ast Expression<'arena>, &'ast Expression<'arena>)>> {
    Some(
        array_elements(expression)?
            .into_iter()
            .filter_map(|element| match element {
                ArrayElement::KeyValue(pair) => Some((pair.key, pair.value)),
                _ => None,
            })
            .collect(),
    )
}

/// The argument at `index`, or the named argument `name`.
pub fn argument<'ast, 'arena>(list: &'ast ArgumentList<'arena>, index: usize, name: &str) -> Option<&'ast Expression<'arena>> {
    let mut position = 0;

    for argument in list.arguments.iter() {
        match argument {
            Argument::Named(named) if named.name.value == name.as_bytes() => return Some(named.value),
            Argument::Positional(positional) if positional.ellipsis.is_none() => {
                if position == index {
                    return Some(positional.value);
                }

                position += 1;
            }
            _ => {}
        }
    }

    None
}

pub fn positional<'ast, 'arena>(list: &'ast ArgumentList<'arena>) -> Vec<&'ast Expression<'arena>> {
    list.arguments
        .iter()
        .filter_map(|argument| match argument {
            Argument::Positional(positional) if positional.ellipsis.is_none() => Some(positional.value),
            _ => None,
        })
        .collect()
}

pub fn method_name(selector: &ClassLikeMemberSelector<'_>) -> Option<String> {
    match selector {
        ClassLikeMemberSelector::Identifier(identifier) => Some(String::from_utf8_lossy(identifier.value).into_owned()),
        _ => None,
    }
}

/// One call in a chain such as `Route::prefix('forum')->name('forum.')->group(...)`.
pub struct Link<'ast, 'arena> {
    pub method: String,
    pub arguments: &'ast ArgumentList<'arena>,
    pub offset: u32,
}

pub enum Base {
    /// `Route::…`; the first link is the static call.
    Static(String),
    /// `$events->…`
    Variable(String),
    /// `$this->app->…`
    ThisProperty(String),
    /// `$this->…`
    This,
    /// `app()->…`
    Function(String),
}

/// Flattens a method chain into its base and its calls, first call first.
pub fn chain<'ast, 'arena>(source: &Source<'_, 'arena>, expression: &'ast Expression<'arena>) -> Option<(Base, Vec<Link<'ast, 'arena>>)> {
    match expression {
        Expression::Parenthesized(inner) => chain(source, inner.expression),
        Expression::Call(Call::Method(call)) => {
            let (base, mut links) = chain(source, call.object)?;
            links.push(Link { method: method_name(&call.method)?, arguments: &call.argument_list, offset: call.method.offset() });

            Some((base, links))
        }
        Expression::Call(Call::NullSafeMethod(call)) => {
            let (base, mut links) = chain(source, call.object)?;
            links.push(Link { method: method_name(&call.method)?, arguments: &call.argument_list, offset: call.method.offset() });

            Some((base, links))
        }
        Expression::Call(Call::StaticMethod(call)) => {
            let class = match call.class {
                Expression::Identifier(identifier) => source.name_of(identifier),
                Expression::Static(_) => "static".to_string(),
                Expression::Self_(_) => "self".to_string(),
                _ => return None,
            };

            Some((
                Base::Static(class),
                vec![Link { method: method_name(&call.method)?, arguments: &call.argument_list, offset: call.class.offset() }],
            ))
        }
        Expression::Call(Call::Function(call)) => {
            let Expression::Identifier(function) = call.function else {
                return None;
            };

            if !call.argument_list.arguments.is_empty() {
                return None;
            }

            let name = String::from_utf8_lossy(function.value()).trim_start_matches('\\').to_ascii_lowercase();

            Some((Base::Function(name), Vec::new()))
        }
        Expression::Variable(Variable::Direct(variable)) if variable.name == b"$this" => Some((Base::This, Vec::new())),
        Expression::Variable(Variable::Direct(variable)) => {
            Some((Base::Variable(String::from_utf8_lossy(variable.name).into_owned()), Vec::new()))
        }
        Expression::Access(Access::Property(access)) => {
            let Expression::Variable(Variable::Direct(variable)) = access.object else {
                return None;
            };

            if variable.name != b"$this" {
                return None;
            }

            Some((Base::ThisProperty(method_name(&access.property)?), Vec::new()))
        }
        _ => None,
    }
}

pub fn is_facade(name: &str, facade: &str) -> bool {
    name.eq_ignore_ascii_case(facade) || name.eq_ignore_ascii_case(&format!("Illuminate\\Support\\Facades\\{facade}"))
}
