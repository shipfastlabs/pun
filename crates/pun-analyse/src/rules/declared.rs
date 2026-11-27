use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use mago_allocator::LocalArena;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::class_like::ClassLikeMetadata;
use mago_codex::metadata::flags::MetadataFlags;
use mago_codex::metadata::function_like::FunctionLikeMetadata;
use mago_codex::symbol::SymbolKind;
use mago_database::file::File;
use mago_names::scope::NamespaceScope;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::parser::parse_file;
use pun_phpstan::resolve::resolve_in;
use pun_phpstan::symbols::{self, Class, Template};
use pun_phpstan::types::{
    NeverKind, Template as TemplateType, Type, decide_type, intersect, mixed, to_phpdoc, union,
};

use super::Checker;
use super::doc::{Doc, NameScope};
use super::facts::DocEntry;
use crate::text;

pub(super) fn original_name(k: &ClassLikeMetadata) -> String {
    text(k.original_name.as_bytes())
}

pub(super) fn doc_at<'a>(
    c: &Checker<'a, '_>,
    span: Span,
) -> Option<(&'a [u8], &'a DocEntry, &'a Arc<NamespaceScope>)> {
    let span = c.patched_docs.get(&span).copied().unwrap_or(span);
    let (raw, entry) = c.doc_before(span.file_id, span.start.offset)?;
    let names = c.file_facts(span.file_id)?.scopes.at(span.start.offset);
    Some((raw, entry, names))
}

pub(super) fn scope_at(c: &Checker, span: Span, class: Option<&ClassLikeMetadata>) -> NameScope {
    let span = c.patched_docs.get(&span).copied().unwrap_or(span);
    let names = c
        .file_facts(span.file_id)
        .map(|facts| facts.scopes.at(span.start.offset).clone())
        .unwrap_or_default();
    let mut scope = NameScope {
        names,
        class: class.map(original_name),
        parent: class.and_then(|k| parent_name(c.codebase, k)),
        templates: HashMap::new(),
    };
    if let Some(k) = class {
        for t in class_templates(c, k) {
            scope
                .templates
                .insert(t.name.clone(), Type::Template(Box::new(t)));
        }
        let owner = c
            .in_trait()
            .and_then(|t| c.codebase.get_class_like(t.as_bytes()))
            .filter(|t| {
                t.span.file_id == span.file_id
                    && t.span.start.offset <= span.start.offset
                    && span.end.offset <= t.span.end.offset
            });
        scope.templates.extend(match owner {
            Some(t) => super::type_aliases::class_aliases(c, t, Some(k)),
            None => super::type_aliases::class_aliases(c, k, None),
        });
    }
    scope
}

pub(super) fn parent_name(codebase: &CodebaseMetadata, k: &ClassLikeMetadata) -> Option<String> {
    let parent = k.direct_parent_class.as_ref()?;
    Some(
        codebase
            .get_class_like(parent.as_bytes())
            .map_or_else(|| text(parent.as_bytes()), original_name),
    )
}

fn templates(doc: &Doc, scope: &NameScope, describe: &str) -> Vec<TemplateType> {
    let resolve_all = |scope: &NameScope| -> Vec<TemplateType> {
        doc.templates
            .iter()
            .map(|t| TemplateType {
                name: t.name.clone(),
                scope: describe.to_string(),
                bound: t
                    .bound
                    .as_ref()
                    .map_or(Type::Mixed { explicit: true }, |b| resolve_in(b, scope)),
                default: t.default.as_ref().map(|d| resolve_in(d, scope)),
                argument: false,
            })
            .collect()
    };
    let mut scope = scope.clone();
    for t in &doc.templates {
        let placeholder = TemplateType {
            name: t.name.clone(),
            scope: describe.to_string(),
            bound: Type::Mixed { explicit: true },
            default: None,
            argument: false,
        };
        scope
            .templates
            .insert(t.name.clone(), Type::Template(Box::new(placeholder)));
    }
    let mut resolved = resolve_all(&scope);
    for _ in 0..doc.templates.len() {
        for t in &resolved {
            scope
                .templates
                .insert(t.name.clone(), Type::Template(Box::new(t.clone())));
        }
        let next = resolve_all(&scope);
        if next == resolved {
            break;
        }
        resolved = next;
    }
    resolved
}

pub(super) fn class_templates(c: &Checker, k: &ClassLikeMetadata) -> Vec<TemplateType> {
    let Some((_, entry, names)) = doc_at(c, k.span) else {
        return Vec::new();
    };
    let doc = &entry.doc;
    if doc.templates.is_empty() {
        return Vec::new();
    }
    let scope = NameScope {
        names: names.clone(),
        class: Some(original_name(k)),
        parent: parent_name(c.codebase, k),
        templates: HashMap::new(),
    };
    templates(doc, &scope, &format!("class {}", original_name(k)))
}

