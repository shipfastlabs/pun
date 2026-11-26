//! phpat 0.12.5's rules (`PHPat\Rule\Assertion\*`): the declaration assertions, which check each class once
//! (`InClassNode`), and the relation assertions, which check the class names each node names
//! (`PHPat\Rule\Extractor\*`). Each rule class runs on its own, as PHPStan runs it, so a `should` rule reports from
//! every rule class of its assertion.

use std::collections::HashMap;
use std::sync::LazyLock;

use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::class_like::ClassLikeMetadata;
use mago_codex::visibility::Visibility;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;
use pun_phpstan::ignore::Error;
use pun_phpstan::phpdoc::TagValue;
use pun_phpstan::resolve::resolve_in;
use pun_phpstan::types::Type;

use super::Checker;
use super::doc::NameScope;
use crate::phpat::{Constraint, Kind, Rules, Selector, Statement as Assertion};

/// `PHPat\Parser\BuiltInClasses::PHP_BUILT_IN_CLASSES`.
static BUILT_IN: LazyLock<std::collections::HashSet<&'static str>> = LazyLock::new(|| {
    include_str!("../phpat/built_in_classes.txt")
        .lines()
        .collect()
});

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// `trimSeparators()`.
fn trim(s: &str) -> &str {
    s.trim_matches('\\')
}

/// What phpat asks PHPStan's `ClassReflection` about a class, answered from the codebase.
struct Reflect<'c> {
    codebase: &'c CodebaseMetadata,
    files: &'c HashMap<mago_database::file::FileId, std::path::PathBuf>,
}

