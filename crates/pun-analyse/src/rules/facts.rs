use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use mago_names::kind::NameKind;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;
use pun_phpstan::phpdoc;

use super::doc::{Doc, FileScopes};

pub(crate) struct Facts {
    pub scopes: FileScopes,
    pub comments: Comments,
    pub declarations: Declarations,
    pub strict_types: bool,
    /// The doc comments, and the functions and methods declared at the top level, in namespaces and in class-likes,
    /// merged where only whitespace separates them: the parts of a file that hold no constant or attribute
    /// initializer unless one is inside them.
    pub blankable: Vec<(u32, u32)>,
    /// The hash of the file without the bodies of its functions and methods (`crate::incremental`): what another
    /// file's analysis reads through these facts and the doc comments.
    pub outline: [u8; 16],
    /// The constants an analysed file names, as written (`crate::codebase::ConstantRef`): set by the codebase scan,
    /// which resolves them against the populated codebase later without parsing the file again.
    pub constant_refs: Vec<crate::codebase::ConstantRef>,
}

impl Facts {
    pub(crate) fn of(contents: &[u8], program: &Program, analysed: bool) -> Facts {
        let mut scopes = FileScopes::of(program);
        let mut declarations = Declarations::default();
        let mut traits = Vec::new();
        let mut trait_uses = Vec::new();
        DeclarationWalker {
            out: &mut declarations,
            scopes: &scopes,
            traits: &mut traits,
            trait_uses: &mut trait_uses,
        }
        .walk_program(program, &mut ());
        scopes.trait_uses = trait_uses;
        let strict_types = strict_types(contents);
        let mut comments = Comments::of(contents, analysed);
        if !analysed {
            comments.keep_trait_tags(contents, &traits);
        }
        Facts {
            scopes,
            comments,
            declarations,
            strict_types,
            blankable: blankable(contents, program),
            outline: outline(contents, program),
            constant_refs: Vec::new(),
        }
    }
}

/// Whether a walked body declares a class-like, a function or a constant (`const` or `define()`), which the scanner
/// registers wherever it is.
struct Declares(bool);

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Declares {
    fn walk_in_class(&mut self, _: &'ast Class<'arena>, (): &mut ()) {
        self.0 = true;
    }
    fn walk_in_interface(&mut self, _: &'ast Interface<'arena>, (): &mut ()) {
        self.0 = true;
    }
    fn walk_in_trait(&mut self, _: &'ast Trait<'arena>, (): &mut ()) {
        self.0 = true;
    }
    fn walk_in_enum(&mut self, _: &'ast Enum<'arena>, (): &mut ()) {
        self.0 = true;
    }
    fn walk_in_function(&mut self, _: &'ast Function<'arena>, (): &mut ()) {
        self.0 = true;
    }
    fn walk_in_constant(&mut self, _: &'ast Constant<'arena>, (): &mut ()) {
        self.0 = true;
    }
    fn walk_in_function_call(&mut self, call: &'ast FunctionCall<'arena>, (): &mut ()) {
        if let Expression::Identifier(name) = call.function
            && name
                .value()
                .strip_prefix(b"\\")
                .unwrap_or(name.value())
                .eq_ignore_ascii_case(b"define")
        {
            self.0 = true;
        }
    }
}