pub(super) fn trait_templates(
    c: &Checker,
    t: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> Vec<(String, Type)> {
    let own = class_templates(c, t);
    if own.is_empty() {
        return Vec::new();
    }
    let trait_name = original_name(t);
    let mut args: Option<Vec<Type>> = None;
    let (start, end) = (context.span.start.offset, context.span.end.offset);
    let scopes = c.file_facts(context.span.file_id).map(|f| &f.scopes);
    let scope = scope_at(c, context.span, Some(context));
    for (scopes, (offset, names)) in scopes
        .iter()
        .flat_map(|s| s.trait_uses(start, end).map(move |u| (*s, u)))
    {
        let names_here = scopes.at(*offset);
        let uses_it = names.iter().any(|n| {
            let (resolved, _) =
                names_here.resolve(mago_names::kind::NameKind::Default, n.as_bytes());
            resolved.eq_ignore_ascii_case(trait_name.as_bytes())
        });
        let Some((_, entry)) = uses_it
            .then(|| c.doc_before(context.span.file_id, *offset))
            .flatten()
        else {
            continue;
        };
        for node in &entry.doc.uses {
            if let Type::Object(name, found, _) = resolve_in(node, &scope)
                && !found.is_empty()
                && name.eq_ignore_ascii_case(&trait_name)
            {
                args = Some(found);
            }
        }
    }
    own.into_iter()
        .enumerate()
        .map(|(i, tpl)| {
            let ty = match &args {
                Some(args) => args
                    .get(i)
                    .cloned()
                    .or_else(|| tpl.default.clone())
                    .unwrap_or_else(|| tpl.bound.clone()),
                None => tpl.bound.clone(),
            };
            (tpl.name, ty)
        })
        .collect()
}

pub(super) fn native(c: &Checker, hint: &Hint, class: Option<&ClassLikeMetadata>) -> Type {
    native_with(c, hint, class, &|id| text(c.names.get(&id.span())))
}

fn native_with(
    c: &Checker,
    hint: &Hint,
    class: Option<&ClassLikeMetadata>,
    name: &dyn Fn(&Identifier) -> String,
) -> Type {
    match hint {
        Hint::Identifier(id) => Type::Object(name(id), vec![], false),
        Hint::Parenthesized(p) => native_with(c, p.hint, class, name),
        Hint::Nullable(n) => native_with(c, n.hint, class, name).add_null(),
        Hint::Union(u) => {
            let mut members = Vec::new();
            let mut stack = vec![u.right, u.left];
            while let Some(h) = stack.pop() {
                match h {
                    Hint::Union(inner) => stack.extend([inner.right, inner.left]),
                    h => members.push(native_with(c, h, class, name)),
                }
            }
            union(members)
        }
        Hint::Intersection(i) => {
            let mut members = Vec::new();
            let mut stack = vec![i.right, i.left];
            while let Some(h) = stack.pop() {
                match h {
                    Hint::Intersection(inner) => stack.extend([inner.right, inner.left]),
                    h => members.push(native_with(c, h, class, name)),
                }
            }
            if !members.iter().all(|m| {
                matches!(
                    m,
                    Type::Object(..) | Type::Static(..) | Type::ObjectWithoutClass
                )
            }) {
                return Type::Never(NeverKind::Implicit);
            }
            intersect(members)
        }
        Hint::Null(_) => Type::Null,
        Hint::True(_) => Type::ConstBool(true),
        Hint::False(_) => Type::ConstBool(false),
        Hint::Array(_) => Type::array(mixed(), mixed()),
        Hint::Callable(_) => Type::Callable(None),
        Hint::Static(_) => match class {
            Some(k) => Type::Static(original_name(k), vec![]),
            None => Type::Object("static".to_string(), vec![], false),
        },
        Hint::Self_(_) => match class {
            Some(k) => Type::Object(original_name(k), vec![], false),
            None => Type::Object("self".to_string(), vec![], false),
        },
        Hint::Parent(_) => match class.and_then(|k| parent_name(c.codebase, k)) {
            Some(parent) => Type::Object(parent, vec![], false),
            None => Type::Object("parent".to_string(), vec![], false),
        },
        Hint::Void(_) => Type::Void,
        Hint::Never(_) => Type::Never(NeverKind::Explicit),
        Hint::Float(_) => Type::Float,
        Hint::Bool(_) => Type::Bool,
        Hint::Integer(_) => Type::Int,
        Hint::String(_) => Type::String,
        Hint::Object(_) => Type::ObjectWithoutClass,
        Hint::Mixed(_) => Type::Mixed { explicit: true },
        Hint::Iterable(_) => Type::Iterable(Box::new(mixed()), Box::new(mixed())),
    }
}

fn native_declaration(
    c: &Checker,
    t: &mago_codex::metadata::ttype::TypeMetadata,
    class: Option<&ClassLikeMetadata>,
) -> Type {
    let contextual = class.is_some()
        && c.file_contents(t.span.file_id).is_some_and(|contents| {
            contents
                .get(t.span.start.offset as usize..t.span.end.offset as usize)
                .is_some_and(|hint| {
                    [b"self".as_slice(), b"parent", b"static"]
                        .iter()
                        .any(|name| hint.eq_ignore_ascii_case(name))
                })
        });
    if t.type_union.types.len() < 2 && !contextual {
        return from_tunion_in(c, &t.type_union, class);
    }
    let written = (|| {
        let contents = c.file_contents(t.span.file_id)?;
        let hint = contents.get(t.span.start.offset as usize..t.span.end.offset as usize)?;
        let arena = LocalArena::new();
        let file = File::ephemeral(
            std::borrow::Cow::Borrowed(b"native.php".as_slice()),
            std::borrow::Cow::Owned(
                [b"<?php function native(".as_slice(), hint, b" $value) {}"].concat(),
            ),
        );
        let program = parse_file(&arena, &file);
        let hint = program.statements.iter().find_map(|s| match s {
            Statement::Function(f) => f.parameter_list.parameters.first()?.hint.as_ref(),
            _ => None,
        })?;
        let scope = c.file_facts(t.span.file_id)?.scopes.at(t.span.start.offset);
        Some(native_with(c, hint, class, &|id| {
            text(
                &scope
                    .resolve(mago_names::kind::NameKind::Default, id.value())
                    .0,
            )
        }))
    })();
    written.unwrap_or_else(|| from_tunion_in(c, &t.type_union, class))
}

pub(super) fn codex_params(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
) -> Vec<Type> {
    let doc = codex_doc(c, f, declaring);
    codex_parameter_types(c, f, declaring, doc)
}

pub(super) fn reflected_params(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
    name: &str,
) -> Vec<Type> {
    let doc = reflected_doc(c, f, declaring, name);
    codex_parameter_types(c, f, declaring, doc)
}

fn codex_parameter_types(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
    doc: Resolved,
) -> Vec<Type> {
    let constructor = declaring.is_some() && f.name.as_bytes().eq_ignore_ascii_case(b"__construct");
    f.parameters
        .iter()
        .map(|p| {
            let name = text(&p.name.0.as_bytes()[1..]);
            let null_default = !p.flags.is_promoted_property()
                && p.default_type
                    .as_ref()
                    .is_some_and(|d| d.type_union.is_null());
            let native = p.type_declaration_metadata.as_ref().map(|t| {
                let t = native_declaration(c, t, declaring);
                if null_default { t.add_null() } else { t }
            });
            let promoted = (constructor && p.flags.is_promoted_property())
                .then(|| codex_promoted_var(c, p.span, &name, declaring))
                .flatten();
            let mut phpdoc = promoted.or_else(|| doc.params.get(&name).cloned());
            if p.default_type
                .as_ref()
                .is_some_and(|d| d.type_union.is_null())
                && let Some(t) = &phpdoc
                && !has_template(t)
            {
                phpdoc = Some(t.clone().add_null());
            }
            match native {
                None if p.flags.is_variadic() => match phpdoc {
                    Some(Type::Array(_, item)) => *item,
                    Some(t @ Type::ConstArray(_)) => t.iterable_value_type(),
                    t => t.unwrap_or_else(mixed),
                },
                None => phpdoc.unwrap_or_else(mixed),
                Some(native) => decide_type(native, phpdoc),
            }
        })
        .collect()
}

pub(super) fn codex_closure_this(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
) -> HashMap<String, Type> {
    codex_doc(c, f, declaring).closure_this
}

pub(super) fn reflected_closure_this(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
    name: &str,
) -> HashMap<String, Type> {
    reflected_doc(c, f, declaring, name).closure_this
}

/// [`reflected_accepts_named`] and [`reflected_closure_this`] of a method, from one reading of its docblocks.
pub(super) fn reflected_named_and_closure_this(
    c: &Checker,
    class: &ClassLikeMetadata,
    name: &str,
) -> (bool, HashMap<String, Type>) {
    let doc = method_doc(c, class, name.as_bytes(), 0);
    let accepts = !super::declarations::has_tag(&c.class_tags(class), &["@no-named-arguments"])
        && doc
            .as_ref()
            .and_then(|doc| doc.accepts_named)
            .unwrap_or(true);
    (accepts, doc.unwrap_or_default().closure_this)
}

pub(super) fn reflected_accepts_named(c: &Checker, class: &ClassLikeMetadata, name: &str) -> bool {
    !super::declarations::has_tag(&c.class_tags(class), &["@no-named-arguments"])
        && method_doc(c, class, name.as_bytes(), 0)
            .and_then(|doc| doc.accepts_named)
            .unwrap_or(true)
}

pub(super) fn codex_return(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
) -> Option<Type> {
    codex_doc(c, f, declaring).ret
}

pub(super) fn codex_return_type(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
    name: &str,
) -> Type {
    let declared =
        (f.return_type_declaration_metadata.as_ref()).map(|t| native_declaration(c, t, declaring));
    let native = magic_native(declaring.map(|_| name.as_bytes()), declared);
    decide_type(
        native.unwrap_or_else(mixed),
        reflected_doc(c, f, declaring, name).ret,
    )
}

/// [`reflected_throws`] and [`codex_return_type`] from one reading of the method's docblocks.
pub(super) fn reflected_throws_and_return(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
    name: &str,
) -> (Option<Type>, Type) {
    let doc = reflected_doc(c, f, declaring, name);
    let declared =
        (f.return_type_declaration_metadata.as_ref()).map(|t| native_declaration(c, t, declaring));
    let native = magic_native(declaring.map(|_| name.as_bytes()), declared);
    (
        doc.throws,
        decide_type(native.unwrap_or_else(mixed), doc.ret),
    )
}

pub(super) fn reflected_throws(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
    name: &str,
) -> Option<Type> {
    reflected_doc(c, f, declaring, name).throws
}

fn reflected_doc(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
    name: &str,
) -> Resolved {
    match declaring {
        Some(class) => method_doc(c, class, name.as_bytes(), 0).unwrap_or_default(),
        None => codex_doc(c, f, None),
    }
}

pub(super) fn value_scope(
    c: &Checker,
    f: &FunctionLikeMetadata,
    class: Option<&ClassLikeMetadata>,
) -> NameScope {
    let mut scope = scope_at(c, f.span, class);
    if let Some((_, entry, _)) = doc_at(c, f.span) {
        let name = text(f.original_name.as_bytes());
        let describe = class.map_or_else(
            || format!("function {name}()"),
            |k| format!("method {}::{name}()", original_name(k)),
        );
        for t in templates(&entry.doc, &scope, &describe) {
            scope
                .templates
                .insert(t.name.clone(), Type::Template(Box::new(t)));
        }
    }
    scope
}

fn codex_doc(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: Option<&ClassLikeMetadata>,
) -> Resolved {
    match declaring {
        Some(k) => method_doc(c, k, &f.name.as_bytes().to_ascii_lowercase(), 0),
        None => doc_at(c, f.span).map(|(_, entry, _)| {
            let doc = &entry.doc;
            let mut scope = scope_at(c, f.span, None);
            let name = text(f.original_name.as_bytes());
            for t in templates(doc, &scope, &format!("function {name}()")) {
                scope
                    .templates
                    .insert(t.name.clone(), Type::Template(Box::new(t)));
            }
            Resolved {
                params: doc
                    .param_types(&scope)
                    .into_iter()
                    .map(|(k, (t, _))| (k, t))
                    .collect(),
                ret: doc.return_type(&scope),
                closure_this: Doc::named_types(&doc.closure_this, &scope),
                throws: super::invalid_throws::throws_type(&entry.phpdoc, &scope),
                ..Resolved::default()
            }
        }),
    }
    .unwrap_or_default()
}

pub(super) fn from_tunion(
    c: &Checker,
    t: &mago_codex::ttype::union::TUnion,
    class: &ClassLikeMetadata,
) -> Type {
    from_tunion_in(c, t, Some(class))
}

pub(super) fn from_tunion_in(
    c: &Checker,
    t: &mago_codex::ttype::union::TUnion,
    class: Option<&ClassLikeMetadata>,
) -> Type {
    use mago_codex::ttype::atomic::TAtomic;
    use mago_codex::ttype::atomic::callable::TCallable;
    use mago_codex::ttype::atomic::object::TObject;
    use mago_codex::ttype::atomic::reference::TReference;
    use mago_codex::ttype::atomic::scalar::TScalar;
    use mago_codex::ttype::union::TUnion;
    let atom = |a: &TAtomic| match a {
        TAtomic::Null => Type::Null,
        TAtomic::Void => Type::Void,
        TAtomic::Never => Type::Never(NeverKind::Explicit),
        TAtomic::Scalar(TScalar::Bool(b)) => b.value.map_or(Type::Bool, Type::ConstBool),
        TAtomic::Scalar(TScalar::Integer(_)) => Type::Int,
        TAtomic::Scalar(TScalar::Float(_)) => Type::Float,
        TAtomic::Scalar(TScalar::String(_)) => Type::String,
        TAtomic::Array(_) => Type::array(mixed(), mixed()),
        TAtomic::Iterable(_) => Type::Iterable(Box::new(mixed()), Box::new(mixed())),
        TAtomic::Callable(TCallable::Signature(s)) if s.is_closure => {
            Type::Object("Closure".to_string(), vec![], false)
        }
        TAtomic::Callable(_) => Type::Callable(None),
        TAtomic::Object(TObject::Named(n)) => {
            let name = text(n.name.as_bytes());
            let ty = if n.is_static {
                Type::Static(name, vec![])
            } else if name.eq_ignore_ascii_case("parent") {
                Type::Object(
                    class
                        .and_then(|k| parent_name(c.codebase, k))
                        .unwrap_or(name),
                    vec![],
                    false,
                )
            } else {
                Type::Object(name, vec![], false)
            };
            match &n.intersection_types {
                None => ty,
                Some(more) => intersect(
                    std::iter::once(ty)
                        .chain(
                            more.iter()
                                .map(|m| from_tunion_in(c, &TUnion::from_atomic(m.clone()), class)),
                        )
                        .collect(),
                ),
            }
        }
        TAtomic::Reference(TReference::Symbol { name, .. }) => {
            Type::Object(text(name.as_bytes()), vec![], false)
        }
        TAtomic::Object(TObject::Any) => Type::ObjectWithoutClass,
        TAtomic::Object(TObject::Enum(e)) => Type::Object(text(e.name.as_bytes()), vec![], false),
        _ => Type::Mixed { explicit: true },
    };
    union(t.types.iter().map(atom).collect())
}

pub(super) struct Signature {
    pub params: Vec<(String, Type, Option<Type>, Option<Type>)>,
    pub ret: Type,
    pub self_out: Option<Type>,
    pub asserts: Vec<String>,
    pub phpdoc: Vec<Type>,
}

#[derive(Clone, Default)]
struct Resolved {
    params: HashMap<String, Type>,
    ret: Option<Type>,
    accepts_named: Option<bool>,
    outs: HashMap<String, Type>,
    closure_this: HashMap<String, Type>,
    self_out: Option<Type>,
    asserts: Vec<String>,
    pure: Option<bool>,
    pure_unless_callable: Vec<String>,
    pure_unless_passed: Vec<String>,
    throws: Option<Type>,
}

pub(super) fn tagged_params(raw: &[u8], names: &[&str]) -> Vec<String> {
    let tagged = (raw.iter().enumerate())
        .filter(|(_, b)| **b == b'@')
        .any(|(at, _)| names.iter().any(|n| raw[at..].starts_with(n.as_bytes())));
    if !tagged {
        return Vec::new();
    }
    let raw = String::from_utf8_lossy(raw);
    raw.lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let mut words = l
                .strip_prefix("/**")
                .unwrap_or(l)
                .trim_start_matches('*')
                .split_whitespace();
            names
                .contains(&words.next()?)
                .then(|| words.next()?.strip_prefix('$').map(str::to_string))
                .flatten()
        })
        .collect()
}

