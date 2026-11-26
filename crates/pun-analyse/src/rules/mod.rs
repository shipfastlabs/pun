mod anonymous;
mod array_column;
mod array_fill;
mod array_sum;
mod arrays;
mod assert_tags;
mod assign;
mod attributes;
mod bind_scope;
mod by_ref_foreach;
mod callable_tags;
mod calls;
mod class_names;
mod class_reflection;
mod class_tags;
mod classes;
mod conditional_return;
mod constants;
mod date_format;
mod date_interval;
mod date_time;
mod dead_code;
mod debug;
mod declaration_effects;
mod declaration_purity;
mod declarations;
mod declared;
mod deprecations;
mod disallowed;
mod doc;
mod driver;
mod dynamic_fetch;
mod enum_methods;
mod enums;
mod exception_codes;
mod facts;
mod flip_intersect;
mod function_callable;
mod function_checks;
mod functions;
mod generics;
mod hierarchy;
mod identifiers;
mod include_file;
mod infer;
mod internal;
mod internal_usage;
mod invalid_throws;
mod larastan;
mod members;
mod method_callable;
mod method_reflection;
mod missing_types;
mod named_argument;
mod names;
mod new_static_abstract;
mod operators;
mod overriding;
mod overriding_property;
mod params;
mod php_rules;
mod phpat;
mod phpdoc_tags;
mod phpdoc_types;
mod printf_helper;
mod properties;
mod property_access;
mod property_hooks;
mod purity_points;
mod randomizer;
mod readonly_init;
mod reflection;
mod regex_shapes;
mod require_ancestors;
mod require_tags;
mod restricted_functions;
mod return_types;
mod scope;
mod selector;
mod set_hook;
mod signatures;
mod statements;
mod strict;
mod syntax;
mod through_static;
mod type_aliases;
mod type_coverage;
mod types;
mod unset;
mod unused_trait;
mod var_names;
mod var_tags;
mod variables;
mod variance;
mod walker;
mod write_only;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::class_like::ClassLikeMetadata;
use mago_codex::symbol::SymbolKind;
use mago_codex::ttype::union::TUnion;
use mago_database::file::{File, FileId};
use mago_names::ResolvedNames;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;

use pun_phpstan::ignore::Error;

use crate::codebase::Sources;
use scope::{Guard, Saved};
use statements::{Frame, FrameKind};

pub(crate) use dead_code::conditions_in_traits;
pub(crate) use dead_code::impure_points;
pub(crate) use dead_code::{dependency_keys, key_symbol};
pub(crate) use driver::Switches;
pub(crate) use driver::{Checks, Natives, check_file};
pub(crate) use facts::Comments;
pub(crate) use facts::Facts;
pub(crate) use identifiers::IDENTIFIERS;
pub(crate) use larastan::finish as larastan_finish;
pub(crate) use larastan::late_builder;
pub use larastan::{Larastan, LarastanSwitches};
pub(crate) use named_argument::is_call_record as is_named_argument_call;
pub(crate) use named_argument::resolve as renamed_named_arguments;
pub(crate) use reflection::builtins;
pub(crate) use unused_trait::unused_traits;

pub(crate) fn reports(identifier: &str) -> bool {
    IDENTIFIERS.contains(&identifier)
        || declaration_purity::IDENTIFIERS.contains(&identifier)
        || missing_types::IDENTIFIERS.contains(&identifier)
        || generics::IDENTIFIERS.contains(&identifier)
        || larastan::IDENTIFIERS.contains(&identifier)
        || types::IDENTIFIERS
            .iter()
            .any(|ids| ids.contains(&identifier))
        || phpdoc_tags::IDENTIFIERS.contains(&identifier)
        || require_tags::IDENTIFIERS.contains(&identifier)
        || var_tags::IDENTIFIERS.contains(&identifier)
        || class_tags::IDENTIFIERS.contains(&identifier)
        || callable_tags::IDENTIFIERS.contains(&identifier)
        || operators::IDENTIFIERS.contains(&identifier)
        || conditional_return::IDENTIFIERS.contains(&identifier)
        || assert_tags::IDENTIFIERS.contains(&identifier)
        || phpdoc_types::IDENTIFIERS.contains(&identifier)
        || invalid_throws::IDENTIFIERS.contains(&identifier)
        || var_names::IDENTIFIERS.contains(&identifier)
        || variance::IDENTIFIERS.contains(&identifier)
        || date_interval::IDENTIFIERS.contains(&identifier)
        || date_time::IDENTIFIERS.contains(&identifier)
        || by_ref_foreach::IDENTIFIERS.contains(&identifier)
}