impl<'c> Reflect<'c> {
    fn class(&self, name: &str) -> Option<&'c ClassLikeMetadata> {
        self.codebase.get_class_like(trim(name).as_bytes())
    }

    fn by_word(&self, w: mago_word::Word) -> Option<&'c ClassLikeMetadata> {
        self.codebase.get_class_like(w.as_bytes())
    }

    fn name(k: &ClassLikeMetadata) -> String {
        text(k.original_name.as_bytes())
    }

    fn parent(&self, k: &ClassLikeMetadata) -> Option<&'c ClassLikeMetadata> {
        k.direct_parent_class.and_then(|p| self.by_word(p))
    }

    fn parents(&self, k: &ClassLikeMetadata) -> Vec<&'c ClassLikeMetadata> {
        let mut out: Vec<&'c ClassLikeMetadata> = Vec::new();
        let mut next = self.parent(k);
        // A cycle, which PHP rejects, ends the walk.
        while let Some(p) =
            next.filter(|p| !out.iter().any(|o| o.name == p.name) && out.len() <= 256)
        {
            out.push(p);
            next = self.parent(p);
        }
        out
    }

    /// The interfaces `k` implements, inherited ones included (`getInterfaces()`, the native `getInterfaceNames()`).
    fn interfaces(&self, k: &ClassLikeMetadata) -> Vec<&'c ClassLikeMetadata> {
        (k.all_parent_interfaces.iter())
            .filter_map(|i| self.by_word(*i))
            .collect()
    }

    /// `getImmediateInterfaces()`: the interfaces that neither a parent class nor another interface brings.
    fn immediate_interfaces(&self, k: &ClassLikeMetadata) -> Vec<&'c ClassLikeMetadata> {
        let all = self.interfaces(k);
        let mut indirect: Vec<mago_word::Word> = Vec::new();
        for p in self.parents(k) {
            indirect.extend(p.all_parent_interfaces.iter().copied());
        }
        for i in &all {
            indirect.extend(i.all_parent_interfaces.iter().copied());
        }
        all.into_iter()
            .filter(|i| !indirect.contains(&i.name))
            .collect()
    }

    /// The traits `k` itself uses (the native `getTraits()`).
    fn traits(&self, k: &ClassLikeMetadata) -> Vec<&'c ClassLikeMetadata> {
        (k.direct_trait_names())
            .filter_map(|t| self.codebase.get_class_like(t.as_bytes()))
            .collect()
    }

    /// `hasTraitUse()`'s names: the traits of `k` and of its traits, then those its parents use themselves.
    fn trait_names(&self, k: &ClassLikeMetadata) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut queue = self.traits(k);
        while let Some(t) = queue.pop() {
            let name = Self::name(t);
            if !out.contains(&name) && out.len() < 256 {
                out.push(name);
                queue.extend(self.traits(t));
            }
        }
        for p in self.parents(k) {
            for t in self.traits(p) {
                let name = Self::name(t);
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
        out
    }

    fn attributes(&self, k: &ClassLikeMetadata) -> Vec<String> {
        (k.attributes.iter())
            .map(|a| text(a.name.as_bytes()))
            .collect()
    }

    fn implements(&self, k: &ClassLikeMetadata, interface: &str) -> bool {
        let interface = interface.trim_start_matches('\\');
        (self.interfaces(k).iter()).any(|i| Self::name(i) == interface)
    }

    /// `isSubclassOf()`: `class` is known, not final, and a parent of `k`.
    fn is_subclass_of(&self, k: &ClassLikeMetadata, class: &str) -> bool {
        self.class(class).is_some_and(|c| !c.flags.is_final())
            && (self.parents(k).iter()).any(|p| Self::name(p) == class)
    }

    fn file(&self, k: &ClassLikeMetadata) -> Option<String> {
        (self.files.get(&k.span.file_id)).map(|p| p.to_string_lossy().into_owned())
    }

    /// The public methods the native reflection lists, inherited ones included.
    fn public_methods(&self, k: &ClassLikeMetadata) -> Vec<String> {
        (k.appearing_method_ids.values())
            .filter_map(|id| {
                let f = self
                    .codebase
                    .function_likes
                    .get(&(id.get_class_name(), id.get_method_name()))?;
                let m = f.method_metadata.as_ref()?;
                (m.visibility == Visibility::Public).then(|| text(f.original_name.as_bytes()))
            })
            .collect()
    }

    fn has_method(k: &ClassLikeMetadata, lower: &[u8]) -> bool {
        k.appearing_method_ids.keys().any(|m| m.as_bytes() == lower)
            || k.pseudo_methods.iter().any(|m| m.as_bytes() == lower)
    }

    /// `isBuiltInClass()` (RelationAssertion).
    fn is_built_in(&self, name: &str) -> bool {
        BUILT_IN.contains(name) || self.class(name).is_some_and(|k| k.flags.is_built_in())
    }

    fn matches(&self, s: &Selector, k: &ClassLikeMetadata) -> bool {
        let name = Self::name(k);
        match s {
            Selector::All => true,
            Selector::IsException => self.is_subclass_of(k, "Exception"),
            Selector::IsError => self.is_subclass_of(k, "Error"),
            Selector::IsThrowable => self.implements(k, "Throwable"),
            Selector::IsInterface => k.kind.is_interface(),
            Selector::IsAbstract => k.flags.is_abstract(),
            Selector::IsFinal => k.flags.is_final(),
            Selector::IsTrait => k.kind.is_trait(),
            Selector::IsReadonly => k.flags.is_readonly(),
            Selector::IsEnum => k.kind.is_enum(),
            Selector::IsAttribute => k.kind.is_class() && k.attribute_flags.is_some(),
            Selector::IsStandardClass => BUILT_IN.contains(name.as_str()),
            Selector::Namespace(p) => {
                let namespace = name.rsplit_once('\\').map_or("", |(n, _)| n);
                if p.regex.is_some() {
                    p.is_match(namespace)
                } else {
                    format!("{}\\", trim(namespace)).starts_with(&format!("{}\\", trim(&p.text)))
                }
            }
            Selector::Classname(p) => {
                if p.regex.is_some() {
                    p.is_match(&name)
                } else {
                    name == trim(&p.text)
                }
            }
            Selector::Filepath(p) => self.file(k).is_some_and(|f| {
                if p.regex.is_some() {
                    p.is_match(&f)
                } else {
                    f == trim(&p.text)
                }
            }),
            Selector::Implements(p) => {
                if p.regex.is_some() {
                    (self.interfaces(k).iter()).any(|i| p.is_match(&Self::name(i)))
                } else {
                    self.implements(k, &p.text)
                }
            }
            Selector::Extends(p) => {
                let parents = self.parents(k);
                if p.regex.is_some() {
                    parents.iter().any(|i| p.is_match(&Self::name(i)))
                } else {
                    parents.iter().any(|i| Self::name(i) == trim(&p.text))
                }
            }
            Selector::AppliesAttribute(p) => {
                let attributes = self.attributes(k);
                if p.regex.is_some() {
                    attributes.iter().any(|a| p.is_match(a))
                } else {
                    attributes.contains(&p.text)
                }
            }
            Selector::Includes(p) => {
                if p.regex.is_some() {
                    (self.traits(k).iter()).any(|t| p.is_match(&Self::name(t)))
                } else {
                    self.trait_names(k).contains(&p.text)
                }
            }
            Selector::AllOf(s) => s.iter().all(|s| self.matches(s, k)),
            Selector::AnyOf(s) => s.iter().any(|s| self.matches(s, k)),
            Selector::NoneOf(s) => !s.iter().any(|s| self.matches(s, k)),
            Selector::AtLeastCountOf(n, s) => self.count(s, k) >= *n,
            Selector::AtMostCountOf(n, s) => self.count(s, k) <= *n,
            Selector::OneOf(s) => self.count(s, k) == 1,
        }
    }

    fn count(&self, s: &[Selector], k: &ClassLikeMetadata) -> i64 {
        s.iter().filter(|s| self.matches(s, k)).count() as i64
    }

    /// The statement applies to `k`: it matches the subject and no subject exclude.
    fn applies(&self, st: &Assertion, k: &ClassLikeMetadata) -> bool {
        !k.flags.is_built_in()
            && self.matches(&st.subject, k)
            && !st.subject_excludes.iter().any(|x| self.matches(x, k))
    }

    /// `nodeMatchesTarget()`.
    fn node_matches(&self, node: &str, target: &Selector, excludes: &[Selector]) -> bool {
        self.class(node).is_some_and(|k| {
            self.matches(target, k) && !excludes.iter().any(|x| self.matches(x, k))
        })
    }
}

