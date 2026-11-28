use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mago_allocator::{Arena, LocalArena};
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::array::TArray;
use mago_codex::ttype::atomic::array::key::ArrayKey;
use mago_codex::ttype::atomic::array::keyed::KnownItems;
use mago_codex::ttype::atomic::array::keyed::TKeyedArray;
use mago_codex::ttype::atomic::array::list::TList;
use mago_codex::ttype::atomic::callable::TCallable;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_codex::ttype::atomic::scalar::class_like_string::TClassLikeString;
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::{
    get_empty_keyed_array, get_false, get_literal_float, get_literal_int, get_literal_string,
    get_null, get_true,
};
use mago_database::file::FileType;
use mago_names::ResolvedNames;
use mago_names::resolver::NameResolver;
use mago_span::HasSpan;
use mago_syntax::cst::*;
use mago_syntax::parser::parse_file;
use mago_syntax::walker::MutWalker;
use pun_phpstan::phpdoc::{self, ConstExpr, Node, ShapeKey};
use pun_phpstan::resolve::{Scope, resolve_in, resolve_late};
use pun_phpstan::types::{LateArrayShape, Type, mixed};

use crate::codebase::read;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) enum IsArray {
    No,
    Maybe,
}

#[derive(Clone, Debug, serde::Serialize)]
enum Returned {
    Doc(#[serde(serialize_with = "debug_text")] Node, Option<TUnion>),
    Array(Vec<Item>, Option<TUnion>),
    Other,
}

#[derive(Clone, Debug, serde::Serialize)]
struct Item {
    key: Option<Option<String>>,
    value: Value,
    ty: Option<TUnion>,
}

#[derive(Clone, Debug, serde::Serialize)]
enum Value {
    Array(Vec<Item>),
    Other(IsArray),
}

type ParsedTypes =
    BTreeMap<(mago_database::file::FileId, String), (IsArray, Arc<mago_codex::reads::Reads>)>;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub(crate) struct Configs {
    #[serde(serialize_with = "crate::fingerprint::path_keyed")]
    files: BTreeMap<PathBuf, Option<Returned>>,
    /// A cache the checks fill: not part of what the configs say. Keyed by the asking file too, because a doc type
    /// resolves in the asker's scope: shared across files, the value would depend on which file the run analyses
    /// first, so a cached run, or one with more processes, would get another one. Each value keeps what computing it
    /// read (`mago_codex::reads`), replayed by every later call of that file.
    #[serde(skip)]
    parsed: Arc<Mutex<ParsedTypes>>,
}

/// A PHPDoc node, which has no `Serialize`, as its `Debug` text: enough for the fingerprint of the shared facts.
fn debug_text<S: serde::Serializer>(node: &Node, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{node:?}"))
}

impl Configs {
    pub(crate) fn load(
        directories: &[String],
        root: &Path,
        codebase: &CodebaseMetadata,
        generalize_env: bool,
    ) -> Configs {
        let php = ["php".to_string()];
        let mut paths: Vec<PathBuf> = directories
            .iter()
            .flat_map(|d| crate::php_files(Path::new(d), &php))
            .collect();
        if paths.is_empty() {
            paths = crate::php_files(&root.join("config"), &php);
        }
        let reader = Reader {
            codebase,
            generalize_env,
        };
        Configs {
            files: paths
                .into_iter()
                .map(|p| {
                    let returned = reader.returned(&p);
                    (p, returned)
                })
                .collect(),
            parsed: Default::default(),
        }
    }

    fn file(&self, key: &str) -> Option<&Returned> {
        let suffix = format!("{}.php", key.split('.').next().unwrap_or_default());
        self.files
            .iter()
            .filter(|(p, _)| p.to_string_lossy().ends_with(&suffix))
            .find_map(|(_, r)| r.as_ref())
    }