pub(super) fn method_purity(
    c: &Checker,
    class: &ClassLikeMetadata,
    name: &[u8],
) -> (Option<bool>, bool, bool) {
    let lower = name.to_ascii_lowercase();
    let doc = method_doc(c, class, &lower, 0).unwrap_or_default();
    let mut pure = doc.pure;
    if pure.is_none() {
        let tags = doc_at(c, class.span).map_or(&[][..], |(_, entry, _)| &entry.names);
        let void = |t: Option<&Type>| t.is_some_and(|t| t.is_void().yes());
        let native = c
            .codebase
            .get_method(class.name.as_bytes(), &lower)
            .and_then(|m| m.return_type_declaration_metadata.as_ref())
            .is_some_and(|t| t.type_union.is_void());
        if tags.iter().any(|t| t == "@phpstan-all-methods-pure") {
            if lower == b"__construct" || (!void(doc.ret.as_ref()) && !native) {
                pure = Some(true);
            }
        } else if tags.iter().any(|t| t == "@phpstan-all-methods-impure") {
            pure = Some(false);
        }
    }
    (
        pure,
        !doc.pure_unless_callable.is_empty(),
        !doc.pure_unless_passed.is_empty(),
    )
}

pub(super) fn method_conditional_purity(
    c: &Checker,
    class: &ClassLikeMetadata,
    name: &[u8],
) -> (Vec<String>, Vec<String>, Option<Type>) {
    let doc = method_doc(c, class, &name.to_ascii_lowercase(), 0).unwrap_or_default();
    (doc.pure_unless_callable, doc.pure_unless_passed, doc.throws)
}

fn resolved_own(
    c: &Checker,
    m: &FunctionLikeMetadata,
    class: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> Option<Resolved> {
    let (raw, entry, _) = doc_at(c, m.span)?;
    let doc = &entry.doc;
    let mut scope = scope_at(c, m.span, Some(context));
    if class.kind == SymbolKind::Trait && !std::ptr::eq(class, context) {
        scope.templates.extend(trait_templates(c, class, context));
    }
    let method = text(m.original_name.as_bytes());
    for t in templates(
        doc,
        &scope,
        &format!("method {}::{method}()", original_name(class)),
    ) {
        scope
            .templates
            .insert(t.name.clone(), Type::Template(Box::new(t)));
    }
    Some(Resolved {
        params: doc
            .param_types(&scope)
            .into_iter()
            .map(|(k, (t, _))| (k, t))
            .collect(),
        ret: doc.return_type(&scope),
        accepts_named: Some(!super::declarations::has_tag(
            &entry.names,
            &["@no-named-arguments"],
        )),
        outs: Doc::named_types(&doc.outs, &scope),
        closure_this: Doc::named_types(&doc.closure_this, &scope),
        self_out: doc.self_out.as_ref().map(|n| resolve_in(n, &scope)),
        asserts: super::assert_tags::parameters(&entry.phpdoc),
        pure: {
            let tags = &entry.names;
            if super::declarations::has_tag(
                tags,
                &[
                    "@pure",
                    "@phan-pure",
                    "@phan-side-effect-free",
                    "@psalm-pure",
                    "@phpstan-pure",
                ],
            ) {
                Some(true)
            } else if super::declarations::has_tag(tags, &["@impure", "@phpstan-impure"]) {
                Some(false)
            } else {
                None
            }
        },
        pure_unless_callable: tagged_params(
            raw,
            &[
                "@pure-unless-callable-is-impure",
                "@phpstan-pure-unless-callable-is-impure",
            ],
        ),
        pure_unless_passed: tagged_params(
            raw,
            &[
                "@pure-unless-parameter-passed",
                "@phpstan-pure-unless-parameter-passed",
            ],
        ),
        throws: super::invalid_throws::throws_type(&entry.phpdoc, &scope),
    })
}

fn method_doc(
    c: &Checker,
    class: &ClassLikeMetadata,
    name: &[u8],
    depth: usize,
) -> Option<Resolved> {
    let m = c
        .codebase
        .get_declaring_method(class.name.as_bytes(), name)?;
    if class.flags.contains(MetadataFlags::BUILTIN) {
        return builtin_method_doc(c, class, m, name);
    }
    let storage = c
        .codebase
        .get_declaring_method_class(class.name.as_bytes(), name)
        .and_then(|n| c.codebase.get_class_like(n.as_bytes()))
        .unwrap_or(class);
    let aliased_source = storage.name != class.name
        && storage.kind.is_trait()
        && class.trait_alias_map.iter().any(|(alias, source)| {
            source.as_bytes() == name
                && class.declaring_method_ids.get(alias)
                    == class.declaring_method_ids.get(&mago_word::word(name))
        });
    let own = if let Some(stub) = (!c.patched_docs.contains_key(&m.span))
        .then(|| ancestor_stub_doc(c, class, storage, name))
        .flatten()
    {
        Some(stub)
    } else if (m.has_docblock || c.patched_docs.contains_key(&m.span)) && !aliased_source {
        resolved_own(c, m, storage, class)
    } else {
        None
    };
    let names: Vec<String> = m
        .parameters
        .iter()
        .map(|p| text(&p.name.0.as_bytes()[1..]))
        .collect();
    inherit(c, class, name, own, &names, depth)
}

fn ancestor_stub_doc(
    c: &Checker,
    class: &ClassLikeMetadata,
    storage: &ClassLikeMetadata,
    name: &[u8],
) -> Option<Resolved> {
    let lower = mago_word::word(name.to_ascii_lowercase());
    let parent = |k: &ClassLikeMetadata| {
        k.direct_parent_class
            .and_then(|p| c.codebase.get_class_like(p.as_bytes()))
    };
    let declaring = if storage.kind.is_trait() {
        std::iter::successors(Some(class), |k| parent(k))
            .take(super::overriding::ANCESTRY)
            .find(|k| k.used_traits.contains(&storage.name))?
    } else {
        storage
    };
    if declaring.flags.contains(MetadataFlags::BUILTIN)
        || !c.codebase.patch_class_likes.contains_key(&declaring.name)
    {
        return None;
    }
    fn collect<'a>(
        c: &Checker<'a, '_>,
        k: &'a ClassLikeMetadata,
        out: &mut Vec<&'a ClassLikeMetadata>,
    ) {
        let next = k
            .direct_parent_interfaces
            .iter()
            .chain(k.used_traits.iter())
            .chain(k.direct_parent_class.iter());
        for n in next {
            if out.len() < 256
                && let Some(a) = c.codebase.get_class_like(n.as_bytes())
                && !out.iter().any(|o| o.name == a.name)
            {
                out.push(a);
                collect(c, a, out);
            }
        }
    }
    let mut ancestors = vec![declaring];
    collect(c, declaring, &mut ancestors);
    ancestors.into_iter().skip(1).find_map(|ancestor| {
        ancestor.declaring_method_ids.get(&lower)?;
        let stub = c
            .codebase
            .patch_function_likes
            .get(&(ancestor.name, lower))
            .filter(|p| p.has_docblock)?;
        let doc = resolved_own(c, stub, ancestor, ancestor)?;
        let map = |t: Type| ancestor_types(class, ancestor, &t);
        Some(Resolved {
            params: doc.params.into_iter().map(|(k, t)| (k, map(t))).collect(),
            ret: doc.ret.map(map),
            outs: doc.outs.into_iter().map(|(k, t)| (k, map(t))).collect(),
            ..doc
        })
    })
}