fn verb(kind: Kind) -> &'static str {
    match kind {
        Kind::Depend => "depend on",
        Kind::Construct => "construct",
        Kind::Extend => "extend",
        Kind::Implement => "implement",
        Kind::Include => "include",
        _ => "apply the attribute",
    }
}

struct Walk<'c, 'arena> {
    r: Reflect<'c>,
    c: &'c Checker<'c, 'arena>,
    rules: &'c Rules,
    subject: &'c ClassLikeMetadata,
    /// The class-like whose code is walked: the subject, or the trait it uses.
    declaring: &'c ClassLikeMetadata,
    errors: Vec<Error>,
}

impl<'c, 'arena> Walk<'c, 'arena> {
    fn report(&mut self, line: i64, st: &Assertion, message: String) {
        let c = self.c;
        let mut e = Error::new(
            self.rules.message(&st.rule_name, message),
            c.label.clone(),
            Some(line),
        );
        e.identifier = Some(format!("phpat.{}", st.rule_name));
        e.tip = match st.tips.as_slice() {
            [] => None,
            [tip] => Some(tip.clone()),
            tips => Some(
                (tips.iter().map(|t| format!("• {t}")))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        };
        e.can_be_ignored = !st.non_ignorable;
        if let Some(t) = &c.trait_context {
            e.file_path.clone_from(&t.class_file);
            e.trait_file_path = Some(c.file_name());
        }
        self.errors.push(e);
    }

    fn line(&self, span: Span) -> i64 {
        self.c.line_of(span)
    }

    /// `RelationAssertion::processNode()` for one rule class of `kind` on a node at `line` that names `nodes`.
    fn relation(&mut self, kind: Kind, nodes: Vec<String>, line: i64) {
        let rules = self.rules;
        let subject = self.subject;
        let subject_name = Reflect::name(subject);
        let should = rules.of(kind).any(|s| s.constraint == Constraint::Should);
        if !should && !nodes.iter().any(|n| subject_name != trim(n)) {
            return;
        }
        let nodes: Vec<String> = if rules.ignore_built_in_classes {
            nodes
                .into_iter()
                .filter(|n| !self.r.is_built_in(n))
                .collect()
        } else {
            nodes
        };
        let negation = |c| if c == Constraint::Should { "" } else { " not" };
        let message = |c, target: &str| {
            format!(
                "{subject_name} should{} {} {target}",
                negation(c),
                verb(kind)
            )
        };
        for st in rules.of(kind) {
            if !self.r.applies(st, subject) {
                continue;
            }
            match st.constraint {
                Constraint::Should => {
                    for target in &st.targets {
                        if !(nodes.iter())
                            .any(|n| self.r.node_matches(n, target, &st.target_excludes))
                        {
                            self.report(line, st, message(st.constraint, &target.name()));
                        }
                    }
                }
                Constraint::ShouldNot => {
                    for target in &st.targets {
                        for n in &nodes {
                            if self.r.node_matches(n, target, &st.target_excludes) {
                                self.report(line, st, message(st.constraint, n));
                            }
                        }
                    }
                }
                Constraint::CanOnly => {
                    for n in &nodes {
                        if !(st.targets.iter())
                            .any(|t| self.r.node_matches(n, t, &st.target_excludes))
                        {
                            self.report(line, st, message(st.constraint, n));
                        }
                    }
                }
            }
        }
    }

    /// `DeclarationAssertion::processNode()` for the rule class of `kind`.
    fn declaration(&mut self, kind: Kind, line: i64) {
        let k = self.subject;
        let name = Reflect::name(k);
        for st in self.rules.of(kind) {
            if !self.r.applies(st, k) {
                continue;
            }
            let pattern = st.name.as_ref();
            let named = |s: &str| {
                pattern.is_some_and(|p| {
                    if p.regex.is_some() {
                        p.is_match(s)
                    } else {
                        s == trim(&p.text)
                    }
                })
            };
            let methods = || {
                (self.r.public_methods(k).into_iter())
                    .filter(|m| m != "__construct")
                    .collect::<Vec<_>>()
            };
            let meets = match kind {
                Kind::Exist => true,
                Kind::BeAbstract => k.flags.is_abstract(),
                Kind::BeFinal => k.flags.is_final(),
                Kind::BeReadonly => k.flags.is_readonly(),
                Kind::BeEnum => k.kind.is_enum(),
                Kind::BeInterface => k.kind.is_interface(),
                Kind::BeInvokable => Reflect::has_method(k, b"__invoke"),
                Kind::BeNamed => named(&name),
                Kind::HaveOnlyOnePublicMethod => methods().len() == 1,
                // HaveOnlyOnePublicMethodNamed, the last kind `class_like` asks for
                _ => match methods().as_slice() {
                    [m] => pattern.is_some_and(|p| {
                        if p.regex.is_some() {
                            p.is_match(m)
                        } else {
                            *m == p.text
                        }
                    }),
                    _ => false,
                },
            };
            // Only `should` and `shouldNot` lead to a declaration assertion.
            let error = if st.constraint == Constraint::Should {
                !meets
            } else {
                meets
            };
            if !error {
                continue;
            }
            let not = if st.constraint == Constraint::Should {
                ""
            } else {
                " not"
            };
            let param = pattern.map_or("", |p| p.text.as_str());
            let message = match kind {
                Kind::Exist => format!("{name} should not exist"),
                Kind::BeAbstract => format!("{name} should{not} be abstract"),
                Kind::BeFinal => format!("{name} should{not} be final"),
                Kind::BeReadonly => format!("{name} should{not} be readonly"),
                Kind::BeEnum => format!("{name} should{not} be enum"),
                Kind::BeInvokable => format!("{name} should{not} be invokable"),
                Kind::BeInterface => format!("{name} should be an interface"),
                Kind::BeNamed if pattern.is_some_and(|p| p.regex.is_some()) => {
                    format!("{name} should be named matching the regex {param}")
                }
                Kind::BeNamed => format!("{name} should be named {param}"),
                Kind::HaveOnlyOnePublicMethod => {
                    format!("{name} should have only one public method")
                }
                _ => format!("{name} should have only one public method named {param}"),
            };
            self.report(line, st, message);
        }
    }

    /// `TypeNodeParser::parse()` on a class expression: a name, with `self`, `static` and `parent` resolved in the
    /// subject's scope.
    fn class_names(&self, e: &Expression<'arena>) -> Vec<String> {
        match e {
            Expression::Identifier(id) => vec![text(self.c.names.get(&id.span()))],
            Expression::Self_(_) | Expression::Static(_) => vec![Reflect::name(self.subject)],
            Expression::Parent(_) => vec![self.parent_name()],
            _ => Vec::new(),
        }
    }

    fn parent_name(&self) -> String {
        self.r
            .parent(self.subject)
            .map_or_else(|| "parent".to_string(), Reflect::name)
    }

    /// `TypeNodeParser::parse()` on a type: the names in it, through nullable, union and intersection types.
    fn hint_names(&self, h: &Hint<'arena>, out: &mut Vec<String>) {
        match h {
            Hint::Identifier(id) => out.push(text(self.c.names.get(&id.span()))),
            Hint::Self_(_) => out.push(Reflect::name(self.subject)),
            Hint::Parent(_) => out.push(self.parent_name()),
            Hint::Parenthesized(p) => self.hint_names(p.hint, out),
            Hint::Nullable(n) => self.hint_names(n.hint, out),
            Hint::Union(u) => {
                self.hint_names(u.left, out);
                self.hint_names(u.right, out);
            }
            Hint::Intersection(i) => {
                self.hint_names(i.left, out);
                self.hint_names(i.right, out);
            }
            _ => {}
        }
    }

    fn hint(&self, h: Option<&Hint<'arena>>) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(h) = h {
            self.hint_names(h, &mut out);
        }
        out
    }