    /// The types of `keys`, a doc-typed one resolved in `scope` (whose building read `scope_reads`), the scope of
    /// `caller`'s first call: the value is memoized for `caller`'s later calls, which replay what computing it read.
    pub(crate) fn types(
        &self,
        keys: &[String],
        scope: &dyn Scope,
        scope_reads: &mago_codex::reads::Reads,
        caller: mago_database::file::FileId,
    ) -> Option<Vec<IsArray>> {
        use mago_codex::reads;
        let mut types = Vec::new();
        let mut parsed = self.parsed.lock().unwrap();
        for key in keys {
            let memo = (caller, key.clone());
            if let Some((ty, read)) = parsed.get(&memo) {
                reads::replay(read);
                types.push(*ty);
                continue;
            }
            let parts: Vec<&str> = key.split('.').skip(1).collect();
            types.extend(match self.file(key)? {
                Returned::Doc(node, _) => {
                    let (ty, mut read) = reads::recording(|| doc_type(node, &parts, scope));
                    read.extend(scope_reads);
                    parsed.insert(memo, (ty, Arc::new(read)));
                    Some(ty)
                }
                Returned::Array(..) if parts.is_empty() => {
                    // always `Maybe`: no scope and no symbol decides it
                    parsed.insert(memo, (IsArray::Maybe, Arc::default()));
                    Some(IsArray::Maybe)
                }
                Returned::Array(items, _) => lookup(items, &parts).map(|item| match item.value {
                    Value::Array(_) => IsArray::Maybe,
                    Value::Other(t) => t,
                }),
                Returned::Other => None,
            });
        }
        Some(types)
    }