/// The hash of `contents` with every top-level function and method body (the braces and what they hold) replaced by
/// `{}`: a change inside a body leaves it as it was. A body that declares something the scanner registers ([`Declares`])
/// keeps its text, as other files read that declaration, its value and its doc comment through the outline.
fn outline(contents: &[u8], program: &Program) -> [u8; 16] {
    fn body(block: &Block, out: &mut Vec<(u32, u32)>) {
        let mut nested = Declares(false);
        nested.walk_block(block, &mut ());
        if !nested.0 {
            let span = block.span();
            out.push((span.start.offset, span.end.offset));
        }
    }
    fn members<'a>(
        members: impl Iterator<Item = &'a ClassLikeMember<'a>>,
        out: &mut Vec<(u32, u32)>,
    ) {
        for member in members {
            if let ClassLikeMember::Method(method) = member
                && let MethodBody::Concrete(block) = &method.body
            {
                body(block, out);
            }
        }
    }
    fn statements<'a>(list: impl Iterator<Item = &'a Statement<'a>>, out: &mut Vec<(u32, u32)>) {
        for statement in list {
            match statement {
                Statement::Namespace(n) => statements(n.statements().iter(), out),
                Statement::Function(f) => body(&f.body, out),
                Statement::Class(k) => members(k.members.iter(), out),
                Statement::Interface(k) => members(k.members.iter(), out),
                Statement::Trait(k) => members(k.members.iter(), out),
                Statement::Enum(k) => members(k.members.iter(), out),
                _ => {}
            }
        }
    }
    let mut bodies = Vec::new();
    statements(program.statements.iter(), &mut bodies);
    bodies.sort_unstable();
    let mut h = blake3::Hasher::new();
    let mut at = 0;
    for (start, end) in bodies {
        // the bodies of top-level declarations never overlap; `get` keeps a broken span from panicking
        h.update(contents.get(at..start as usize).unwrap_or_default());
        h.update(b"{}");
        at = at.max(end as usize);
    }
    h.update(&contents[at.min(contents.len())..]);
    let mut out = [0; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    out
}

fn blankable(contents: &[u8], program: &Program) -> Vec<(u32, u32)> {
    fn whole(span: Span) -> (u32, u32) {
        (span.start.offset, span.end.offset)
    }
    fn members<'a>(
        members: impl Iterator<Item = &'a ClassLikeMember<'a>>,
        out: &mut Vec<(u32, u32)>,
    ) {
        for member in members {
            if let ClassLikeMember::Method(method) = member {
                out.push(whole(method.span()));
            }
        }
    }
    fn statements<'a>(list: impl Iterator<Item = &'a Statement<'a>>, out: &mut Vec<(u32, u32)>) {
        for statement in list {
            match statement {
                Statement::Namespace(n) => statements(n.statements().iter(), out),
                Statement::Function(f) => out.push(whole(f.span())),
                Statement::Class(k) => members(k.members.iter(), out),
                Statement::Interface(k) => members(k.members.iter(), out),
                Statement::Trait(k) => members(k.members.iter(), out),
                Statement::Enum(k) => members(k.members.iter(), out),
                _ => {}
            }
        }
    }
    let mut found: Vec<_> = (program.trivia.iter())
        .filter(|t| t.kind.is_docblock())
        .map(|t| whole(t.span))
        .collect();
    statements(program.statements.iter(), &mut found);
    found.sort_unstable();
    // A doc comment and the method after it, and the methods of a class, are one range: whitespace apart. The doc
    // comments inside a function are in its range.
    let mut out: Vec<(u32, u32)> = Vec::new();
    for (start, end) in found {
        match out.last_mut() {
            Some(last)
                if start <= last.1
                    || contents[last.1 as usize..start as usize]
                        .iter()
                        .all(u8::is_ascii_whitespace) =>
            {
                last.1 = last.1.max(end);
            }
            _ => out.push((start, end)),
        }
    }
    out.shrink_to_fit();
    out
}

fn strict_types(contents: &[u8]) -> bool {
    const PATTERN: &[u8] = b"declare(strict_types=1)";
    let mut matched = 0;
    for b in contents
        .iter()
        .filter(|b| !b.is_ascii_whitespace())
        .map(u8::to_ascii_lowercase)
    {
        matched = if b == PATTERN[matched] {
            matched + 1
        } else {
            usize::from(b == PATTERN[0])
        };
        if matched == PATTERN.len() {
            return true;
        }
    }
    false
}

#[derive(Clone)]
pub(crate) struct DocEntry {
    pub doc: Doc,
    pub tags: Vec<(String, Vec<u8>)>,
    pub names: Vec<String>,
    pub phpdoc: Vec<phpdoc::Tag>,
}