/// A built-in method's PHPDoc as PHPStan reflects it, including its throw type, which an overriding method without
/// a PHPDoc inherits.
fn builtin_method_doc(
    c: &Checker,
    class: &ClassLikeMetadata,
    m: &FunctionLikeMetadata,
    name: &[u8],
) -> Option<Resolved> {
    let lower = text(name).to_ascii_lowercase();
    let throws = super::types::throw_points::builtin_method_throws(
        c,
        &text(class.name.as_bytes()).to_ascii_lowercase(),
        &lower,
    );
    let Some((k, doc)) = symbols::class(&text(class.name.as_bytes()))
        .and_then(|k| Some((k, k.methods.get(&lower)?.doc.as_ref()?)))
    else {
        return throws.map(|throws| Resolved {
            throws: Some(throws),
            ..Resolved::default()
        });
    };
    let resolve = |s: &str| pun_phpstan::resolve::resolve_str(s, Some(&k.name));
    Some(Resolved {
        throws,
        params: doc
            .params
            .iter()
            .filter_map(|(i, t)| {
                let p = m.parameters.get(*i)?;
                Some((text(&p.name.0.as_bytes()[1..]), resolve(t)))
            })
            .collect(),
        ret: doc.ret.as_deref().map(resolve),
        ..Resolved::default()
    })
}

fn inherit(
    c: &Checker,
    class: &ClassLikeMetadata,
    name: &[u8],
    current: Option<Resolved>,
    positional: &[String],
    depth: usize,
) -> Option<Resolved> {
    if depth > 32 {
        return current;
    }
    let lower = name.to_ascii_lowercase();
    let from = |parent: &ClassLikeMetadata| -> Option<Resolved> {
        let declaring = declaring_method(c.codebase, parent, &lower)?;
        if declaring
            .1
            .method_metadata
            .as_ref()
            .is_some_and(|m| m.visibility == mago_codex::visibility::Visibility::Private)
        {
            return None;
        }
        // a method a trait gives `parent` is reflected with `parent` as its declaring class, whose own parents the
        // PHPDoc inheritance walks on to (PhpDocInheritanceResolver)
        let owner = if declaring.0.kind.is_trait() {
            parent
        } else {
            declaring.0
        };
        let parent_doc = method_doc(c, owner, &lower, depth + 1)?;
        let parent_doc = {
            let map = |t: Type| ancestor_types(class, owner, &t);
            Resolved {
                params: parent_doc
                    .params
                    .into_iter()
                    .map(|(k, t)| (k, map(t)))
                    .collect(),
                ret: parent_doc.ret.map(map),
                outs: parent_doc
                    .outs
                    .into_iter()
                    .map(|(k, t)| (k, map(t)))
                    .collect(),
                closure_this: parent_doc
                    .closure_this
                    .into_iter()
                    .map(|(k, t)| (k, map(t)))
                    .collect(),
                ..parent_doc
            }
        };
        Some(merge(
            current.clone(),
            parent_doc,
            positional,
            &declaring.1.parameters,
            is_constructor(declaring.0, &lower),
        ))
    };
    if let Some(parent) = class
        .direct_parent_class
        .as_ref()
        .and_then(|p| c.codebase.get_class_like(p.as_bytes()))
        && let Some(found) = from(parent)
        && !(lower == b"__construct" && parent.flags.contains(MetadataFlags::BUILTIN))
    {
        return Some(found);
    }
    let interfaces = class.direct_parent_interfaces.iter().map(|i| {
        if i.as_bytes().starts_with(b"__internal_do_not_use__") {
            &b"BackedEnum"[..]
        } else {
            i.as_bytes()
        }
    });
    for interface in interfaces {
        if let Some(i) = c.codebase.get_class_like(interface)
            && let Some(found) = from(i)
        {
            return Some(found);
        }
    }
    for t in &class.used_traits {
        let (Some(trait_), Some(m)) = (
            c.codebase.get_class_like(t.as_bytes()),
            c.codebase.get_method(t.as_bytes(), &lower),
        ) else {
            continue;
        };
        let abstract_with_doc =
            m.has_docblock && m.method_metadata.as_ref().is_some_and(|mm| mm.is_abstract);
        let Some(own) = abstract_with_doc
            .then(|| resolved_own(c, m, trait_, class))
            .flatten()
        else {
            continue;
        };
        return Some(merge(
            current,
            own,
            positional,
            &m.parameters,
            is_constructor(trait_, &lower),
        ));
    }
    current
}

fn declaring_method<'c>(
    codebase: &'c CodebaseMetadata,
    parent: &ClassLikeMetadata,
    lower: &[u8],
) -> Option<(&'c ClassLikeMetadata, &'c FunctionLikeMetadata)> {
    let id = parent.declaring_method_ids.get(&mago_word::word(lower))?;
    let m = codebase.get_method_by_id(id)?;
    if m.flags.contains(MetadataFlags::MAGIC_METHOD) {
        return None;
    }
    let k = codebase.get_class_like(id.get_class_name().as_bytes())?;
    Some((k, m))
}

fn ancestor_types(class: &ClassLikeMetadata, declaring: &ClassLikeMetadata, t: &Type) -> Type {
    let (Some(k), Some(d)) = (
        symbols::lookup(&original_name(class)),
        symbols::lookup(&original_name(declaring)),
    ) else {
        return t.clone();
    };
    let Some(written) = k.generics.get(&d.name) else {
        return t.clone();
    };
    // The arguments are resolved at the first template they replace: most types have none.
    let resolve = || -> Vec<Type> {
        (written.iter())
            .map(|s| pun_phpstan::resolve::resolve_str(s, Some(&k.name)))
            .collect()
    };
    let args = std::cell::OnceCell::new();
    let scope = format!("class {}", d.name);
    fn map(
        t: &Type,
        scope: &str,
        d: &symbols::Class,
        args: &std::cell::OnceCell<Vec<Type>>,
        resolve: &dyn Fn() -> Vec<Type>,
    ) -> Type {
        match t {
            Type::Template(tpl) if tpl.scope == scope => d
                .templates
                .iter()
                .position(|x| x.name == tpl.name)
                .and_then(|i| args.get_or_init(resolve).get(i))
                .cloned()
                .unwrap_or_else(|| t.clone()),
            _ => t.traverse(&|i| map(i, scope, d, args, resolve)),
        }
    }
    map(t, &scope, &d, &args, &resolve)
}