    pub(crate) fn value_types(&self, keys: &[String]) -> Option<Option<Vec<TUnion>>> {
        let mut types = Vec::new();
        for key in keys {
            let parts: Vec<&str> = key.split('.').skip(1).collect();
            let ty = match self.file(key)? {
                Returned::Doc(_, ty) => {
                    let Some(mut ty) = ty.clone() else {
                        return Some(None);
                    };
                    for part in &parts {
                        match offset(&ty, part) {
                            Some(value) => ty = value,
                            None => return Some(None),
                        }
                    }
                    Some(ty)
                }
                Returned::Array(_, ty) if parts.is_empty() => match ty {
                    Some(ty) => Some(ty.clone()),
                    None => return Some(None),
                },
                Returned::Array(items, _) => match lookup(items, &parts) {
                    Some(Item { ty: None, .. }) => return Some(None),
                    item => item.and_then(|item| item.ty.clone()),
                },
                Returned::Other => None,
            };
            types.extend(ty);
        }
        Some(Some(types))
    }
}

fn offset(ty: &TUnion, part: &str) -> Option<TUnion> {
    let key = match pun_phpstan::value::numeric(part) {
        Some(_) => ArrayKey::Integer(pun_tokens::php_compat::intval(part)),
        None => ArrayKey::String(mago_word::word(part)),
    };
    match ty.types.as_ref() {
        [TAtomic::Array(TArray::Keyed(k))] => k
            .known_items
            .as_ref()?
            .get(&key)
            .filter(|(optional, _)| !optional)
            .map(|(_, t)| t.clone()),
        [TAtomic::Array(TArray::List(l))] => match key {
            ArrayKey::Integer(i) => l
                .known_elements
                .as_ref()?
                .get(&usize::try_from(i).ok()?)
                .filter(|(optional, _)| !optional)
                .map(|(_, t)| t.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn lookup<'i>(items: &'i [Item], parts: &[&str]) -> Option<&'i Item> {
    let mut items = items;
    let mut found = None;
    for part in parts {
        let current = items;
        for item in current {
            let key = item.key.as_ref()?;
            if key.as_deref() != Some(*part) {
                continue;
            }
            if let Value::Array(nested) = &item.value {
                items = nested;
            }
            found = Some(item);
        }
    }
    found
}

fn doc_type(node: &Node, parts: &[&str], scope: &dyn Scope) -> IsArray {
    let mut ty = doc_shape(node, scope);
    for part in parts {
        let offset = if pun_phpstan::value::numeric(part).is_some() {
            Type::ConstInt(pun_tokens::php_compat::intval(part))
        } else {
            Type::ConstString(part.as_bytes().to_vec())
        };
        ty = resolve_late(&Type::OffsetAccess(Box::new(ty), Box::new(offset)));
    }
    if ty.is_array().no() {
        IsArray::No
    } else {
        IsArray::Maybe
    }
}

fn doc_shape(node: &Node, scope: &dyn Scope) -> Type {
    let Node::ArrayShape {
        kind,
        items,
        sealed,
        unsealed,
    } = node
    else {
        return resolve_in(node, scope);
    };
    LateArrayShape {
        items: items
            .iter()
            .map(|item| {
                let key = item.key.as_ref().map(|key| match key {
                    ShapeKey::Ident(name) => scope
                        .template(name)
                        .unwrap_or_else(|| Type::ConstString(name.as_bytes().to_vec())),
                    ShapeKey::Str(value) => Type::ConstString(value.clone()),
                    ShapeKey::Int(value) => {
                        resolve_in(&Node::Const(ConstExpr::Int(value.clone())), scope)
                    }
                    ShapeKey::Fetch(class, name) => resolve_in(
                        &Node::Const(ConstExpr::Fetch(class.clone(), name.clone())),
                        scope,
                    ),
                });
                (key, doc_shape(&item.value, scope), item.optional)
            })
            .collect(),
        unsealed: (!sealed).then(|| {
            unsealed.as_ref().map_or_else(
                || (None, mixed()),
                |(key, value)| {
                    (
                        key.as_ref().map(|key| resolve_in(key, scope)),
                        doc_shape(value, scope),
                    )
                },
            )
        }),
        kind: kind.clone(),
    }
    .create()
}

struct Reader<'c> {
    codebase: &'c CodebaseMetadata,
    generalize_env: bool,
}

impl Reader<'_> {
    fn returned(&self, path: &Path) -> Option<Returned> {
        let file = read(path, FileType::Host).ok()??;
        let arena = LocalArena::new();
        let program = parse_file(&arena, &file);
        if program.has_errors() {
            return None;
        }
        let names = NameResolver::new(&arena).resolve(program);
        let mut first = FirstReturn(None);
        first.walk_program(program, &mut ());
        let r = first.0?;
        let comments = crate::rules::Comments::of(&file.contents, true);
        if let Some((_, entry)) =
            comments.doc_before(&file.contents, r.span().start.offset as usize)
            && let Some((_, ty)) = entry
                .tags
                .iter()
                .find(|(tag, _)| tag == "@return" || tag == "@phpstan-return")
            && let Ok((node, end)) = phpdoc::parse_prefix(ty)
        {
            return Some(Returned::Doc(
                node,
                type_from_text(ty.get(..end).unwrap_or(ty)),
            ));
        }
        Some(match r.value {
            Some(Expression::Array(a)) => Returned::Array(
                self.items(&a.elements, &names),
                self.array_type(&a.elements, &names),
            ),
            Some(Expression::LegacyArray(a)) => Returned::Array(
                self.items(&a.elements, &names),
                self.array_type(&a.elements, &names),
            ),
            _ => Returned::Other,
        })
    }

    fn items(
        &self,
        elements: &TokenSeparatedSequence<ArrayElement>,
        names: &ResolvedNames,
    ) -> Vec<Item> {
        elements
            .iter()
            .map(|e| match e {
                ArrayElement::KeyValue(kv) => Item {
                    key: match kv.key {
                        Expression::Literal(Literal::String(s)) => Some(Some(
                            String::from_utf8_lossy(s.value.unwrap_or_default()).into_owned(),
                        )),
                        Expression::Literal(Literal::Integer(i)) => {
                            Some(i.value.map(|v| v.to_string()))
                        }
                        Expression::Literal(Literal::Float(f)) => {
                            Some(Some(pun_phpstan::types::float_to_string(*f.value)))
                        }
                        Expression::MagicConstant(_) | Expression::CompositeString(_) => Some(None),
                        _ => None,
                    },
                    value: self.value(kv.value, names),
                    ty: self.type_of(kv.value, names),
                },
                ArrayElement::Value(v) => Item {
                    key: None,
                    value: self.value(v.value, names),
                    ty: self.type_of(v.value, names),
                },
                _ => Item {
                    key: None,
                    value: Value::Other(IsArray::Maybe),
                    ty: None,
                },
            })
            .collect()
    }

    fn value(&self, e: &Expression, names: &ResolvedNames) -> Value {
        match e {
            Expression::Array(a) => Value::Array(self.items(&a.elements, names)),
            Expression::LegacyArray(a) => Value::Array(self.items(&a.elements, names)),
            e => Value::Other(is_array(e, names, self.codebase)),
        }
    }

    fn type_of(&self, e: &Expression, names: &ResolvedNames) -> Option<TUnion> {
        Some(match e {
            Expression::Parenthesized(p) => return self.type_of(p.expression, names),
            Expression::Literal(Literal::String(s)) => {
                let value = s.value?;
                get_literal_string(mago_word::word(value))
            }
            Expression::Literal(Literal::Integer(i)) => match i.value {
                Some(value) if i64::try_from(value).is_ok() => get_literal_int(value as i64),
                _ => return None,
            },
            Expression::Literal(Literal::Float(f)) => get_literal_float(*f.value),
            Expression::Literal(Literal::True(_)) => get_true(),
            Expression::Literal(Literal::False(_)) => get_false(),
            Expression::Literal(Literal::Null(_)) => get_null(),
            Expression::Array(a) => return self.array_type(&a.elements, names),
            Expression::LegacyArray(a) => return self.array_type(&a.elements, names),
            Expression::Access(Access::ClassConstant(c)) => match (&c.class, &c.constant) {
                (Expression::Identifier(class), ClassLikeConstantSelector::Identifier(i))
                    if i.value.eq_ignore_ascii_case(b"class") =>
                {
                    let name = names.get(&class.span());
                    if [&b"self"[..], b"static", b"parent"]
                        .iter()
                        .any(|n| name.eq_ignore_ascii_case(n))
                    {
                        return None;
                    }
                    TUnion::from_atomic(TAtomic::Scalar(TScalar::ClassLikeString(
                        TClassLikeString::Literal {
                            value: mago_word::word(name),
                        },
                    )))
                }
                _ => return None,
            },
            Expression::Call(Call::Function(f)) => {
                let Expression::Identifier(id) = f.function else {
                    return None;
                };
                let name = names.get(&id.span());
                let short = name.rsplit(|b| *b == b'\\').next().unwrap_or(name);
                let arguments = &f.argument_list.arguments;
                if short.eq_ignore_ascii_case(b"env") {
                    if arguments
                        .iter()
                        .any(|a| !matches!(a, Argument::Positional(_)))
                    {
                        return None;
                    }
                    let default = match arguments.get(1) {
                        Some(d) => self.type_of(d.value(), names)?,
                        None => get_null(),
                    };
                    if self.generalize_env && arguments.len() >= 2 {
                        return generalize(&default);
                    }
                    let mut types = vec![
                        TAtomic::Scalar(TScalar::bool()),
                        TAtomic::Scalar(TScalar::string()),
                    ];
                    types.extend(default.types.iter().cloned());
                    return Some(TUnion::from_vec(mago_codex::ttype::combiner::combine(
                        types,
                        self.codebase,
                        Default::default(),
                    )));
                }
                let function = self
                    .codebase
                    .get_function(name)
                    .or_else(|| self.codebase.get_function(short))?;
                if !function.template_types.is_empty() {
                    return None;
                }
                function.return_type_metadata.as_ref()?.type_union.clone()
            }
            _ => return None,
        })
    }

    fn array_type(
        &self,
        elements: &TokenSeparatedSequence<ArrayElement>,
        names: &ResolvedNames,
    ) -> Option<TUnion> {
        let mut items = KnownItems::default();
        let mut next = 0i64;
        for element in elements {
            let (key, value) = match element {
                ArrayElement::KeyValue(item) => {
                    let key = match item.key {
                        Expression::Literal(Literal::Integer(i)) => {
                            ArrayKey::Integer(i64::try_from(i.value?).ok()?)
                        }
                        Expression::Literal(Literal::String(s)) => {
                            let value = s.value?;
                            match std::str::from_utf8(value)
                                .ok()
                                .and_then(|v| v.parse::<i64>().ok())
                            {
                                Some(i) if i.to_string().as_bytes() == value => {
                                    ArrayKey::Integer(i)
                                }
                                _ => ArrayKey::String(mago_word::word(value)),
                            }
                        }
                        _ => return None,
                    };
                    (key, item.value)
                }
                ArrayElement::Value(item) => (ArrayKey::Integer(next), item.value),
                _ => return None,
            };
            if let ArrayKey::Integer(index) = key {
                next = next.max(index.checked_add(1)?);
            }
            items.insert(key, (false, self.type_of(value, names)?));
        }
        if items.is_empty() {
            return Some(get_empty_keyed_array());
        }
        if items
            .keys()
            .enumerate()
            .all(|(index, key)| *key == ArrayKey::Integer(index as i64))
        {
            return Some(TUnion::from_atomic(TAtomic::Array(TArray::List(
                TList::from_known_elements(
                    items
                        .into_iter()
                        .enumerate()
                        .map(|(index, (_, item))| (index, item))
                        .collect(),
                ),
            ))));
        }
        Some(TUnion::from_atomic(TAtomic::Array(TArray::Keyed(
            TKeyedArray::new()
                .with_known_items(items)
                .with_non_empty(true),
        ))))
    }
}

fn generalize(t: &TUnion) -> Option<TUnion> {
    let types: Option<Vec<TAtomic>> = t
        .types
        .iter()
        .map(|a| match a {
            TAtomic::Scalar(TScalar::Integer(_)) => Some(TAtomic::Scalar(TScalar::int())),
            TAtomic::Scalar(TScalar::String(_)) => Some(TAtomic::Scalar(TScalar::string())),
            TAtomic::Scalar(TScalar::Float(_)) => Some(TAtomic::Scalar(TScalar::float())),
            TAtomic::Scalar(TScalar::Bool(_)) => Some(TAtomic::Scalar(TScalar::bool())),
            TAtomic::Null => Some(TAtomic::Null),
            _ => None,
        })
        .collect();
    let mut types = types?;
    types.dedup();
    Some(TUnion::from_vec(types))
}

pub(crate) fn type_from_text(ty: &[u8]) -> Option<TUnion> {
    type_from_text_in(ty, None).filter(|t| {
        t.types.iter().all(|a| {
            !matches!(
                a,
                TAtomic::GenericParameter(_) | TAtomic::Alias(_) | TAtomic::Reference(_)
            )
        })
    })
}

/// The type text as Mago reads a PHPDoc type: a class name stays a symbol reference, and `self` and `static` name
/// `class` (`static` marked static) when it is given.
pub(crate) fn type_from_text_in(ty: &[u8], class: Option<&str>) -> Option<TUnion> {
    let arena = LocalArena::new();
    let content = arena.alloc_slice_copy(ty);
    let span = mago_span::Span::new(
        mago_database::file::FileId::zero(),
        mago_span::Position::new(0),
        mago_span::Position::new(ty.len() as u32),
    );
    let parsed = mago_phpdoc_syntax::parser::parse_type(&arena, content, span).ok()?;
    mago_codex::ttype::builder::get_union_from_type(
        &parsed,
        &mago_names::scope::NamespaceScope::default(),
        &mago_codex::ttype::resolution::TypeResolutionContext::new(),
        class.map(mago_word::word),
    )
    .ok()
}

struct FirstReturn<'ast, 'arena>(Option<&'ast Return<'arena>>);

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for FirstReturn<'ast, 'arena> {
    fn walk_in_return(&mut self, r: &'ast Return<'arena>, (): &mut ()) {
        self.0.get_or_insert(r);
    }
}

fn is_array(e: &Expression, names: &ResolvedNames, codebase: &CodebaseMetadata) -> IsArray {
    let no = match e {
        Expression::Literal(_) | Expression::CompositeString(_) => true,
        Expression::Binary(b) => b.operator.is_concatenation(),
        Expression::Access(Access::ClassConstant(c)) => {
            matches!(&c.constant, ClassLikeConstantSelector::Identifier(i) if i.value.eq_ignore_ascii_case(b"class"))
        }
        Expression::UnaryPrefix(u) => matches!(
            u.operator,
            UnaryPrefixOperator::StringCast(..)
                | UnaryPrefixOperator::IntCast(..)
                | UnaryPrefixOperator::IntegerCast(..)
                | UnaryPrefixOperator::FloatCast(..)
                | UnaryPrefixOperator::DoubleCast(..)
                | UnaryPrefixOperator::RealCast(..)
                | UnaryPrefixOperator::BoolCast(..)
                | UnaryPrefixOperator::BooleanCast(..)
                | UnaryPrefixOperator::Not(..)
        ),
        Expression::Call(Call::Function(f)) => match f.function {
            Expression::Identifier(id) => {
                let name = names.get(&id.span());
                let short = name.rsplit(|b| *b == b'\\').next().unwrap_or(name);
                if short.eq_ignore_ascii_case(b"env") {
                    f.argument_list
                        .arguments
                        .get(1)
                        .is_none_or(|d| is_array(d.value(), names, codebase) == IsArray::No)
                } else {
                    codebase
                        .get_function(name)
                        .or_else(|| codebase.get_function(short))
                        .and_then(|f| f.return_type_metadata.as_ref())
                        .is_some_and(|t| union_is_array(&t.type_union, false) == IsArray::No)
                }
            }
            _ => false,
        },
        _ => false,
    };
    if no { IsArray::No } else { IsArray::Maybe }
}

pub(crate) fn union_is_array(t: &TUnion, default: bool) -> IsArray {
    let no = !t.types.is_empty()
        && t.types.iter().all(|a| match a {
            TAtomic::Scalar(_)
            | TAtomic::Null
            | TAtomic::Void
            | TAtomic::Never
            | TAtomic::Resource(_)
            | TAtomic::Object(TObject::Enum(_)) => true,
            TAtomic::Object(TObject::Named(n)) => {
                !default || !n.name.as_bytes().eq_ignore_ascii_case(b"closure")
            }
            TAtomic::Callable(TCallable::Signature(s)) if default => s
                .get_return_type()
                .is_some_and(|r| union_is_array(r, false) == IsArray::No),
            TAtomic::Callable(TCallable::Signature(s)) => s.is_closure,
            _ => false,
        });
    if no { IsArray::No } else { IsArray::Maybe }
}