const SCANNED_TAGS: &[&str] = &[
    "@phpstan-assert",
    "@phpstan-assert-if-true",
    "@phpstan-assert-if-false",
    "@psalm-assert",
    "@psalm-assert-if-true",
    "@psalm-assert-if-false",
    "@phan-assert",
    "@phan-assert-if-true",
    "@phan-assert-if-false",
    "@throws",
    "@phpstan-throws",
    "@method",
    "@phan-method",
    "@psalm-method",
    "@phpstan-method",
    "@property",
    "@phpstan-property",
    "@property-read",
    "@phpstan-property-read",
    "@property-write",
    "@phpstan-property-write",
    "@mixin",
    "@psalm-require-extends",
    "@phpstan-require-extends",
    "@psalm-require-implements",
    "@phpstan-require-implements",
    "@psalm-inheritors",
    "@phpstan-sealed",
];

const SCANNED_PREFIXES: &[&[u8]] = &[
    b"@phpstan-",
    b"@psalm-",
    b"@phan-",
    b"@throws",
    b"@method",
    b"@property",
    b"@mixin",
];

const TRAIT_TAGS: &[&str] = &[
    "@method",
    "@phan-method",
    "@psalm-method",
    "@phpstan-method",
    "@property",
    "@phpstan-property",
    "@property-read",
    "@phpstan-property-read",
    "@property-write",
    "@phpstan-property-write",
    "@mixin",
];

impl DocEntry {
    pub(crate) fn parse(raw: &[u8], analysed: bool) -> DocEntry {
        let mut tags = Vec::new();
        if analysed {
            tags = phpdoc::tags(raw).unwrap_or_default();
        } else if (0..raw.len())
            .any(|at| raw[at] == b'@' && SCANNED_PREFIXES.iter().any(|p| raw[at..].starts_with(p)))
        {
            tags = phpdoc::tags(raw).unwrap_or_default();
            tags.retain(|t| SCANNED_TAGS.contains(&t.name.as_str()));
            tags.shrink_to_fit();
        }
        DocEntry {
            doc: Doc::parse(raw),
            tags: super::doc::tags(raw),
            names: super::declarations::doc_tags(raw),
            phpdoc: tags,
        }
    }
}

const NONE: u32 = u32::MAX;

#[derive(Default)]
pub(crate) struct Comments {
    ends: Vec<u32>,
    doc: Vec<u32>,
    plain: Vec<u32>,
    docs: Vec<LazyDoc>,
    analysed: bool,
}

/// A doc comment, parsed the first time a rule asks for it: most docs of the scanned files never are.
struct LazyDoc {
    start: u32,
    end: u32,
    /// False for a scanned file's doc that is not a trait's own: it drops the trait tags.
    trait_tags: bool,
    entry: OnceLock<Box<DocEntry>>,
}

impl Comments {
    pub(crate) fn of(contents: &[u8], analysed: bool) -> Comments {
        let mut out = Comments {
            analysed,
            ..Comments::default()
        };
        let (mut doc_opens, mut opens, mut last_close) = ([None; 2], [None; 2], None);
        let last = contents.len().saturating_sub(1);
        for p in memchr::memchr2_iter(b'/', b'*', &contents[..last]) {
            match &contents[p..p + 2] {
                b"/*" => {
                    if contents.get(p + 2) == Some(&b'*') {
                        doc_opens = [doc_opens[1], Some(p)];
                    }
                    opens = [opens[1], Some(p)];
                }
                b"*/" => {
                    let end = p + 2;
                    let doc_start = doc_opens[1].filter(|&s| s + 3 <= p).or(doc_opens[0]);
                    let plain_start = opens[1].filter(|&s| s + 2 <= p).or(opens[0]);
                    let doc = doc_start.filter(|&s| {
                        contents.get(s + 3).is_some_and(u8::is_ascii_whitespace)
                            && last_close.is_none_or(|q| q < s + 3)
                    });
                    out.ends.push(end as u32);
                    out.plain.push(plain_start.map_or(NONE, |s| s as u32));
                    out.doc.push(match doc {
                        Some(start) => {
                            out.docs.push(LazyDoc {
                                start: start as u32,
                                end: end as u32,
                                trait_tags: true,
                                entry: OnceLock::new(),
                            });
                            (out.docs.len() - 1) as u32
                        }
                        None => NONE,
                    });
                    last_close = Some(p);
                }
                _ => {}
            }
        }
        out.docs.shrink_to_fit();
        out
    }