pub(crate) const DISCOVERING_SYMBOLS_TIP: &str =
    "Learn more at https://phpstan.org/user-guide/discovering-symbols";

struct ClassScope {
    name: Option<String>,
    constructor_guards: Vec<Guard>,
    /// A closure's `Closure::bind()`, `@param-closure-this` or macro binding, not a class declaration.
    bound: bool,
}

fn out_of_class(e: &Expression) -> &'static str {
    match e {
        Expression::Self_(_) => "outOfClass.self",
        Expression::Static(_) => "outOfClass.static",
        _ => "outOfClass.parent",
    }
}

fn restricted(v: mago_codex::visibility::Visibility) -> &'static str {
    if v.is_private() {
        "private"
    } else {
        "protected"
    }
}

struct TraitContext {
    class_file: String,
    class: String,
    trait_name: String,
    renames: HashMap<String, String>,
}

pub(crate) struct FunctionScope {
    pub name: String,
    pub kind: FunctionScopeKind,
    pub deprecated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FunctionScopeKind {
    Function,
    Method { is_static: bool },
}

bitflags::bitflags! {
    #[derive(Clone, Copy)]
    struct Walk: u8 {
        const COMPILER_HALT = 1;
        const IN_TARGET = 1 << 1;
        const FIRST_STATEMENT = 1 << 2;
    }
}

pub(crate) struct Checker<'a, 'arena> {
    file: &'a File,
    walk: Walk,
    anonymous: HashMap<Vec<u8>, String>,
    property_access: property_access::State,
    relative_path: &'a crate::relative_path::RelativePath,
    types: HashMap<Span, TUnion>,
    keep_void_types: HashMap<Span, TUnion>,
    native_types: HashMap<Span, TUnion>,
    nullsafe_method_receiver_types: HashMap<Span, TUnion>,
    property_offset_writes: HashMap<Span, TUnion>,
    variable_writes: HashMap<Span, mago_analyzer::variable_flow::VariableWritesNode>,
    property_initializations:
        HashMap<Span, mago_analyzer::property_initialization::PropertyInitializationCertainty>,
    call_terminations: HashMap<(u32, u32), bool>,
    named_call_scopes: HashMap<Span, Vec<infer::NamedCallScope>>,
    defined: HashMap<Span, pun_phpstan::types::Tri>,
    variables: variables::Observations,
    origins: infer::Origins,
    /// `source_type` results on `types`, by span, `KEEP_FILLED` and class scope, with whether the result set
    /// `WITHOUT_SIGNATURE`. Each call in a fluent chain asks for its source, which recurses through every call before
    /// it, so without this a chain of n calls costs n² resolutions (symfony/messenger's config tree: 16 s for one file).
    source_types: std::sync::Mutex<
        HashMap<(Span, bool, Option<mago_word::Word>), (Option<pun_phpstan::types::Type>, bool)>,
    >,
    names: &'a ResolvedNames<'arena>,
    codebase: &'a CodebaseMetadata,
    classes: Vec<ClassScope>,
    guards: Vec<Guard>,
    marks: Vec<usize>,
    vars: HashMap<Vec<u8>, Vec<Guard>>,
    saved: Vec<Saved>,
    functions: Vec<FunctionScope>,
    var_tag_functions: Vec<mago_span::Span>,
    in_exists_call: usize,
    top_level: HashSet<(u32, u32)>,
    piped: HashSet<Span>,
    rebound: HashMap<Span, bind_scope::Binding>,
    label: String,
    trait_context: Option<TraitContext>,
    trait_uses: Vec<driver::TraitUse>,
    renamed: Option<(Span, String)>,
    php_version: u32,
    builtins: &'static pun_phpstan::symbols::Symbols,
    natives: &'a driver::Natives,
    resolving_aliases: std::sync::atomic::AtomicBool,
    trait_lines: Vec<(i64, i64)>,
    frames: Vec<Frame>,
    namespaces: Vec<Option<String>>,
    magic_defaults: HashSet<Span>,
    declares_not_first: Vec<Span>,
    decls: Vec<declarations::Decl>,
    immutable: HashMap<String, bool>,
    files: &'a HashMap<FileId, PathBuf>,
    sources: &'a Sources,
    facts: &'a facts::Facts,
    function_likes: &'a HashMap<FileId, Vec<(Span, mago_codex::symbol::SymbolIdentifier)>>,
    patched_docs: &'a HashMap<Span, Span>,
    class_cache: &'a declared::ClassCache,
    anonymous_names: &'a anonymous::Cache,
    trait_properties: &'a readonly_init::Cache,
    syntax: syntax::State,
    assign: assign::State,
    signatures: signatures::State,
    class_names: class_names::State,
    arrays: arrays::State,
    standalone_throws: HashSet<Span>,
    level: u8,
    switches: driver::Switches,
    larastan: Option<&'a Larastan>,
    larastan_state: larastan::State,
    internal: Option<&'a internal::Names>,
    unreachable: dead_code::UnreachableStatements,
    conditions: dead_code::ConditionFacts,
    strict: crate::StrictRules,
    exceptions: &'a crate::Exceptions,
    too_wide: crate::TooWide,
    include_settings: &'a include_file::Settings,
    include_state: include_file::State,
    closure_bind: usize,
    toggled: &'a driver::Toggled,
    by_ref_foreach: by_ref_foreach::Entries,
    method_rules: crate::MethodRules,
    errors: Vec<Error>,
}

impl<'a, 'arena> Checker<'a, 'arena> {
    fn file_contents(&self, id: FileId) -> Option<&'a [u8]> {
        if id == self.file.id {
            return Some(&self.file.contents[..]);
        }
        self.sources.scanned(id).map(|s| &s.file.contents[..])
    }