fn merge(
    current: Option<Resolved>,
    parent: Resolved,
    positional: &[String],
    parent_params: &[mago_codex::metadata::parameter::FunctionLikeParameterMetadata],
    constructor: bool,
) -> Resolved {
    let mut out = current.unwrap_or_default();
    out.accepts_named =
        Some(out.accepts_named.unwrap_or(true) && parent.accepts_named.unwrap_or(true));
    let parent_names: Vec<String> = if constructor {
        positional.to_vec()
    } else {
        parent_params
            .iter()
            .map(|p| text(&p.name.0.as_bytes()[1..]))
            .collect()
    };
    let mapped = |name: &str| {
        parent_names
            .iter()
            .position(|n| n == name)
            .and_then(|i| positional.get(i))
            .cloned()
    };
    let rename = |name: String| mapped(&name).unwrap_or(name);
    let types = |t: Type| rename_conditionals(&t, &mapped);
    if out.ret.is_none() {
        out.ret = parent.ret.map(types);
    }
    for (name, ty) in parent.params {
        out.params.entry(rename(name)).or_insert_with(|| types(ty));
    }
    if out.self_out.is_none() {
        out.self_out = parent.self_out;
    }
    if out.throws.is_none() {
        out.throws = parent.throws.map(types);
    }
    if out.asserts.is_empty() {
        out.asserts = parent.asserts.into_iter().map(rename).collect();
    }
    for (name, ty) in parent.outs {
        out.outs.entry(rename(name)).or_insert(ty);
    }
    for (name, ty) in parent.closure_this {
        out.closure_this
            .entry(rename(name))
            .or_insert_with(|| types(ty));
    }
    out.pure = out.pure.or(parent.pure);
    for name in parent.pure_unless_callable {
        let name = rename(name);
        if !out.pure_unless_callable.contains(&name) {
            out.pure_unless_callable.push(name);
        }
    }
    for name in parent.pure_unless_passed {
        let name = rename(name);
        if !out.pure_unless_passed.contains(&name) {
            out.pure_unless_passed.push(name);
        }
    }
    out
}

fn rename_conditionals(t: &Type, mapped: &dyn Fn(&str) -> Option<String>) -> Type {
    let t = match t {
        Type::Conditional(c) => match &c.subject {
            pun_phpstan::types::Subject::Parameter(p) => match mapped(&p[1..]) {
                Some(name) => Type::Conditional(Box::new(pun_phpstan::types::Conditional {
                    subject: pun_phpstan::types::Subject::Parameter(format!("${name}")),
                    resolved: Default::default(),
                    ..(**c).clone()
                })),
                None => t.clone(),
            },
            _ => t.clone(),
        },
        _ => t.clone(),
    };
    t.traverse(&|i| rename_conditionals(i, mapped))
}