    /// The doc comment before `at`, unless `ignore_doc_comments` is on.
    fn doc(&self, at: Span) -> Option<&'c super::facts::DocEntry> {
        // Only the depend rules read PHPDoc: without one, resolving its types is wasted work
        if self.rules.ignore_doc_comments || self.rules.of(Kind::Depend).next().is_none() {
            return None;
        }
        self.c
            .doc_before(at.file_id, at.start.offset)
            .map(|(_, e)| e)
    }

    /// The classes a resolved PHPDoc type references (`getReferencedClasses()`).
    fn referenced(ty: &Type, out: &mut Vec<String>) {
        out.extend(super::generics::referenced_classes(ty));
    }

    /// The class-scope tag extractors (`MethodTagExtractor`, `MixinTagExtractor`, `PropertyTagExtractor`).
    fn class_tags(&mut self, at: Span, line: i64) {
        let mut methods = Vec::new();
        let mut mixins = Vec::new();
        let mut properties = Vec::new();
        if let Some(e) = self.doc(at) {
            let scope = super::declared::class_scope(self.c, at, self.subject);
            let resolve = |n: &pun_phpstan::phpdoc::Node, out: &mut Vec<String>| {
                Self::referenced(&resolve_in(n, &scope), out);
            };
            for m in &e.doc.methods {
                for (_, ty, _) in &m.params {
                    if let Some(ty) = ty {
                        resolve(ty, &mut methods);
                    }
                }
                if let Some(ret) = &m.ret {
                    resolve(ret, &mut methods);
                }
            }
            for m in &e.doc.mixins {
                resolve(m, &mut mixins);
            }
            for (_, read, write) in &e.doc.properties {
                for ty in [read, write].into_iter().flatten() {
                    resolve(ty, &mut properties);
                }
            }
            let mut unique = Vec::new();
            for p in properties {
                if !unique.contains(&p) {
                    unique.push(p);
                }
            }
            properties = unique;
        }
        self.relation(Kind::Depend, methods, line);
        self.relation(Kind::Depend, mixins, line);
        self.relation(Kind::Depend, properties, line);
    }

    /// The `InClassNode` rules of a class, interface or enum.
    fn class_like(&mut self, at: Span) {
        let line = self.line(at);
        for kind in [
            Kind::BeNamed,
            Kind::BeReadonly,
            Kind::BeAbstract,
            Kind::BeFinal,
            Kind::BeEnum,
            Kind::BeInvokable,
            Kind::HaveOnlyOnePublicMethod,
            Kind::HaveOnlyOnePublicMethodNamed,
            Kind::BeInterface,
            Kind::Exist,
        ] {
            self.declaration(kind, line);
        }
        let k = self.subject;
        let names = |list: Vec<&ClassLikeMetadata>| list.into_iter().map(Reflect::name).collect();
        let interfaces: Vec<String> = names(self.r.interfaces(k));
        let immediate: Vec<String> = names(self.r.immediate_interfaces(k));
        let traits: Vec<String> = names(self.r.traits(k));
        self.relation(Kind::Implement, interfaces.clone(), line);
        let parents = if k.kind.is_interface() {
            interfaces
        } else {
            names(self.r.parents(k))
        };
        self.relation(Kind::Extend, parents, line);
        self.relation(Kind::Include, traits.clone(), line);
        self.relation(Kind::Depend, immediate.clone(), line);
        let parent = if k.kind.is_interface() {
            immediate
        } else {
            self.r.parent(k).map(Reflect::name).into_iter().collect()
        };
        self.relation(Kind::Depend, parent, line);
        self.relation(Kind::Depend, traits, line);
        self.relation(Kind::ApplyAttribute, self.r.attributes(k), line);
        self.class_tags(at, line);
    }
}