    fn file_facts(&self, id: FileId) -> Option<&'a facts::Facts> {
        if id == self.file.id {
            return Some(self.facts);
        }
        self.sources.scanned(id).map(|s| &s.facts)
    }

    fn php_doc_comment(&self, id: FileId, offset: u32) -> Option<&'a [u8]> {
        self.file_facts(id)?
            .comments
            .php_doc_comment(self.file_contents(id)?, offset as usize)
    }

    fn php_doc(
        &self,
        id: FileId,
        offset: u32,
    ) -> Option<(&'a [u8], std::borrow::Cow<'a, facts::DocEntry>)> {
        let raw = self.php_doc_comment(id, offset)?;
        let start = raw.as_ptr() as usize - self.file_contents(id)?.as_ptr() as usize;
        let entry = self.file_facts(id)?.comments.entry(
            self.file_contents(id)?,
            start as u32,
            (start + raw.len()) as u32,
        );
        Some((
            raw,
            entry.map_or_else(
                || std::borrow::Cow::Owned(facts::DocEntry::parse(raw, true)),
                std::borrow::Cow::Borrowed,
            ),
        ))
    }

    fn doc_before(&self, id: FileId, offset: u32) -> Option<(&'a [u8], &'a facts::DocEntry)> {
        self.file_facts(id)?
            .comments
            .doc_before(self.file_contents(id)?, offset as usize)
    }

    fn report_no_parent(&mut self, at: impl HasSpan, scope: &ClassLikeMetadata, action: &str) {
        let display = self.display(scope);
        self.report(
            at,
            "class.noParent",
            format!("{action} but {display} does not extend any class."),
            None,
        );
    }

    fn function_name(&self) -> String {
        self.functions
            .last()
            .map(|f| f.name.clone())
            .unwrap_or_default()
    }

    /// In a trait's context, a declaration outside that trait reports nothing, so its walk is skipped.
    fn outside_target_trait(&self) -> bool {
        self.trait_context.is_some() && !self.walk.contains(Walk::IN_TARGET)
    }

    fn in_trait(&self) -> Option<&str> {
        self.trait_context
            .as_ref()
            .filter(|_| self.walk.contains(Walk::IN_TARGET))
            .map(|t| t.trait_name.as_str())
    }

    fn file_name(&self) -> String {
        String::from_utf8_lossy(&self.file.name).into_owned()
    }

    fn report(&mut self, at: impl HasSpan, identifier: &str, message: String, tip: Option<&str>) {
        let line = self.line_of(at.span());
        self.report_at_line(line, identifier, message, tip);
    }

    fn line_of(&self, at: Span) -> i64 {
        i64::from(self.file.line_number(at.start.offset)) + 1
    }

    fn report_at_line(&mut self, line: i64, identifier: &str, message: String, tip: Option<&str>) {
        debug_assert!(reports(identifier), "{identifier} is not in IDENTIFIERS");
        if self.trait_context.is_some() && !self.walk.contains(Walk::IN_TARGET) {
            return;
        }
        self.errors.push(Error {
            identifier: Some(identifier.to_string()),
            tip: tip.map(str::to_string),
            file_path: self
                .trait_context
                .as_ref()
                .map_or_else(|| self.file_name(), |t| t.class_file.clone()),
            trait_file_path: self.trait_context.as_ref().map(|_| self.file_name()),
            ..Error::new(message, self.label.clone(), Some(line))
        });
    }

    fn fixable_since(&mut self, count: usize) {
        for e in &mut self.errors[count..] {
            e.fixable = true;
        }
    }

    fn report_non_ignorable(
        &mut self,
        at: impl HasSpan,
        identifier: &str,
        message: String,
        tip: Option<&str>,
    ) {
        let count = self.errors.len();
        self.report(at, identifier, message, tip);
        if let Some(e) = self.errors.get_mut(count) {
            e.can_be_ignored = false;
        }
    }

    fn written_or_resolved(&self, id: &Identifier<'arena>) -> String {
        let text = match id {
            Identifier::Local(l) if !self.names.is_imported(&l.span) => l.value,
            _ => self.names.get(&id.span()),
        };
        String::from_utf8_lossy(text).into_owned()
    }

    fn is_closure_bind(&self, call: &StaticMethodCall<'arena>) -> bool {
        matches!((call.class, &call.method), (Expression::Identifier(id), ClassLikeMemberSelector::Identifier(m))
            if self.resolved(id).eq_ignore_ascii_case("Closure") && m.value.eq_ignore_ascii_case(b"bind"))
    }

    fn rebind(&mut self, e: &Expression, class: Option<String>, defines_this: bool) {
        let e = types::flow::unparen(e);
        if matches!(e, Expression::Closure(_) | Expression::ArrowFunction(_)) {
            self.rebound.insert(
                e.span(),
                bind_scope::Binding {
                    class,
                    defines_this,
                },
            );
        }
    }

    fn trait_uses_of(&mut self, class: &str, members: &[ClassLikeMember<'arena>]) {
        for m in members {
            let ClassLikeMember::TraitUse(u) = m else {
                continue;
            };
            let adaptations = match &u.specification {
                TraitUseSpecification::Concrete(s) => s.adaptations.as_slice(),
                TraitUseSpecification::Abstract(_) => &[],
            };
            for id in &u.trait_names {
                let trait_name = self.resolved(id);
                let mut renames = HashMap::new();
                for a in adaptations {
                    let TraitUseAdaptation::Alias(a) = a else {
                        continue;
                    };
                    let Some(alias) = &a.alias else {
                        continue;
                    };
                    let method = match &a.method_reference {
                        TraitUseMethodReference::Identifier(m) => m,
                        TraitUseMethodReference::Absolute(r) => {
                            if !self
                                .written(r.trait_name.span())
                                .eq_ignore_ascii_case(&self.written(id.span()))
                            {
                                continue;
                            }
                            &r.method_name
                        }
                    };
                    renames.insert(
                        String::from_utf8_lossy(method.value).to_ascii_lowercase(),
                        String::from_utf8_lossy(alias.value).into_owned(),
                    );
                }
                self.trait_uses.push(driver::TraitUse {
                    class: class.to_string(),
                    trait_name,
                    renames,
                    php_versions: self
                        .guards
                        .iter()
                        .filter(|g| matches!(g, Guard::PhpVersion(..)))
                        .cloned()
                        .collect(),
                });
            }
        }
    }

    fn shadowed(&self, class: &str, name: &str, trait_span: Span) -> bool {
        let this_trait = |f: &mago_codex::metadata::function_like::FunctionLikeMetadata| {
            f.span.file_id == trait_span.file_id
                && trait_span.start.offset <= f.span.start.offset
                && f.span.end.offset <= trait_span.end.offset
        };
        self.codebase
            .get_class_like(class.as_bytes())
            .is_some_and(|k| {
                let lower = name.to_ascii_lowercase();
                (k.kind == SymbolKind::Enum
                    && (lower == "cases"
                        || (k.enum_type.is_some() && (lower == "from" || lower == "tryfrom"))))
                    || members::method(self, k, name.as_bytes()).is_some_and(
                        |m| !matches!(m, members::Method::User { f, .. } if this_trait(f)),
                    )
            })
    }

    fn written(&self, span: Span) -> String {
        String::from_utf8_lossy(&self.file.contents[span.to_range_usize()]).into_owned()
    }

    /// Pun names classes with UTF-8 strings, so it doesn't walk a class-like whose name isn't UTF-8 (Symfony's
    /// `class \xA9`): the codebase keys it by the raw bytes, and lookups by the lossy name miss it.
    fn utf8_name(&self, span: Span) -> bool {
        std::str::from_utf8(self.names.get(&span)).is_ok()
    }

    fn resolved_span(&self, span: Span) -> String {
        String::from_utf8_lossy(self.names.get(&span)).into_owned()
    }

    fn resolved(&self, id: &Identifier<'arena>) -> String {
        String::from_utf8_lossy(self.names.get(&id.span())).into_owned()
    }

    fn class_scope(&self) -> Option<&'a ClassLikeMetadata> {
        let name = self.classes.last()?.name.as_deref()?;
        self.codebase.get_class_like(name.as_bytes())
    }

    fn member_scope(&self) -> (&'a ClassLikeMetadata, String) {
        let class = self
            .class_scope()
            .expect("a member is walked inside its class-like");
        let display = self
            .decls
            .last()
            .and_then(|d| d.display.clone())
            .expect("a class-like's declaration has its display name");
        (class, display)
    }

    fn in_anonymous_class(&self) -> bool {
        self.trait_context.is_none()
            && self
                .classes
                .last()
                .and_then(|k| k.name.as_deref())
                .is_some_and(|n| n.starts_with("{anonymous-class:"))
    }

    fn enter_class(&mut self, name: Option<String>, members: &[ClassLikeMember<'arena>]) {
        let constructor_guards = members
            .iter()
            .find_map(|m| match m {
                ClassLikeMember::Method(m) if m.name.value.eq_ignore_ascii_case(b"__construct") => {
                    match &m.body {
                        MethodBody::Concrete(b) => Some(self.exit_guards(b.statements.iter())),
                        MethodBody::Abstract(_) => None,
                    }
                }
                _ => None,
            })
            .unwrap_or_default();
        self.classes.push(ClassScope {
            name,
            constructor_guards,
            bound: false,
        });
    }

    fn mark_top_level(&mut self, e: &Expression) {
        let e = types::flow::unparen(e);
        let span = e.span();
        self.top_level.insert((span.start.offset, span.end.offset));
        match e {
            Expression::Conditional(c) => {
                if let Some(then) = c.then {
                    self.mark_top_level(then);
                }
                self.mark_top_level(c.r#else);
            }
            Expression::Binary(b)
                if matches!(
                    b.operator,
                    BinaryOperator::And(_)
                        | BinaryOperator::LowAnd(_)
                        | BinaryOperator::Or(_)
                        | BinaryOperator::LowOr(_)
                ) =>
            {
                self.mark_top_level(b.rhs);
            }
            Expression::UnaryPrefix(u)
                if matches!(u.operator, UnaryPrefixOperator::ErrorControl(_)) =>
            {
                self.mark_top_level(u.operand);
            }
            _ => {}
        }
    }

    fn is_top_level(&self, span: Span) -> bool {
        self.top_level
            .contains(&(span.start.offset, span.end.offset))
    }
}

impl Checker<'_, '_> {
    pub(crate) fn on(&self, switch: Switches) -> bool {
        self.switches.contains(switch)
    }
}