pub(super) fn method_scope(
    c: &Checker,
    m: &Method,
    doc: &Doc,
    k: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> NameScope {
    let mut scope = scope_at(c, m.span(), Some(context));
    if k.kind == SymbolKind::Trait && !std::ptr::eq(k, context) {
        scope.templates.extend(trait_templates(c, k, context));
    } else {
        for t in class_templates(c, k) {
            scope
                .templates
                .insert(t.name.clone(), Type::Template(Box::new(t)));
        }
    }
    if k.name.as_bytes().starts_with(b"{anonymous-class:") {
        enclosing_templates(c, m.span(), &mut scope);
    }
    let method = text(m.name.value);
    for t in templates(
        doc,
        &scope,
        &format!("method {}::{method}()", original_name(context)),
    ) {
        scope
            .templates
            .insert(t.name.clone(), Type::Template(Box::new(t)));
    }
    scope
}

fn enclosing_templates(c: &Checker, at: Span, scope: &mut NameScope) {
    let Some(i) = c
        .var_tag_functions
        .iter()
        .rposition(|f| f.start.offset < at.start.offset && at.end.offset <= f.end.offset)
    else {
        return;
    };
    let (span, function) = (c.var_tag_functions[i], &c.functions[i]);
    let Some((_, entry)) = c.doc_before(c.file.id, span.start.offset) else {
        return;
    };
    let described = match function.kind {
        super::FunctionScopeKind::Method { .. } => {
            let class = c
                .classes
                .iter()
                .rev()
                .nth(1)
                .and_then(|k| k.name.as_deref())
                .unwrap_or_default();
            let class = c
                .codebase
                .get_class_like(class.as_bytes())
                .map_or_else(|| class.to_string(), original_name);
            format!("method {class}::{}()", function.name)
        }
        super::FunctionScopeKind::Function => format!("function {}()", function.name),
    };
    for t in templates(&entry.doc, scope, &described) {
        scope
            .templates
            .insert(t.name.clone(), Type::Template(Box::new(t)));
    }
}

pub(super) fn function_scope(c: &Checker, f: &Function, doc: &Doc) -> NameScope {
    let mut scope = scope_at(c, f.span(), None);
    let name = text(c.names.get(&f.name.span()));
    for t in templates(doc, &scope, &format!("function {name}()")) {
        scope
            .templates
            .insert(t.name.clone(), Type::Template(Box::new(t)));
    }
    scope
}

pub(super) fn method(
    c: &Checker,
    m: &Method,
    k: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> Signature {
    let resolved = method_resolved(c, m, k, context);
    signature(
        c,
        Some(m.name.value),
        &m.parameter_list,
        m.return_type_hint.as_ref(),
        resolved,
        Some(context),
    )
}

pub(super) struct PhpDocTypes {
    pub params: HashMap<String, Vec<Type>>,
    pub ret: Option<Type>,
}

impl From<Resolved> for PhpDocTypes {
    fn from(r: Resolved) -> PhpDocTypes {
        let mut params: HashMap<String, Vec<Type>> = HashMap::new();
        for (name, t) in r.params.into_iter().chain(r.outs).chain(r.closure_this) {
            params.entry(name).or_default().push(t);
        }
        PhpDocTypes { params, ret: r.ret }
    }
}

pub(super) fn method_phpdoc(
    c: &Checker,
    m: &Method,
    k: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> PhpDocTypes {
    let mut resolved = method_resolved(c, m, k, context);
    if m.name.value.eq_ignore_ascii_case(b"__construct") {
        for p in &m.parameter_list.parameters {
            if let Some(t) = promoted_var(c, p, Some(context)) {
                resolved.params.insert(text(&p.variable.name[1..]), t);
            }
        }
    }
    resolved.into()
}

pub(super) fn method_phpdoc_signature(
    c: &Checker,
    m: &Method,
    k: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> (HashMap<String, Type>, Option<Type>) {
    let resolved = method_resolved(c, m, k, context);
    (resolved.params, resolved.ret)
}

pub(super) fn reflected_phpdoc_signature(
    c: &Checker,
    f: &FunctionLikeMetadata,
    declaring: &ClassLikeMetadata,
    name: &str,
) -> (HashMap<String, Type>, Option<Type>) {
    let resolved = reflected_doc(c, f, Some(declaring), name);
    (resolved.params, resolved.ret)
}

/// The PHPDoc of trait method `f` itself, resolved in `context` with the trait's templates as the class's `@use` gives
/// them: `context` may override `f`, so `reflected_phpdoc_signature()` would find the override's PHPDoc instead.
pub(super) fn trait_phpdoc_signature(
    c: &Checker,
    f: &FunctionLikeMetadata,
    t: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> Option<(HashMap<String, Type>, Option<Type>)> {
    let resolved = f
        .has_docblock
        .then(|| resolved_own(c, f, t, context))
        .flatten()?;
    Some((resolved.params, resolved.ret))
}

pub(super) fn function_phpdoc(c: &Checker, f: &Function) -> PhpDocTypes {
    function_resolved(c, f).into()
}

fn method_resolved(
    c: &Checker,
    m: &Method,
    k: &ClassLikeMetadata,
    context: &ClassLikeMetadata,
) -> Resolved {
    let own = c
        .doc_before(c.file.id, m.span().start.offset)
        .map(|(_, entry)| {
            let doc = &entry.doc;
            let scope = method_scope(c, m, doc, k, context);
            Resolved {
                params: doc
                    .param_types(&scope)
                    .into_iter()
                    .map(|(k, (t, _))| (k, t))
                    .collect(),
                ret: doc.return_type(&scope),
                outs: Doc::named_types(&doc.outs, &scope),
                closure_this: Doc::named_types(&doc.closure_this, &scope),
                self_out: doc.self_out.as_ref().map(|n| resolve_in(n, &scope)),
                asserts: super::assert_tags::parameters(&entry.phpdoc),
                ..Resolved::default()
            }
        });
    let positional: Vec<String> = m
        .parameter_list
        .parameters
        .iter()
        .map(|p| text(&p.variable.name[1..]))
        .collect();
    inherit(c, context, m.name.value, own, &positional, 0).unwrap_or_default()
}

pub(super) fn function(c: &Checker, f: &Function) -> Signature {
    let resolved = function_resolved(c, f);
    signature(
        c,
        None,
        &f.parameter_list,
        f.return_type_hint.as_ref(),
        resolved,
        None,
    )
}

fn function_resolved(c: &Checker, f: &Function) -> Resolved {
    c.doc_before(c.file.id, f.span().start.offset)
        .map(|(_, entry)| {
            let doc = &entry.doc;
            let scope = function_scope(c, f, doc);
            Resolved {
                params: doc
                    .param_types(&scope)
                    .into_iter()
                    .map(|(k, (t, _))| (k, t))
                    .collect(),
                ret: doc.return_type(&scope),
                outs: Doc::named_types(&doc.outs, &scope),
                closure_this: Doc::named_types(&doc.closure_this, &scope),
                self_out: doc.self_out.as_ref().map(|n| resolve_in(n, &scope)),
                asserts: super::assert_tags::parameters(&entry.phpdoc),
                ..Resolved::default()
            }
        })
        .unwrap_or_default()
}

fn signature(
    c: &Checker,
    method: Option<&[u8]>,
    params: &FunctionLikeParameterList,
    ret: Option<&FunctionLikeReturnTypeHint>,
    doc: Resolved,
    class: Option<&ClassLikeMetadata>,
) -> Signature {
    let params = params
        .parameters
        .iter()
        .map(|p| {
            let name = text(&p.variable.name[1..]);
            let native = p.hint.as_ref().map(|h| {
                let t = native(c, h, class);
                if default_is_null(p) && p.modifiers.is_empty() {
                    t.add_null()
                } else {
                    t
                }
            });
            let promoted = if method.is_some_and(|m| m.eq_ignore_ascii_case(b"__construct")) {
                promoted_var(c, p, class)
            } else {
                None
            };
            let mut phpdoc = promoted.or_else(|| doc.params.get(&name).cloned());
            if default_is_null(p)
                && let Some(t) = &phpdoc
                && !has_template(t)
            {
                phpdoc = Some(t.clone().add_null());
            }
            let ty = decide_type(native.unwrap_or_else(mixed), phpdoc);
            let out = doc.outs.get(&name).cloned();
            let closure_this = doc.closure_this.get(&name).cloned();
            (name, ty, out, closure_this)
        })
        .collect();
    let native_ret = magic_native(method, ret.map(|r| native(c, &r.hint, class)));
    let phpdoc = doc
        .params
        .values()
        .cloned()
        .chain(doc.ret.clone())
        .collect();
    let ret = decide_type(native_ret.unwrap_or_else(mixed), doc.ret);
    Signature {
        params,
        ret,
        self_out: doc.self_out,
        asserts: doc.asserts,
        phpdoc,
    }
}

/// PhpMethodFromParserNodeReflection: a magic method's real return type replaces the declared one, except
/// `__set_state()` and `__debugInfo()`, whose declared type it narrows (kept here), and a constructor, which PHP
/// gives no return type (a declared one stays as written); it also fills in a missing one.
fn magic_native(method: Option<&[u8]>, declared: Option<Type>) -> Option<Type> {
    let replaced = method.filter(|m| {
        !(m.eq_ignore_ascii_case(b"__set_state")
            || m.eq_ignore_ascii_case(b"__debuginfo")
            || m.eq_ignore_ascii_case(b"__construct"))
    });
    replaced
        .and_then(magic_return)
        .or(declared)
        .or_else(|| method.and_then(magic_return))
}

fn magic_return(name: &[u8]) -> Option<Type> {
    match name.to_ascii_lowercase().as_slice() {
        b"__construct" | b"__destruct" | b"__unset" | b"__wakeup" | b"__clone" | b"__set"
        | b"__unserialize" => Some(Type::Void),
        b"__serialize" => Some(Type::array(
            Type::Mixed { explicit: true },
            Type::Mixed { explicit: true },
        )),
        b"__debuginfo" => Some(Type::Union(vec![
            Type::array(
                Type::Mixed { explicit: true },
                Type::Mixed { explicit: true },
            ),
            Type::Null,
        ])),
        b"__tostring" => Some(Type::String),
        b"__isset" => Some(Type::Bool),
        b"__sleep" => Some(Type::array(Type::Int, Type::String)),
        b"__set_state" => Some(Type::ObjectWithoutClass),
        _ => None,
    }
}

fn promoted_var(
    c: &Checker,
    p: &FunctionLikeParameter,
    class: Option<&ClassLikeMetadata>,
) -> Option<Type> {
    if !p.is_promoted_property() {
        return None;
    }
    let (_, entry) = c.doc_before(c.file.id, p.span().start.offset)?;
    let doc = &entry.doc;
    let scope = scope_at(c, p.span(), class);
    let vars = doc.var_types(&scope);
    let name = text(&p.variable.name[1..]);
    match vars.as_slice() {
        [(n, t)] if n.is_empty() => Some(t.clone()),
        _ => vars.into_iter().find(|(n, _)| *n == name).map(|(_, t)| t),
    }
}

fn codex_promoted_var(
    c: &Checker,
    span: Span,
    name: &str,
    class: Option<&ClassLikeMetadata>,
) -> Option<Type> {
    let (_, entry, _) = doc_at(c, span)?;
    let vars = entry.doc.var_types(&scope_at(c, span, class));
    match vars.as_slice() {
        [(n, t)] if n.is_empty() => Some(t.clone()),
        _ => vars.into_iter().find(|(n, _)| n == name).map(|(_, t)| t),
    }
}

pub(crate) fn has_template(t: &Type) -> bool {
    let found = std::cell::Cell::new(false);
    fn walk(t: &Type, found: &std::cell::Cell<bool>) {
        if matches!(t, Type::Template(_)) {
            found.set(true);
        }
        t.traverse(&|inner| {
            walk(inner, found);
            inner.clone()
        });
    }
    walk(t, &found);
    found.get()
}

fn is_constructor(k: &ClassLikeMetadata, lower: &[u8]) -> bool {
    let short = k
        .name
        .as_bytes()
        .rsplit(|b| *b == b'\\')
        .next()
        .unwrap_or(b"");
    lower == b"__construct" || short.eq_ignore_ascii_case(lower)
}

pub(super) fn default_is_null(p: &FunctionLikeParameter) -> bool {
    match p.default_value.as_ref().map(|d| d.value) {
        Some(Expression::Literal(Literal::Null(_))) => true,
        Some(Expression::ConstantAccess(a)) => a.name.value().eq_ignore_ascii_case(b"null"),
        _ => false,
    }
}

pub(super) fn property(
    c: &Checker,
    hint: Option<&Hint>,
    at: Span,
    name: &[u8],
    context: &ClassLikeMetadata,
) -> Type {
    let native = hint.map(|h| native(c, h, Some(context)));
    let mut phpdoc = c
        .doc_before(c.file.id, at.start.offset)
        .and_then(|(_, entry)| {
            let doc = &entry.doc;
            let mut scope = scope_at(c, at, Some(context));
            if let Some(t) = c
                .in_trait()
                .and_then(|t| c.codebase.get_class_like(t.as_bytes()))
            {
                scope.templates.extend(trait_templates(c, t, context));
            }
            doc.var_types(&scope)
                .into_iter()
                .find(|(n, _)| n.is_empty() || n.as_bytes() == name)
                .map(|(_, t)| t)
        });
    if phpdoc.is_none() {
        phpdoc = inherited_property_doc(c, context, name, 0);
    }
    match native {
        Some(n) => decide_type(n, phpdoc),
        None => phpdoc.unwrap_or_else(mixed),
    }
}

pub(super) fn inherited_property_doc(
    c: &Checker,
    class: &ClassLikeMetadata,
    name: &[u8],
    depth: usize,
) -> Option<Type> {
    if depth > 32 {
        return None;
    }
    let mut parents: Vec<&ClassLikeMetadata> = class
        .direct_parent_class
        .iter()
        .filter_map(|p| c.codebase.get_class_like(p.as_bytes()))
        .collect();
    parents.extend(
        class
            .direct_parent_interfaces
            .iter()
            .filter_map(|i| c.codebase.get_class_like(i.as_bytes())),
    );
    let mut var = Vec::with_capacity(name.len() + 1);
    var.push(b'$');
    var.extend_from_slice(name);
    for parent in parents {
        let Some((declaring, p)) = parent
            .declaring_property_ids
            .get(&mago_word::word(&var))
            .and_then(|k| c.codebase.get_class_like(k.as_bytes()))
            .and_then(|d| Some((d, d.properties.get(&mago_word::word(&var))?)))
        else {
            continue;
        };
        if p.read_visibility == mago_codex::visibility::Visibility::Private {
            continue;
        }
        if declaring.flags.contains(MetadataFlags::BUILTIN) {
            let class = symbols::class(&text(declaring.name.as_bytes()))?;
            return class
                .property_docs
                .get(&text(name))
                .map(|t| pun_phpstan::resolve::resolve_str(t, Some(&class.name)));
        }
        let reflected = if declaring.kind.is_trait() {
            // `used_traits` also holds the parents' traits: the class that declares the property is the nearest one
            // that uses the trait itself, else (a trait that a trait uses) the farthest one that has it
            let chain = || {
                std::iter::successors(Some(parent), |k| {
                    k.direct_parent_class
                        .and_then(|p| c.codebase.get_class_like(p.as_bytes()))
                })
                .take(super::overriding::ANCESTRY)
            };
            chain()
                .find(|k| k.direct_trait_names().any(|t| t == declaring.name))
                .or_else(|| {
                    chain()
                        .filter(|k| k.used_traits.contains(&declaring.name))
                        .last()
                })
                .unwrap_or(parent)
        } else {
            declaring
        };
        let stub = c
            .codebase
            .patch_class_likes
            .get(&reflected.name)
            .and_then(|patch| {
                let p = patch.properties.get(&mago_word::word(&var))?;
                own_property_doc(c, patch, p, name)
            });
        if let Some(t) = stub {
            return Some(ancestor_types(class, reflected, &t));
        }
        let own = own_property_doc(c, declaring, p, name);
        return own
            .or_else(|| inherited_property_doc(c, declaring, name, depth + 1))
            .map(|t| ancestor_types(class, declaring, &t));
    }
    None
}

pub(super) fn property_span(
    c: &Checker,
    p: &mago_codex::metadata::property::PropertyMetadata,
) -> Option<Span> {
    p.span.or_else(|| {
        let mut span = p.name_span?;
        let contents = c.file_contents(span.file_id)?;
        let before = contents[..span.start.offset as usize]
            .iter()
            .rev()
            .take_while(|b| b.is_ascii_alphanumeric() || b" \t\r\n_\\?|&()".contains(b))
            .count();
        span.start.offset -= before as u32;
        Some(span)
    })
}

pub(super) fn own_property_doc(
    c: &Checker,
    declaring: &ClassLikeMetadata,
    p: &mago_codex::metadata::property::PropertyMetadata,
    name: &[u8],
) -> Option<Type> {
    property_span(c, p).and_then(|span| {
        let (_, entry, _) = doc_at(c, span)?;
        let scope = scope_at(c, span, Some(declaring));
        entry
            .doc
            .var_types(&scope)
            .into_iter()
            .find(|(n, _)| n.is_empty() || n.as_bytes() == name)
            .map(|(_, t)| t)
    })
}

pub(super) fn property_of(
    c: &Checker,
    declaring: &ClassLikeMetadata,
    p: &mago_codex::metadata::property::PropertyMetadata,
    name: &[u8],
) -> Type {
    let native = p
        .type_declaration_metadata
        .as_ref()
        .map(|t| from_tunion(c, &t.type_union, declaring));
    let phpdoc = own_property_doc(c, declaring, p, name)
        .or_else(|| inherited_property_doc(c, declaring, name, 0));
    match native {
        Some(n) => decide_type(n, phpdoc),
        None => phpdoc.unwrap_or_else(mixed),
    }
}

pub(super) fn class_scope(c: &Checker, span: Span, k: &ClassLikeMetadata) -> NameScope {
    scope_at(c, span, Some(k))
}

pub(super) fn with_method_templates(
    scope: &NameScope,
    k: &ClassLikeMetadata,
    method: &str,
    tags: &[super::doc::TemplateTag],
) -> NameScope {
    let mut scope = scope.clone();
    let described = format!("method {}::{method}()", original_name(k));
    for t in tags {
        let template = TemplateType {
            name: t.name.clone(),
            scope: described.clone(),
            bound: t
                .bound
                .as_ref()
                .map_or(Type::Mixed { explicit: false }, |b| resolve_in(b, &scope)),
            default: t.default.as_ref().map(|d| resolve_in(d, &scope)),
            argument: false,
        };
        scope
            .templates
            .insert(t.name.clone(), Type::Template(Box::new(template)));
    }
    scope
}

fn constant_doc<'a>(c: &Checker<'a, '_>, at: Span) -> Option<&'a DocEntry> {
    let contents = c.file_contents(at.file_id)?;
    let before = &contents[..(at.start.offset as usize).min(contents.len())];
    let stop = before
        .iter()
        .rposition(|b| matches!(b, b';' | b'{' | b'}'))?;
    let end = before.windows(2).rposition(|w| w == b"*/")?;
    if end < stop {
        return None;
    }
    c.doc_before(at.file_id, end as u32 + 2)
        .map(|(_, entry)| entry)
}

fn constant_var(
    c: &Checker,
    doc: &Doc,
    at: Span,
    class: &ClassLikeMetadata,
) -> Option<Option<Type>> {
    let scope = scope_at(c, at, Some(class));
    let vars = doc.var_types(&scope);
    Some(match vars.as_slice() {
        [(n, t)] if n.is_empty() => Some(t.clone()),
        _ => None,
    })
}

pub(super) fn constant(
    c: &Checker,
    at: Span,
    name: &[u8],
    context: &ClassLikeMetadata,
) -> Option<Type> {
    let facts = c.file_facts(at.file_id)?;
    let start = facts
        .declarations
        .starts
        .get(&at.start.offset)
        .copied()
        .unwrap_or(at.start.offset);
    match c.doc_before(at.file_id, start) {
        Some((_, entry)) => constant_var(c, &entry.doc, at, context)?,
        None => inherited_constant_doc(c, context, name, 0)?,
    }
}

fn inherited_constant_doc(
    c: &Checker,
    class: &ClassLikeMetadata,
    name: &[u8],
    depth: usize,
) -> Option<Option<Type>> {
    if depth > 32 {
        return None;
    }
    let parents = class
        .direct_parent_class
        .iter()
        .chain(class.direct_parent_interfaces.iter())
        .filter_map(|p| c.codebase.get_class_like(p.as_bytes()));
    for parent in parents {
        let Some((declaring, constant)) = std::iter::once(parent)
            .chain(
                parent
                    .all_parent_classes
                    .iter()
                    .chain(parent.all_parent_interfaces.iter())
                    .filter_map(|a| c.codebase.get_class_like(a.as_bytes())),
            )
            .find_map(|k| k.constants.get(&mago_word::word(name)).map(|m| (k, m)))
        else {
            continue;
        };
        if constant.visibility == mago_codex::visibility::Visibility::Private {
            continue;
        }
        let own = constant_doc(c, constant.span)
            .and_then(|entry| constant_var(c, &entry.doc, constant.span, declaring));
        match own {
            Some(found) => return Some(found),
            None => match inherited_constant_doc(c, declaring, name, depth + 1) {
                Some(found) => return Some(found),
                None => continue,
            },
        }
    }
    None
}

pub(super) struct Classes<'c, 'a, 'arena> {
    checker: &'c Checker<'a, 'arena>,
    cache: RefCell<HashMap<String, Option<Arc<Class>>>>,
}

thread_local! {
    static BUILDING: RefCell<std::collections::HashSet<String>> = RefCell::new(std::collections::HashSet::new());
    static CYCLES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
pub(crate) struct ClassCache(std::sync::Mutex<HashMap<String, Option<Arc<Class>>>>);

impl<'c, 'a, 'arena> Classes<'c, 'a, 'arena> {
    pub(super) fn new(checker: &'c Checker<'a, 'arena>) -> Self {
        Classes {
            checker,
            cache: RefCell::new(HashMap::new()),
        }
    }

    pub(super) fn get(&self, name: &str) -> Option<Arc<Class>> {
        let key = name.to_ascii_lowercase();
        if let Some(found) = self.cache.borrow().get(&key) {
            return found.clone();
        }
        let shared = &self.checker.class_cache.0;
        if let Some(found) = shared.lock().expect("class cache").get(&key).cloned() {
            self.cache.borrow_mut().insert(key, found.clone());
            return found;
        }
        if !BUILDING.with_borrow_mut(|b| b.insert(key.clone())) {
            CYCLES.set(CYCLES.get() + 1);
            return None;
        }
        let cycles = CYCLES.get();
        let built = self.build(name).map(Arc::new);
        BUILDING.with_borrow_mut(|b| b.remove(&key));
        if CYCLES.get() == cycles {
            shared
                .lock()
                .expect("class cache")
                .insert(key.clone(), built.clone());
        }
        self.cache.borrow_mut().insert(key, built.clone());
        built
    }

    fn build(&self, name: &str) -> Option<Class> {
        let c = self.checker;
        // An anonymous class's object type carries its display name (`Shape@anonymous/a.php:6`), which names no class
        // in the codebase: find the class this file declares under it
        let k = c.codebase.get_class_like(name.as_bytes()).or_else(|| {
            let (internal, _) = c.anonymous.iter().find(|(_, display)| *display == name)?;
            c.codebase.get_class_like(internal)
        })?;
        let canonical = |w: &mago_word::Word| {
            c.codebase
                .get_class_like(w.as_bytes())
                .map_or_else(|| text(w.as_bytes()), original_name)
        };
        let mut pending: Vec<_> = k
            .direct_parent_class
            .iter()
            .chain(k.direct_parent_interfaces.iter())
            .collect();
        let mut ancestors = Vec::new();
        let mut seen = std::collections::HashSet::from([k.name]);
        while let Some(name) = pending.pop() {
            if name.as_bytes().starts_with(b"__internal_do_not_use__") || !seen.insert(*name) {
                continue;
            }
            ancestors.push(canonical(name));
            if let Some(parent) = c.codebase.get_class_like(name.as_bytes()) {
                pending.extend(
                    parent
                        .direct_parent_class
                        .iter()
                        .chain(parent.direct_parent_interfaces.iter()),
                );
            }
        }
        if k.kind == SymbolKind::Enum {
            ancestors.retain(|a| a != "UnitEnum" && a != "BackedEnum");
            ancestors.push("UnitEnum".to_string());
            ancestors.extend(k.enum_type.is_some().then(|| "BackedEnum".to_string()));
        }
        let own = class_templates(c, k);
        let generics = self.generics(k, &own);
        let variances = doc_at(c, k.span)
            .map(|(_, entry, _)| entry.doc.template_variances.clone())
            .unwrap_or_default();
        let templates = own
            .into_iter()
            .zip(variances)
            .map(|(t, variance)| Template {
                name: t.name,
                bound: serialize(&t.bound),
                default: t.default.as_ref().map(serialize),
                variance,
            })
            .collect();
        Some(Class {
            name: if k.name.as_bytes().starts_with(b"{anonymous-class:") {
                name.to_string()
            } else {
                original_name(k)
            },
            ancestors,
            templates,
            cases: k.enum_cases.keys().map(|w| text(w.as_bytes())).collect(),
            case_values: k
                .enum_cases
                .values()
                .filter_map(|m| {
                    let value = super::enums::constant_value(c, m.value_type.as_ref()?)?;
                    Some((text(m.name.as_bytes()), to_phpdoc(&value)))
                })
                .collect(),
            is_interface: k.kind == SymbolKind::Interface,
            is_trait: k.kind == SymbolKind::Trait,
            is_enum: k.kind == SymbolKind::Enum,
            is_final: k.flags.contains(MetadataFlags::FINAL),
            properties: k
                .declaring_property_ids
                .iter()
                .filter(|(name, declaring)| {
                    c.codebase
                        .get_property(declaring.as_bytes(), name.as_bytes())
                        .is_some_and(|p| !p.flags.is_static())
                })
                .map(|(name, _)| text(&name.as_bytes()[1..]))
                .collect(),
            allows_dynamic_properties: super::property_access::allows_dynamic_properties(c, k),
            constants: k
                .constants
                .values()
                .map(|m| {
                    let value = m.inferred_type.as_ref().map(|value| c.to_phpstan(value));
                    (
                        text(m.name.as_bytes()),
                        value.map_or_else(|| "mixed".to_string(), |t| to_phpdoc(&t)),
                    )
                })
                .collect(),
            methods: k
                .declaring_method_ids
                .iter()
                .map(|(m, id)| {
                    let f = symbols::Function {
                        name: text(m.as_bytes()),
                        side_effects: "Maybe".to_string(),
                        deprecated: false,
                        variants: Vec::new(),
                        named: None,
                        is_static: c
                            .codebase
                            .get_method_by_id(id)
                            .and_then(|f| f.method_metadata.as_ref())
                            .is_some_and(|m| m.is_static),
                        doc: None,
                    };
                    (text(m.as_bytes()).to_ascii_lowercase(), f)
                })
                .collect(),
            iterable: self.iterable(k),
            generics,
            property_docs: HashMap::new(),
        })
    }

    fn iterable(&self, k: &ClassLikeMetadata) -> Option<(String, String)> {
        let c = self.checker;
        let returned = |method: &str| {
            let id = k.declaring_method_ids.get(&mago_word::word(method))?;
            let f = c.codebase.get_method_by_id(id)?;
            let t = f
                .return_type_metadata
                .as_ref()
                .or(f.return_type_declaration_metadata.as_ref())?;
            Some(c.to_phpstan(&t.type_union))
        };
        let own = |t: &Type| matches!(t, Type::Object(n, ..) | Type::Static(n, _) | Type::This(n) if n.eq_ignore_ascii_case(&original_name(k)));
        if c.codebase.class_implements(k.name.as_bytes(), b"Iterator") {
            Some((
                serialize(&returned("key")?),
                serialize(&returned("current")?),
            ))
        } else if c
            .codebase
            .class_implements(k.name.as_bytes(), b"IteratorAggregate")
        {
            let t = returned("getiterator")?;
            if own(&t) {
                return None;
            }
            Some((
                serialize(&t.iterable_key_type()),
                serialize(&t.iterable_value_type()),
            ))
        } else {
            None
        }
    }

    fn generics(
        &self,
        k: &ClassLikeMetadata,
        own: &[TemplateType],
    ) -> HashMap<String, Vec<String>> {
        let c = self.checker;
        let canonical = |w: &mago_word::Word| {
            c.codebase
                .get_class_like(w.as_bytes())
                .map_or_else(|| text(w.as_bytes()), original_name)
        };
        let mut direct: HashMap<String, Vec<Type>> = HashMap::new();
        if let Some((_, entry, names)) = doc_at(c, k.span) {
            let doc = &entry.doc;
            let mut scope = NameScope {
                names: names.clone(),
                class: Some(original_name(k)),
                parent: parent_name(c.codebase, k),
                templates: HashMap::new(),
            };
            for t in own {
                scope
                    .templates
                    .insert(t.name.clone(), Type::Template(Box::new(t.clone())));
            }
            for node in &doc.ancestors {
                if let Type::Object(name, args, _) = resolve_in(node, &scope)
                    && !args.is_empty()
                {
                    let name = c
                        .codebase
                        .get_class_like(name.as_bytes())
                        .map_or(name, original_name);
                    direct.insert(name.to_ascii_lowercase(), args);
                }
            }
        }
        let mut out = HashMap::new();
        let parents = k
            .direct_parent_class
            .iter()
            .chain(k.direct_parent_interfaces.iter())
            .map(canonical);
        for parent in parents {
            let Some(p) = symbols::lookup(&parent) else {
                continue;
            };
            if p.templates.is_empty() {
                for (ancestor, args) in &p.generics {
                    out.entry(ancestor.clone()).or_insert_with(|| args.clone());
                }
                continue;
            }
            let args = direct
                .get(&parent.to_ascii_lowercase())
                .cloned()
                .unwrap_or_else(|| {
                    p.templates
                        .iter()
                        .map(|t| {
                            let s = t.default.as_deref().unwrap_or(&t.bound);
                            pun_phpstan::resolve::resolve_str(s, Some(&p.name))
                        })
                        .collect()
                });
            let mapped = p.generics.keys().filter_map(|ancestor| {
                Some((
                    ancestor,
                    pun_phpstan::types::ancestor_args(&p, ancestor, &args)?,
                ))
            });
            for (ancestor, mapped) in mapped {
                out.entry(ancestor.clone())
                    .or_insert_with(|| mapped.iter().map(serialize).collect());
            }
            out.insert(p.name.clone(), args.iter().map(serialize).collect());
        }
        out
    }
}

fn serialize(t: &Type) -> String {
    fn encode(t: &Type) -> Type {
        match t {
            Type::Template(tpl) => {
                let mut args = vec![
                    Type::Object(tpl.name.clone(), vec![], false),
                    Type::ConstString(tpl.scope.as_bytes().to_vec()),
                    encode(&tpl.bound),
                ];
                args.extend(tpl.default.as_ref().map(encode));
                Type::Object("__template".to_string(), args, false)
            }
            Type::Benevolent(types) => Type::Object(
                "__benevolent".to_string(),
                vec![Type::Union(types.iter().map(encode).collect())],
                false,
            ),
            _ => t.traverse(&encode),
        }
    }
    to_phpdoc(&encode(t))
}

pub(super) fn with_classes<R>(c: &Checker, f: impl FnOnce() -> R) -> R {
    let classes = Classes::new(c);
    let functions = |name: &str| c.codebase.get_function(name.as_bytes()).is_some();
    let properties = |t: &Type, name: &str| c.object_property(t, name);
    let Some(larastan) = c.larastan else {
        return symbols::with_classes(&|name| classes.get(name), || {
            symbols::with_functions(&functions, || {
                pun_phpstan::types::with_object_properties(&properties, f)
            })
        });
    };
    let strings = |query: pun_phpstan::types::StringQuery<'_>| match query {
        pun_phpstan::types::StringQuery::Property(t, name) => {
            c.has_property(t, format!("${name}").as_bytes(), false)
        }
        pun_phpstan::types::StringQuery::Column(table, column) => pun_phpstan::types::Tri::from(
            larastan
                .table_columns
                .get(table)
                .is_some_and(|columns| columns.contains(column)),
        ),
        pun_phpstan::types::StringQuery::View(name) => pun_phpstan::types::Tri::from(
            crate::larastan::views::Finder::new(&larastan.root)
                .find(name)
                .is_some(),
        ),
    };
    let late = |name: &str, args: &[Type]| super::larastan::resolve_late(c.codebase, name, args);
    symbols::with_classes(&|name| classes.get(name), || {
        symbols::with_functions(&functions, || {
            pun_phpstan::types::with_larastan_strings(&strings, || {
                pun_phpstan::types::with_larastan_late_types(&late, || {
                    pun_phpstan::types::with_object_properties(&properties, f)
                })
            })
        })
    })
}