    fn keep_trait_tags(&mut self, contents: &[u8], traits: &[u32]) {
        let kept: Vec<usize> = (traits.iter())
            .filter_map(|&at| self.doc_index(contents, at as usize))
            .collect();
        for (i, doc) in self.docs.iter_mut().enumerate() {
            doc.trait_tags = kept.contains(&i);
        }
    }

    fn parsed<'c>(&'c self, contents: &'c [u8], d: usize) -> (&'c [u8], &'c DocEntry) {
        let doc = &self.docs[d];
        let raw = &contents[doc.start as usize..doc.end as usize];
        let entry = doc.entry.get_or_init(|| {
            let mut entry = DocEntry::parse(raw, self.analysed);
            if !doc.trait_tags {
                entry
                    .phpdoc
                    .retain(|t| !TRAIT_TAGS.contains(&t.name.as_str()));
                entry.phpdoc.shrink_to_fit();
            }
            Box::new(entry)
        });
        (raw, entry)
    }

    fn closing(&self, end: usize) -> Option<usize> {
        self.ends.binary_search(&(end as u32)).ok()
    }

    pub(crate) fn doc_before<'c>(
        &'c self,
        contents: &'c [u8],
        offset: usize,
    ) -> Option<(&'c [u8], &'c DocEntry)> {
        Some(self.parsed(contents, self.doc_index(contents, offset)?))
    }

    fn doc_index(&self, contents: &[u8], offset: usize) -> Option<usize> {
        let mut end = offset.min(contents.len());
        loop {
            end -= contents[..end]
                .iter()
                .rev()
                .take_while(|b| b.is_ascii_whitespace())
                .count();
            if !contents[..end].ends_with(b"*/") || end < 4 {
                return None;
            }
            let i = self.closing(end)?;
            match self.doc[i] {
                NONE => end = Some(self.plain[i]).filter(|&s| s != NONE)? as usize,
                d => return Some(d as usize),
            }
        }
    }

    pub(crate) fn entry<'c>(
        &'c self,
        contents: &'c [u8],
        start: u32,
        end: u32,
    ) -> Option<&'c DocEntry> {
        let i = self.docs.binary_search_by_key(&end, |doc| doc.end).ok()?;
        (self.docs[i].start == start).then(|| self.parsed(contents, i).1)
    }

    pub(crate) fn php_doc_comment<'c>(
        &self,
        contents: &'c [u8],
        offset: usize,
    ) -> Option<&'c [u8]> {
        let mut end = offset.min(contents.len());
        loop {
            end -= contents[..end]
                .iter()
                .rev()
                .take_while(|b| b.is_ascii_whitespace())
                .count();
            if !contents[..end].ends_with(b"*/") {
                return None;
            }
            let start = Some(self.plain[self.closing(end)?]).filter(|&s| s != NONE)? as usize;
            let comment = &contents[start..end];
            if comment.starts_with(b"/**") && comment.get(3).is_some_and(u8::is_ascii_whitespace) {
                return Some(comment);
            }
            end = start;
        }
    }
}

#[derive(Default)]
pub(crate) struct Declarations {
    pub starts: HashMap<u32, u32>,
    pub readonly: HashSet<u32>,
    pub parents: HashMap<u32, (bool, Vec<Vec<u8>>)>,
}

struct DeclarationWalker<'d> {
    out: &'d mut Declarations,
    scopes: &'d FileScopes,
    traits: &'d mut Vec<u32>,
    trait_uses: &'d mut Vec<(u32, Vec<String>)>,
}