impl<'arena> MutWalker<'arena, 'arena, ()> for Walk<'_, 'arena> {
    fn walk_in_attribute_list(&mut self, l: &'arena AttributeList<'arena>, (): &mut ()) {
        let names = (l.attributes.iter())
            .map(|a| text(self.c.names.get(&a.name.span())))
            .collect();
        self.relation(Kind::Depend, names, self.line(l.span()));
    }

    fn walk_in_method(&mut self, m: &'arena Method<'arena>, (): &mut ()) {
        let line = self.line(m.span());
        let ret = self.hint(m.return_type_hint.as_ref().map(|r| &r.hint));
        self.relation(Kind::Depend, ret, line);
        let entry = self.doc(m.span());
        let (mut params, mut returns, mut throws) = (Vec::new(), Vec::new(), Vec::new());
        if let Some(e) = entry {
            let scope: NameScope =
                super::declared::method_scope(self.c, m, &e.doc, self.declaring, self.subject);
            for (ty, _) in e.doc.param_types(&scope).values() {
                Self::referenced(ty, &mut params);
            }
            if let Some(ty) = e.doc.return_type(&scope) {
                Self::referenced(&ty, &mut returns);
            }
            for tag in &e.phpdoc {
                if let ("@throws" | "@phpstan-throws", TagValue::Type(n)) =
                    (tag.name.as_str(), &tag.value)
                {
                    Self::referenced(&resolve_in(n, &scope), &mut throws);
                }
            }
        }
        self.relation(Kind::Depend, params, line);
        self.relation(Kind::Depend, returns, line);
        self.relation(Kind::Depend, throws, line);
    }

    fn walk_in_function_like_parameter(
        &mut self,
        p: &'arena FunctionLikeParameter<'arena>,
        (): &mut (),
    ) {
        let names = self.hint(p.hint.as_ref());
        self.relation(Kind::Depend, names, self.line(p.span()));
    }

    fn walk_in_plain_property(&mut self, p: &'arena PlainProperty<'arena>, (): &mut ()) {
        let names = self.hint(p.hint.as_ref());
        self.relation(Kind::Depend, names, self.line(p.span()));
    }

    fn walk_in_hooked_property(&mut self, p: &'arena HookedProperty<'arena>, (): &mut ()) {
        let names = self.hint(p.hint.as_ref());
        self.relation(Kind::Depend, names, self.line(p.span()));
    }

    fn walk_in_instantiation(&mut self, n: &'arena Instantiation<'arena>, (): &mut ()) {
        let names = self.class_names(n.class);
        let line = self.line(n.span());
        self.relation(Kind::Depend, names.clone(), line);
        self.relation(Kind::Construct, names, line);
    }

    fn walk_in_static_method_call(&mut self, s: &'arena StaticMethodCall<'arena>, (): &mut ()) {
        let names = self.class_names(s.class);
        self.relation(Kind::Depend, names, self.line(s.span()));
    }

    fn walk_in_class_constant_access(
        &mut self,
        a: &'arena ClassConstantAccess<'arena>,
        (): &mut (),
    ) {
        let names = self.class_names(a.class);
        self.relation(Kind::Depend, names, self.line(a.span()));
    }

    fn walk_in_binary(&mut self, b: &'arena Binary<'arena>, (): &mut ()) {
        if matches!(b.operator, BinaryOperator::Instanceof(_)) {
            let names = self.class_names(b.rhs);
            self.relation(Kind::Depend, names, self.line(b.span()));
        }
    }

    fn walk_in_try_catch_clause(&mut self, t: &'arena TryCatchClause<'arena>, (): &mut ()) {
        let names = self.hint(Some(&t.hint));
        self.relation(Kind::Depend, names, self.line(t.span()));
    }

    fn walk_in_statement_expression(
        &mut self,
        s: &'arena ExpressionStatement<'arena>,
        (): &mut (),
    ) {
        let mut names = Vec::new();
        if let Some(e) = self.doc(s.span()) {
            let scope = super::declared::scope_at(self.c, s.span(), Some(self.subject));
            for (_, ty) in e.doc.var_types(&scope) {
                Self::referenced(&ty, &mut names);
            }
        }
        self.relation(Kind::Depend, names, self.line(s.span()));
    }

    // An anonymous class has a scope of its own. PHPStan checks a function declared in a method like the method's
    // code, and a first-class callable `X::y(...)` is no static call to it.
    fn walk_anonymous_class(&mut self, _: &'arena AnonymousClass<'arena>, (): &mut ()) {}
}