impl DeclarationWalker<'_> {
    fn parents<'n>(
        &mut self,
        at: Span,
        interface: bool,
        names: impl Iterator<Item = &'n Identifier<'n>>,
    ) {
        let scope = self.scopes.at(at.start.offset);
        let resolved = names
            .map(|id| scope.resolve(NameKind::Default, id.value()).0)
            .collect();
        self.out
            .parents
            .insert(at.start.offset, (interface, resolved));
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for DeclarationWalker<'_> {
    fn walk_in_trait_use(&mut self, t: &'ast TraitUse<'arena>, (): &mut ()) {
        let names = t
            .trait_names
            .iter()
            .map(|n| String::from_utf8_lossy(n.value()).into_owned())
            .collect();
        self.trait_uses.push((t.span().start.offset, names));
    }

    fn walk_in_property(&mut self, p: &'ast Property<'arena>, (): &mut ()) {
        for v in p.variables() {
            self.out
                .starts
                .insert(v.span.start.offset, p.span().start.offset);
            if p.modifiers().iter().any(Modifier::is_readonly) {
                self.out.readonly.insert(v.span.start.offset);
            }
        }
    }
    fn walk_in_function_like_parameter(
        &mut self,
        p: &'ast FunctionLikeParameter<'arena>,
        (): &mut (),
    ) {
        if p.modifiers.iter().any(Modifier::is_readonly) {
            self.out.readonly.insert(p.variable.span.start.offset);
        }
    }
    fn walk_in_class(&mut self, c: &'ast Class<'arena>, (): &mut ()) {
        if c.modifiers.iter().any(Modifier::is_readonly) {
            self.out.readonly.insert(c.span().start.offset);
        }
        self.parents(
            c.span(),
            false,
            c.implements.iter().flat_map(|i| i.types.iter()),
        );
    }
    fn walk_in_anonymous_class(&mut self, c: &'ast AnonymousClass<'arena>, (): &mut ()) {
        if c.modifiers.iter().any(Modifier::is_readonly) {
            self.out.readonly.insert(c.span().start.offset);
        }
        self.parents(
            c.span(),
            false,
            c.implements.iter().flat_map(|i| i.types.iter()),
        );
    }
    fn walk_in_enum(&mut self, e: &'ast Enum<'arena>, (): &mut ()) {
        self.parents(
            e.span(),
            false,
            e.implements.iter().flat_map(|i| i.types.iter()),
        );
    }
    fn walk_in_trait(&mut self, t: &'ast Trait<'arena>, (): &mut ()) {
        self.traits.push(t.span().start.offset);
    }
    fn walk_in_interface(&mut self, i: &'ast Interface<'arena>, (): &mut ()) {
        self.parents(
            i.span(),
            true,
            i.extends.iter().flat_map(|e| e.types.iter()),
        );
    }
    fn walk_in_class_like_constant(&mut self, k: &'ast ClassLikeConstant<'arena>, (): &mut ()) {
        for item in &k.items {
            self.out
                .starts
                .insert(item.span().start.offset, k.span().start.offset);
        }
    }
    fn walk_in_constant(&mut self, k: &'ast Constant<'arena>, (): &mut ()) {
        for item in &k.items {
            self.out
                .starts
                .insert(item.span().start.offset, k.span().start.offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline_of(code: &str) -> [u8; 16] {
        let file = mago_database::file::File::ephemeral(
            b"a.php".as_slice().into(),
            code.as_bytes().to_vec().into(),
        );
        let arena = mago_allocator::LocalArena::new();
        let program = mago_syntax::parser::parse_file(&arena, &file);
        outline(code.as_bytes(), program)
    }

    /// A body that declares something the scanner registers keeps its text in the outline, whatever it declares; any
    /// other body is left out, so editing it leaves the outline as it was.
    #[test]
    fn a_body_that_declares_keeps_its_text() {
        let wrap = |inner: &str| format!("<?php\nfunction f() {{ if (true) {{ {inner} }} }}\n");
        assert_eq!(outline_of(&wrap("$a = 1;")), outline_of(&wrap("$a = 2;")));
        for declares in [
            "/** @a */ class K {}",
            "/** @a */ interface I {}",
            "/** @a */ trait T {}",
            "/** @a */ enum E {}",
            "/** @a */ function g() {}",
            "/** @a */ const C = 1;",
            "define('C', 1); /** @a */",
            "\\define('C', 1); /** @a */",
        ] {
            assert_ne!(
                outline_of(&wrap(declares)),
                outline_of(&wrap(&declares.replace("@a", "@b"))),
                "{declares}"
            );
        }
    }
}