/// The top-level class-likes of a file, in namespaces too: PHPStan analyses no other class declaration.
fn top_level<'arena>(program: &'arena Program<'arena>) -> Vec<&'arena Statement<'arena>> {
    let mut out = Vec::new();
    for s in &program.statements {
        match s {
            Statement::Namespace(n) => out.extend(n.statements().iter()),
            s => out.push(s),
        }
    }
    out
}

/// The errors phpat reports in a file, or in a trait in the context of the class `c` walks it for.
pub(super) fn check<'c, 'arena>(
    c: &'c Checker<'c, 'arena>,
    program: &'arena Program<'arena>,
    rules: &'c Rules,
) -> Vec<Error> {
    let r = || Reflect {
        codebase: c.codebase,
        files: c.files,
    };
    let mut errors = Vec::new();
    let in_trait = c.trait_context.as_ref();
    for s in top_level(program) {
        let (name, members, attributes): (_, &'arena Sequence<'arena, ClassLikeMember<'arena>>, _) =
            match s {
                Statement::Class(k) => (&k.name, &k.members, Some(&k.attribute_lists)),
                Statement::Interface(k) => (&k.name, &k.members, Some(&k.attribute_lists)),
                Statement::Enum(k) => (&k.name, &k.members, Some(&k.attribute_lists)),
                Statement::Trait(k) => (&k.name, &k.members, None),
                _ => continue,
            };
        let resolved = text(c.names.get(&name.span));
        let declared = c.codebase.get_class_like(resolved.as_bytes());
        let subject = match (in_trait, attributes) {
            (None, Some(_)) => declared,
            (Some(t), None) if t.trait_name.eq_ignore_ascii_case(&resolved) => {
                c.codebase.get_class_like(t.class.as_bytes())
            }
            _ => None,
        };
        let (Some(declared), Some(subject)) = (declared, subject) else {
            continue;
        };
        let mut walk = Walk {
            r: r(),
            c,
            rules,
            subject,
            declaring: declared,
            errors: Vec::new(),
        };
        if let Some(attributes) = attributes {
            for l in attributes {
                walk.walk_attribute_list(l, &mut ());
            }
            walk.class_like(s.span());
        }
        for m in members {
            walk.walk_class_like_member(m, &mut ());
        }
        errors.extend(walk.errors);
    }
    errors
}
