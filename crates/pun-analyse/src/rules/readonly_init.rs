use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mago_allocator::LocalArena;
use mago_codex::metadata::class_like::ClassLikeMetadata;
use mago_codex::symbol::SymbolKind;
use mago_codex::visibility::Visibility;
use mago_database::file::{File, FileId};
use mago_names::ResolvedNames;
use mago_span::HasSpan;
use mago_syntax::cst::*;
use pun_phpstan::ignore::Error;
use pun_phpstan::types::Tri;

use super::Checker;
use super::declarations::{READONLY_TAGS, doc_tags, has_tag};
use super::types::flow::is_this;
use crate::codebase::parse;
use crate::text;

type State = HashMap<String, Tri>;

fn get(s: &State, p: &str) -> Tri {
    s.get(p).copied().unwrap_or(Tri::No)
}

fn merge(a: &State, b: &State) -> State {
    let mut out = State::new();
    for (p, t) in a {
        out.insert(p.clone(), b.get(p).map_or(Tri::Maybe, |u| t.and(*u)));
    }
    for p in b.keys().filter(|p| !a.contains_key(*p)) {
        out.insert(p.clone(), Tri::Maybe);
    }
    out
}

fn merge_initialized(scope: &mut State, called: &State) {
    for (p, t) in called {
        let certainty = scope.get(p).map_or(*t, |u| t.or(*u));
        scope.insert(p.clone(), certainty);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Flow {
    Next,
    Skip,
    Exit,
}

struct Usage {
    property: String,
    source: usize,
    line: i64,
    write: Option<bool>,
    state: Tri,
    parent: Option<Tri>,
}

struct Call {
    method: String,
    state: State,
}

#[derive(Default)]
struct Gathered {
    usages: Vec<Usage>,
    calls: Vec<Call>,
}

struct Source<'x> {
    file: &'x File,
    names: &'x ResolvedNames<'x>,
    label: Option<String>,
}

struct MethodAst<'x> {
    name: String,
    method: &'x Method<'x>,
    source: usize,
    order: usize,
    constructor: bool,
}

struct PropertyNode {
    name: String,
    line: i64,
    readonly: bool,
    readonly_by_phpdoc: bool,
    initial: Tri,
    offsets: (bool, bool),
}

struct Ctx<'c, 'x> {
    c: &'c Checker<'c, 'x>,
    k: &'c ClassLikeMetadata,
    sources: Vec<Source<'x>>,
    offsets: HashMap<String, (bool, bool)>,
    methods: HashMap<String, MethodAst<'x>>,
}

struct Walk<'w, 'c, 'x> {
    ctx: &'w Ctx<'c, 'x>,
    source: usize,
    this: bool,
    constructor: bool,
    parent: Option<State>,
    gathered: Gathered,
    returns: Vec<State>,
}

fn this_property<'e>(e: &'e Expression) -> Option<&'e [u8]> {
    match e {
        Expression::Access(Access::Property(p)) if is_this(p.object) => match &p.property {
            ClassLikeMemberSelector::Identifier(id) => Some(id.value),
            _ => None,
        },
        Expression::Parenthesized(p) => this_property(p.expression),
        _ => None,
    }
}

impl<'x> Walk<'_, '_, 'x> {
    fn line(&self, at: impl HasSpan) -> i64 {
        i64::from(
            self.ctx.sources[self.source]
                .file
                .line_number(at.span().start.offset),
        ) + 1
    }

    fn usage(&mut self, s: &State, property: &[u8], at: impl HasSpan, write: Option<bool>) {
        if !self.this {
            return;
        }
        let property = text(property);
        let parent = self.parent.as_ref().map(|p| get(p, &property));
        let line = self.line(at);
        let source = self.source;
        self.gathered.usages.push(Usage {
            state: get(s, &property),
            property,
            source,
            line,
            write,
            parent,
        });
    }

    fn call(&mut self, s: &State, method: &[u8]) {
        if self.this {
            self.gathered.calls.push(Call {
                method: text(method),
                state: s.clone(),
            });
        }
    }

    fn statements<'a>(
        &mut self,
        list: impl IntoIterator<Item = &'a Statement<'a>>,
        s: &mut State,
    ) -> Flow
    where
        'x: 'a,
    {
        for statement in list {
            let flow = self.node(Node::Statement(statement), s);
            if flow != Flow::Next {
                return flow;
            }
        }
        Flow::Next
    }

    fn branches(
        &mut self,
        s: &mut State,
        branches: Vec<(Vec<Node<'_, '_>>, bool)>,
        fallthrough: bool,
    ) -> Flow {
        let mut ends: Vec<State> = Vec::new();
        if fallthrough {
            ends.push(s.clone());
        }
        for (nodes, _) in branches {
            let mut branch = s.clone();
            let mut flow = Flow::Next;
            for n in nodes {
                flow = self.node(n, &mut branch);
                if flow != Flow::Next {
                    break;
                }
            }
            if flow != Flow::Exit {
                ends.push(branch);
            }
        }
        let Some(first) = ends.first() else {
            return Flow::Exit;
        };
        *s = ends[1..].iter().fold(first.clone(), |a, b| merge(&a, b));
        Flow::Next
    }

    fn looped(&mut self, s: &mut State, nodes: Vec<Node<'_, '_>>, once: bool) -> Flow {
        let run = |w: &mut Self, mut from: State| {
            let mut flow = Flow::Next;
            for n in nodes.iter().copied() {
                flow = w.node(n, &mut from);
                if flow != Flow::Next {
                    break;
                }
            }
            (from, flow)
        };
        let mut entry = s.clone();
        loop {
            let kept = (
                self.gathered.usages.len(),
                self.gathered.calls.len(),
                self.returns.len(),
            );
            let (end, _) = run(self, entry.clone());
            self.gathered.usages.truncate(kept.0);
            self.gathered.calls.truncate(kept.1);
            self.returns.truncate(kept.2);
            let next = merge(&entry, &end);
            if next == entry {
                break;
            }
            entry = next;
        }
        let (end, flow) = run(self, entry);
        *s = if once { end } else { merge(s, &end) };
        flow
    }

    fn maybe(&mut self, s: &mut State, nodes: Vec<Node<'_, '_>>) {
        let mut body = s.clone();
        for n in nodes {
            if self.node(n, &mut body) != Flow::Next {
                break;
            }
        }
        *s = merge(s, &body);
    }

    fn node(&mut self, n: Node<'_, '_>, s: &mut State) -> Flow {
        match n {
            Node::If(i) => {
                self.node(Node::Expression(i.condition), s);
                let (branches, has_else) = match &i.body {
                    IfBody::Statement(b) => {
                        let mut out = vec![(vec![Node::Statement(b.statement)], true)];
                        for e in &b.else_if_clauses {
                            out.push((vec![Node::IfStatementBodyElseIfClause(e)], true));
                        }
                        if let Some(e) = &b.else_clause {
                            out.push((vec![Node::Statement(e.statement)], true));
                        }
                        (out, b.else_clause.is_some())
                    }
                    IfBody::ColonDelimited(b) => {
                        let mut out =
                            vec![(b.statements.iter().map(Node::Statement).collect(), true)];
                        for e in &b.else_if_clauses {
                            out.push((vec![Node::IfColonDelimitedBodyElseIfClause(e)], true));
                        }
                        if let Some(e) = &b.else_clause {
                            out.push((e.statements.iter().map(Node::Statement).collect(), true));
                        }
                        (out, b.else_clause.is_some())
                    }
                };
                self.branches(s, branches, !has_else)
            }
            Node::IfStatementBodyElseIfClause(e) => {
                self.node(Node::Expression(e.condition), s);
                self.node(Node::Statement(e.statement), s)
            }
            Node::IfColonDelimitedBodyElseIfClause(e) => {
                self.node(Node::Expression(e.condition), s);
                self.statements(e.statements.iter(), s)
            }
            Node::Switch(sw) => {
                if self.node(Node::Expression(sw.expression), s) == Flow::Exit {
                    return Flow::Exit;
                }
                let cases = match &sw.body {
                    SwitchBody::BraceDelimited(b) => b.cases.as_slice(),
                    SwitchBody::ColonDelimited(b) => b.cases.as_slice(),
                };
                let has_default = cases.iter().any(|c| matches!(c, SwitchCase::Default(_)));
                let branches = cases
                    .iter()
                    .map(|case| match case {
                        SwitchCase::Expression(e) => {
                            let mut nodes = vec![Node::Expression(e.expression)];
                            nodes.extend(e.statements.iter().map(Node::Statement));
                            (nodes, true)
                        }
                        SwitchCase::Default(d) => {
                            (d.statements.iter().map(Node::Statement).collect(), true)
                        }
                    })
                    .collect();
                self.branches(s, branches, !has_default)
            }
            Node::Match(m) => {
                if self.node(Node::Expression(m.expression), s) == Flow::Exit {
                    return Flow::Exit;
                }
                let branches = m
                    .arms
                    .iter()
                    .map(|arm| match arm {
                        MatchArm::Expression(a) => {
                            let mut nodes: Vec<Node> =
                                a.conditions.iter().map(|c| Node::Expression(c)).collect();
                            nodes.push(Node::Expression(a.expression));
                            (nodes, true)
                        }
                        MatchArm::Default(d) => (vec![Node::Expression(d.expression)], true),
                    })
                    .collect();
                self.branches(s, branches, false)
            }
            Node::While(w) => {
                self.node(Node::Expression(w.condition), s);
                let body = match &w.body {
                    WhileBody::Statement(b) => vec![Node::Statement(b)],
                    WhileBody::ColonDelimited(b) => {
                        b.statements.iter().map(Node::Statement).collect()
                    }
                };
                self.looped(s, body, false);
                Flow::Next
            }
            Node::For(f) => {
                for e in &f.initializations {
                    if self.node(Node::Expression(e), s) == Flow::Exit {
                        return Flow::Exit;
                    }
                }
                for e in &f.conditions {
                    self.node(Node::Expression(e), s);
                }
                let mut body: Vec<Node> = match &f.body {
                    ForBody::Statement(b) => vec![Node::Statement(b)],
                    ForBody::ColonDelimited(b) => {
                        b.statements.iter().map(Node::Statement).collect()
                    }
                };
                body.extend(f.increments.iter().map(|e| Node::Expression(e)));
                self.looped(s, body, false);
                Flow::Next
            }
            Node::Foreach(f) => {
                self.node(Node::Expression(f.expression), s);
                let mut body = vec![Node::ForeachTarget(&f.target)];
                match &f.body {
                    ForeachBody::Statement(b) => body.push(Node::Statement(b)),
                    ForeachBody::ColonDelimited(b) => {
                        body.extend(b.statements.iter().map(Node::Statement));
                    }
                }
                self.looped(s, body, false);
                Flow::Next
            }
            Node::DoWhile(d) => {
                if self.looped(s, vec![Node::Statement(d.statement)], true) == Flow::Exit {
                    return Flow::Exit;
                }
                self.node(Node::Expression(d.condition), s);
                Flow::Next
            }
            Node::Try(t) => {
                let before = s.clone();
                let mut ends = Vec::new();
                let mut after_try = s.clone();
                if self.statements(t.block.statements.iter(), &mut after_try) != Flow::Exit {
                    ends.push(after_try.clone());
                }
                let catch_from = merge(&before, &after_try);
                for clause in &t.catch_clauses {
                    let mut caught = catch_from.clone();
                    if self.statements(clause.block.statements.iter(), &mut caught) != Flow::Exit {
                        ends.push(caught);
                    }
                }
                let exits = ends.is_empty();
                let mut end = ends.first().cloned().unwrap_or(catch_from);
                for e in ends.iter().skip(1) {
                    end = merge(&end, e);
                }
                if let Some(finally) = &t.finally_clause
                    && self.statements(finally.block.statements.iter(), &mut end) == Flow::Exit
                {
                    return Flow::Exit;
                }
                *s = end;
                if exits { Flow::Exit } else { Flow::Next }
            }
            Node::Return(r) => {
                if let Some(v) = r.value {
                    self.node(Node::Expression(v), s);
                }
                self.returns.push(s.clone());
                Flow::Exit
            }
            Node::Break(_) | Node::Continue(_) => Flow::Skip,
            Node::Throw(t) => {
                self.node(Node::Expression(t.exception), s);
                Flow::Exit
            }
            Node::ExitConstruct(_) | Node::DieConstruct(_) => {
                for child in n.children() {
                    self.node(child, s);
                }
                Flow::Exit
            }
            Node::Function(_)
            | Node::Class(_)
            | Node::Interface(_)
            | Node::Trait(_)
            | Node::Enum(_) => Flow::Next,
            Node::AnonymousClass(a) => {
                if let Some(args) = &a.argument_list {
                    self.node(Node::PartialArgumentList(args), s);
                }
                Flow::Next
            }
            Node::Closure(cl) => {
                if cl.r#static.is_none() {
                    self.nested(s, State::new(), |w, inner| {
                        w.statements(cl.body.statements.iter(), inner);
                    });
                }
                Flow::Next
            }
            Node::ArrowFunction(a) => {
                if a.r#static.is_none() {
                    self.nested(s, s.clone(), |w, inner| {
                        w.node(Node::Expression(a.expression), inner);
                    });
                }
                Flow::Next
            }
            Node::Binary(b)
                if matches!(
                    b.operator,
                    BinaryOperator::And(_)
                        | BinaryOperator::LowAnd(_)
                        | BinaryOperator::Or(_)
                        | BinaryOperator::LowOr(_)
                        | BinaryOperator::NullCoalesce(_)
                ) =>
            {
                if self.node(Node::Expression(b.lhs), s) == Flow::Exit {
                    return Flow::Exit;
                }
                self.maybe(s, vec![Node::Expression(b.rhs)]);
                Flow::Next
            }
            Node::Conditional(c) => {
                if self.node(Node::Expression(c.condition), s) == Flow::Exit {
                    return Flow::Exit;
                }
                let then = c.then.map_or_else(Vec::new, |t| vec![Node::Expression(t)]);
                self.branches(
                    s,
                    vec![(then, true), (vec![Node::Expression(c.r#else)], true)],
                    false,
                )
            }
            Node::Assignment(a) => self.assignment(a, s),
            Node::PropertyAccess(p) => {
                if is_this(p.object)
                    && let ClassLikeMemberSelector::Identifier(id) = &p.property
                {
                    self.usage(s, id.value, p, None);
                    return Flow::Next;
                }
                self.children(n, s)
            }
            Node::MethodCall(m) => {
                let on_this = is_this(m.object);
                let method = match &m.method {
                    ClassLikeMemberSelector::Identifier(id) if on_this => Some(id.value),
                    _ => None,
                };
                if let Some(method) = method {
                    self.call(s, method);
                }
                let flow = self.children(n, s);
                if let Some(method) = method
                    && self.constructor
                    && let Some(end) = self.ctx.called_end(method)
                {
                    merge_initialized(s, &end);
                }
                if flow == Flow::Next && method.is_some_and(|m| self.ctx.method_never(m)) {
                    return Flow::Exit;
                }
                flow
            }
            Node::MethodPartialApplication(m) => {
                if is_this(m.object)
                    && let ClassLikeMemberSelector::Identifier(id) = &m.method
                {
                    self.call(s, id.value);
                }
                self.children(n, s)
            }
            Node::StaticMethodCall(sc) => {
                let method = match &sc.method {
                    ClassLikeMemberSelector::Identifier(id) => Some(id.value),
                    _ => None,
                };
                let class = self.ctx.this_class(self.source, sc.class);
                if let (Some(method), Some(class)) = (method, &class) {
                    if class.eq_ignore_ascii_case(&self.ctx.name()) {
                        self.call(s, method);
                    }
                    if method.eq_ignore_ascii_case(b"__construct") {
                        for p in self.ctx.promoted_of(class) {
                            self.usage(s, p.as_bytes(), sc, Some(false));
                        }
                    }
                }
                let flow = self.children(n, s);
                if let (Some(method), Some(class)) = (method, class)
                    && method.eq_ignore_ascii_case(b"__construct")
                    && self.this
                    && !class.eq_ignore_ascii_case(&self.ctx.name())
                {
                    for p in self.ctx.promoted_of(&class) {
                        s.insert(p, Tri::Yes);
                    }
                }
                flow
            }
            Node::StaticMethodPartialApplication(sc) => {
                if let ClassLikeMemberSelector::Identifier(id) = &sc.method
                    && self
                        .ctx
                        .this_class(self.source, sc.class)
                        .is_some_and(|c| c.eq_ignore_ascii_case(&self.ctx.name()))
                {
                    self.call(s, id.value);
                }
                self.children(n, s)
            }
            Node::FunctionCall(f) => {
                let name = match f.function {
                    Expression::Identifier(id) => {
                        Some(self.ctx.sources[self.source].names.get(&id.span()))
                    }
                    _ => None,
                };
                let enumerating = match f.function {
                    Expression::Identifier(id) => {
                        let names = self.ctx.sources[self.source].names;
                        let n =
                            super::function_checks::function_name(names, self.ctx.c.codebase, id);
                        n == "get_object_vars" || n == "array_walk"
                    }
                    _ => false,
                };
                if enumerating
                    && let Some(Argument::Positional(first)) = f.argument_list.arguments.first()
                    && is_this(first.value)
                {
                    let at = self.gathered.usages.len();
                    for p in self.ctx.instance_properties() {
                        self.usage(s, p.as_bytes(), f, None);
                    }
                    let line = i64::from(
                        self.ctx.sources[0]
                            .file
                            .line_number(self.ctx.k.span.start.offset),
                    ) + 1;
                    for u in &mut self.gathered.usages[at..] {
                        (u.line, u.source) = (line, 0);
                    }
                }
                let (by_ref, virtual_assigned) = if self.source == 0 {
                    (
                        super::assign::by_ref_arguments(self.ctx.c, f),
                        super::assign::virtual_assigned(self.ctx.c, f),
                    )
                } else {
                    (Default::default(), None)
                };
                let flow = self.children(n, s);
                if flow == Flow::Next && name.is_some_and(|n| self.ctx.never(n)) {
                    return Flow::Exit;
                }
                let by_ref = f
                    .argument_list
                    .arguments
                    .iter()
                    .zip(by_ref)
                    .filter(|(_, r)| *r)
                    .map(|(a, _)| a.value());
                for value in by_ref.chain(virtual_assigned) {
                    if let Some(property) = this_property(value) {
                        self.usage(s, property, value, Some(false));
                    }
                }
                flow
            }
            _ => self.children(n, s),
        }
    }

    fn children(&mut self, n: Node<'_, '_>, s: &mut State) -> Flow {
        for child in n.children() {
            let flow = self.node(child, s);
            if flow != Flow::Next {
                return flow;
            }
        }
        Flow::Next
    }

    fn nested(&mut self, s: &State, mut inner: State, f: impl FnOnce(&mut Self, &mut State)) {
        let parent = self.parent.replace(s.clone());
        let returns = std::mem::take(&mut self.returns);
        let constructor = std::mem::replace(&mut self.constructor, false);
        f(self, &mut inner);
        self.parent = parent;
        self.returns = returns;
        self.constructor = constructor;
    }

    fn assignment(&mut self, a: &Assignment, s: &mut State) -> Flow {
        let target = a.lhs;
        if let Some(property) = this_property(target) {
            let assign = matches!(a.operator, AssignmentOperator::Assign(_));
            let coalesce = matches!(a.operator, AssignmentOperator::Coalesce(_));
            if !assign {
                self.usage(s, property, target, None);
            }
            if self.node(Node::Expression(a.rhs), s) == Flow::Exit {
                return Flow::Exit;
            }
            self.usage(s, property, target, Some(false));
            if assign || coalesce {
                s.insert(text(property), Tri::Yes);
            }
            return Flow::Next;
        }
        let mut root = target;
        let mut dims = Vec::new();
        loop {
            match root {
                Expression::ArrayAccess(x) => {
                    dims.push(x.index);
                    root = x.array;
                }
                Expression::ArrayAppend(x) => root = x.array,
                _ => break,
            }
        }
        if (!dims.is_empty() || !std::ptr::eq(root, target))
            && let Some(property) = this_property(root)
        {
            for d in dims.iter().rev() {
                self.node(Node::Expression(d), s);
            }
            let flow = self.node(Node::Expression(a.rhs), s);
            let (object, array_access) = self
                .ctx
                .offsets
                .get(&text(property))
                .copied()
                .unwrap_or((true, false));
            if object {
                self.usage(s, property, root, None);
            }
            self.usage(s, property, root, Some(array_access));
            s.insert(text(property), Tri::Yes);
            return flow;
        }
        if let Expression::List(_) | Expression::Array(_) | Expression::LegacyArray(_) = target {
            if self.node(Node::Expression(a.rhs), s) == Flow::Exit {
                return Flow::Exit;
            }
            self.targets(Node::Expression(target), s);
            return Flow::Next;
        }
        match target {
            Expression::Variable(_) => {}
            _ => {
                self.children(Node::Expression(target), s);
            }
        }
        self.node(Node::Expression(a.rhs), s)
    }

    fn targets(&mut self, n: Node<'_, '_>, s: &mut State) {
        for child in n.children() {
            match child {
                Node::Expression(e) => match this_property(e) {
                    Some(property) => {
                        self.usage(s, property, e, Some(false));
                        s.insert(text(property), Tri::Yes);
                    }
                    None => self.targets(child, s),
                },
                _ => self.targets(child, s),
            }
        }
    }
}

impl<'x> Ctx<'_, 'x> {
    fn name(&self) -> String {
        text(self.k.name.as_bytes())
    }

    fn this_class(&self, source: usize, class: &Expression) -> Option<String> {
        let name = match class {
            Expression::Self_(_) => return Some(self.name()),
            Expression::Parent(_) => return self.k.direct_parent_class.map(|p| text(p.as_bytes())),
            Expression::Identifier(id) => text(self.sources[source].names.get(&id.span())),
            _ => return None,
        };
        let lower = name.trim_start_matches('\\').to_ascii_lowercase();
        (lower == self.name()
            || self
                .k
                .all_parent_classes
                .iter()
                .any(|p| p.as_bytes() == lower.as_bytes()))
        .then_some(lower)
    }

    fn promoted_of(&self, class: &str) -> Vec<String> {
        let Some(k) = self.c.codebase.get_class_like(class.as_bytes()) else {
            return Vec::new();
        };
        let mut out: Vec<String> = k
            .properties
            .iter()
            .filter(|(name, p)| {
                p.flags.is_promoted_property()
                    && p.read_visibility != Visibility::Private
                    && k.declaring_property_ids
                        .get(*name)
                        .is_some_and(|d| *d == k.name)
            })
            .map(|(name, _)| text(&name.as_bytes()[1..]))
            .collect();
        out.sort();
        out
    }

    fn instance_properties(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .k
            .declaring_property_ids
            .iter()
            .filter_map(|(name, declaring)| {
                let d = self.c.codebase.get_class_like(declaring.as_bytes())?;
                let p = d.properties.get(name)?;
                (!p.flags.is_static()).then(|| text(&name.as_bytes()[1..]))
            })
            .collect();
        out.sort();
        out
    }

    fn never(&self, name: &[u8]) -> bool {
        self.c.codebase.get_function(name).is_some_and(|f| {
            f.return_type_metadata
                .as_ref()
                .is_some_and(|t| t.type_union.is_never())
        })
    }

    fn method_never(&self, method: &[u8]) -> bool {
        matches!(
            super::members::method(self.c, self.k, method),
            Some(super::members::Method::User { f, .. }) if f.return_type_metadata.as_ref().is_some_and(|t| t.type_union.is_never())
        )
    }

    fn start(m: &Method) -> State {
        m.parameter_list
            .parameters
            .iter()
            .filter(|p| p.is_promoted_property())
            .map(|p| (text(&p.variable.name[1..]), Tri::Yes))
            .collect()
    }

    fn walk(&self, m: &MethodAst<'x>, nested: bool, carried: &State) -> (Gathered, Option<State>) {
        let is_static = m.method.modifiers.contains_static();
        let mut walk = Walk {
            ctx: self,
            source: m.source,
            this: !is_static,
            constructor: !nested && m.name.eq_ignore_ascii_case("__construct"),
            parent: None,
            gathered: Gathered::default(),
            returns: Vec::new(),
        };
        let mut s = carried.clone();
        s.extend(Self::start(m.method));
        let flow = match &m.method.body {
            MethodBody::Concrete(b) => walk.statements(b.statements.iter(), &mut s),
            MethodBody::Abstract(_) => return (walk.gathered, None),
        };
        let mut ends = std::mem::take(&mut walk.returns);
        if flow != Flow::Exit {
            ends.insert(0, s);
        }
        let end = ends
            .first()
            .cloned()
            .map(|first| ends[1..].iter().fold(first, |a, b| merge(&a, b)));
        (walk.gathered, end)
    }

    fn called_end(&self, method: &[u8]) -> Option<State> {
        if self.k.kind != SymbolKind::Class
            || self.k.name.as_bytes().starts_with(b"{anonymous-class:")
        {
            return None;
        }
        let lower = text(method).to_ascii_lowercase();
        self.methods
            .get(&lower)
            .and_then(|m| self.walk(m, true, &State::new()).1)
    }
}

fn traits_in<'p>(program: &'p Program<'p>, spans: &[mago_span::Span]) -> Vec<&'p Trait<'p>> {
    fn walk<'p>(
        statements: &'p [Statement<'p>],
        spans: &[mago_span::Span],
        out: &mut Vec<&'p Trait<'p>>,
    ) {
        for s in statements {
            match s {
                Statement::Trait(t) if spans.contains(&t.span()) => out.push(t),
                Statement::Namespace(n) => walk(n.statements().as_slice(), spans, out),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(program.statements.as_slice(), spans, &mut out);
    out
}

/// A member [`class`] can turn into a property node: a typed, non-static, non-abstract property with an item
/// that has no default value, or a method with a typed promoted parameter (a constructor's, once the methods
/// are known).
fn may_add_nodes(member: &ClassLikeMember) -> bool {
    match member {
        ClassLikeMember::Property(p) => {
            p.hint().is_some()
                && !(p.modifiers().iter())
                    .any(|m| matches!(m, Modifier::Static(_) | Modifier::Abstract(_)))
                && match p {
                    Property::Plain(plain) => {
                        (plain.items.iter()).any(|item| !matches!(item, PropertyItem::Concrete(_)))
                    }
                    Property::Hooked(h) => !matches!(h.item, PropertyItem::Concrete(_)),
                }
        }
        ClassLikeMember::Method(m) => (m.parameter_list.parameters.iter())
            .any(|p| p.is_promoted_property() && p.hint.is_some()),
        _ => false,
    }
}

/// Per trait file, whether any trait in it has a member that [`may_add_nodes`], so a class whose own members
/// and traits add no property node skips parsing its trait files. The answer depends only on the file.
#[derive(Default)]
pub(crate) struct Cache(Mutex<HashMap<FileId, bool>>);

impl Cache {
    fn may_add_nodes(&self, file: &File) -> bool {
        mago_codex::reads::record_file(file.id);
        if let Some(&found) = self.0.lock().expect("trait property files").get(&file.id) {
            return found;
        }
        fn any(statements: &[Statement]) -> bool {
            statements.iter().any(|s| match s {
                Statement::Trait(t) => t.members.iter().any(may_add_nodes),
                Statement::Namespace(n) => any(n.statements().as_slice()),
                _ => false,
            })
        }
        let arena = LocalArena::new();
        let (program, _) = parse(&arena, file);
        let found = any(program.statements.as_slice());
        (self.0.lock().expect("trait property files")).insert(file.id, found);
        found
    }
}

pub(super) fn class<'arena>(
    c: &mut Checker<'_, 'arena>,
    k: &ClassLikeMetadata,
    members: &[ClassLikeMember<'arena>],
) {
    if k.kind != SymbolKind::Class {
        return;
    }
    let decl = c.decls.last().expect("in a class");
    let (immutable_doc, parent, class_readonly) =
        (decl.immutable_doc, decl.parent.clone(), decl.is_readonly);
    let immutable = immutable_doc || parent.is_some_and(|p| c.class_immutable(&p, 0));
    let mut traits: Vec<&ClassLikeMetadata> = Vec::new();
    let mut pending: Vec<&ClassLikeMetadata> = vec![k];
    while let Some(t) = pending.pop() {
        for name in &t.used_traits {
            if let Some(t) = c.codebase.get_class_like(name.as_bytes())
                && t.kind == SymbolKind::Trait
                && !traits.iter().any(|x| x.name == t.name)
            {
                traits.push(t);
                pending.push(t);
            }
        }
    }
    let mut files: Vec<Arc<File>> = Vec::new();
    for t in &traits {
        if let Some(file) = c.files.get(&t.span.file_id).and_then(|p| c.sources.get(p))
            && !files.iter().any(|f| f.id == file.id)
        {
            files.push(file.clone());
        }
    }
    if !members.iter().any(may_add_nodes)
        && !files.iter().any(|f| c.trait_properties.may_add_nodes(f))
    {
        return;
    }
    let arenas: Vec<LocalArena> = files.iter().map(|_| LocalArena::new()).collect();
    let parsed: Vec<_> = files
        .iter()
        .zip(&arenas)
        .map(|(f, a)| parse(a, f))
        .collect();
    let display = c.display(k);
    let anonymous = k.name.as_bytes().starts_with(b"{anonymous-class:");
    let context = if anonymous {
        display.clone()
    } else {
        format!("class {display}")
    };
    let mut sources = vec![Source {
        file: c.file,
        names: c.names,
        label: None,
    }];
    let mut trait_members: Vec<(usize, String, &ClassLikeMember)> = Vec::new();
    for (i, (file, (program, names))) in files.iter().zip(&parsed).enumerate() {
        sources.push(Source {
            file,
            names,
            label: Some(format!("{} (in context of {context})", text(&file.name))),
        });
        let spans: Vec<_> = traits
            .iter()
            .filter(|t| t.span.file_id == file.id)
            .map(|t| t.span)
            .collect();
        for t in traits_in(program, &spans) {
            let name = traits
                .iter()
                .find(|x| x.span == t.span())
                .map(|x| text(x.name.as_bytes()).to_ascii_lowercase())
                .unwrap_or_default();
            trait_members.extend(t.members.iter().map(|m| (i + 1, name.clone(), m)));
        }
    }
    let mut methods: HashMap<String, MethodAst> = HashMap::new();
    fn add<'x>(
        methods: &mut HashMap<String, MethodAst<'x>>,
        name: &[u8],
        method: &'x Method<'x>,
        source: usize,
        constructor: bool,
        own: bool,
    ) {
        let order = methods.len();
        let lower = text(name).to_ascii_lowercase();
        if own || !methods.contains_key(&lower) {
            methods.insert(
                lower,
                MethodAst {
                    name: text(name),
                    method,
                    source,
                    order,
                    constructor,
                },
            );
        }
    }
    let mut traits_added = false;
    for member in members {
        match member {
            ClassLikeMember::Method(m) => {
                add(
                    &mut methods,
                    m.name.value,
                    m,
                    0,
                    m.name.value.eq_ignore_ascii_case(b"__construct"),
                    true,
                );
            }
            ClassLikeMember::TraitUse(u) if !traits_added => {
                traits_added = true;
                for (source, _, member) in &trait_members {
                    if let ClassLikeMember::Method(m) = member
                        && !members.iter().any(|x| {
                            matches!(x, ClassLikeMember::Method(own) if own.name.value.eq_ignore_ascii_case(m.name.value))
                        })
                    {
                        add(&mut methods, m.name.value, m, *source, m.name.value.eq_ignore_ascii_case(b"__construct"), false);
                    }
                }
                let _ = u;
            }
            _ => {}
        }
    }
    for member in members {
        let ClassLikeMember::TraitUse(u) = member else {
            continue;
        };
        let TraitUseSpecification::Concrete(spec) = &u.specification else {
            continue;
        };
        for a in &spec.adaptations {
            let TraitUseAdaptation::Alias(a) = a else {
                continue;
            };
            let Some(alias) = &a.alias else {
                continue;
            };
            let (trait_name, method) = match &a.method_reference {
                TraitUseMethodReference::Identifier(m) => (None, m.value),
                TraitUseMethodReference::Absolute(r) => (
                    Some(text(r.trait_name.value()).to_ascii_lowercase()),
                    r.method_name.value,
                ),
            };
            let found = trait_members
                .iter()
                .find_map(|(source, t, member)| match member {
                    ClassLikeMember::Method(m)
                        if m.name.value.eq_ignore_ascii_case(method)
                            && trait_name.as_ref().is_none_or(|n| {
                                let n = n.trim_start_matches('\\');
                                t == n || t.ends_with(&format!("\\{n}"))
                            }) =>
                    {
                        Some((*source, m))
                    }
                    _ => None,
                });
            if let Some((source, m)) = found {
                add(
                    &mut methods,
                    alias.value,
                    m,
                    source,
                    method.eq_ignore_ascii_case(b"__construct"),
                    false,
                );
            }
        }
    }
    let has_constructor = super::members::method(c, k, b"__construct").is_some();
    let constructor = has_constructor.then(|| {
        methods
            .get("__construct")
            .map_or_else(|| "__construct".to_string(), |m| m.name.clone())
    });
    let mut nodes: Vec<PropertyNode> = Vec::new();
    let own = members.iter().map(|m| (0, m));
    for (source, member) in own.chain(trait_members.iter().map(|(s, _, m)| (*s, *m))) {
        let ClassLikeMember::Property(p) = member else {
            continue;
        };
        let from_trait = source != 0;
        let file = sources[source].file;
        let modifiers = p.modifiers();
        let Some(hint) = p.hint() else {
            continue;
        };
        if modifiers
            .iter()
            .any(|m| matches!(m, Modifier::Static(_) | Modifier::Abstract(_)))
        {
            continue;
        }
        let readonly = class_readonly || modifiers.contains_readonly();
        let tags = c
            .php_doc_comment(file.id, p.span().start.offset)
            .map(doc_tags)
            .unwrap_or_default();
        let by_phpdoc = immutable || has_tag(&tags, READONLY_TAGS);
        let items: Vec<&PropertyItem> = match p {
            Property::Plain(plain) => plain.items.iter().collect(),
            Property::Hooked(h) => vec![&h.item],
        };
        for item in items {
            if matches!(item, PropertyItem::Concrete(_)) {
                continue;
            }
            let variable = item.variable();
            let declared = c
                .codebase
                .get_declaring_property_class(k.name.as_bytes(), variable.name)
                .and_then(|d| c.codebase.get_class_like(d.as_bytes()))
                .zip(
                    c.codebase
                        .get_declaring_property(k.name.as_bytes(), variable.name),
                );
            if matches!(p, Property::Hooked(_))
                && declared.is_some_and(|(d, meta)| c.property_virtual(d, meta, variable.name))
            {
                continue;
            }
            let inherited = from_trait
                && k.direct_parent_class
                    .and_then(|parent| {
                        c.codebase
                            .get_declaring_property(parent.as_bytes(), variable.name)
                    })
                    .is_some_and(|meta| meta.read_visibility != Visibility::Private);
            nodes.push(PropertyNode {
                name: text(&variable.name[1..]),
                line: i64::from(file.line_number(item.span().start.offset)) + 1,
                readonly,
                readonly_by_phpdoc: by_phpdoc,
                initial: Tri::from(inherited),
                offsets: offsets(c, &sources[source], hint),
            });
        }
    }
    let mut constructors: Vec<&MethodAst> = methods.values().filter(|m| m.constructor).collect();
    constructors.sort_by_key(|m| m.order);
    for m in constructors {
        let file = sources[m.source].file;
        let tags = c
            .php_doc_comment(file.id, m.method.span().start.offset)
            .map(doc_tags)
            .unwrap_or_default();
        let by_phpdoc = immutable || has_tag(&tags, READONLY_TAGS);
        for p in m
            .method
            .parameter_list
            .parameters
            .iter()
            .filter(|p| p.is_promoted_property())
        {
            let Some(hint) = &p.hint else {
                continue;
            };
            nodes.push(PropertyNode {
                name: text(&p.variable.name[1..]),
                line: i64::from(file.line_number(p.span().start.offset)) + 1,
                readonly: class_readonly || p.modifiers.contains_readonly(),
                readonly_by_phpdoc: by_phpdoc,
                initial: Tri::from(m.source == 0),
                offsets: offsets(c, &sources[m.source], hint),
            });
        }
    }
    let ctx = Ctx {
        c: &*c,
        k,
        sources,
        offsets: nodes.iter().map(|n| (n.name.clone(), n.offsets)).collect(),
        methods,
    };
    let errors = uninitialized(&ctx, nodes, constructor, &display);
    drop(ctx);
    for (identifier, message, line, file) in errors {
        c.report_line(line, identifier, message, file);
    }
}

fn offsets(c: &Checker, source: &Source, hint: &Hint) -> (bool, bool) {
    match hint {
        Hint::Nullable(n) => offsets(c, source, n.hint),
        Hint::Identifier(id) => {
            let Some(name) = source.names.resolve(&id.span()) else {
                return (true, false);
            };
            (true, c.codebase.is_instance_of(name, b"ArrayAccess"))
        }
        Hint::Array(_) | Hint::Iterable(_) => (false, false),
        Hint::Union(_)
        | Hint::Intersection(_)
        | Hint::Object(_)
        | Hint::Mixed(_)
        | Hint::Self_(_)
        | Hint::Static(_)
        | Hint::Parent(_) => (true, false),
        _ => (false, false),
    }
}

type Reported = (&'static str, String, i64, Option<(String, String)>);

fn initialized_in(scope: &State, base: &State) -> State {
    base.iter()
        .map(|(p, t)| (p.clone(), t.or(get(scope, p))))
        .collect()
}

fn uninitialized(
    ctx: &Ctx,
    nodes: Vec<PropertyNode>,
    constructor: Option<String>,
    display: &str,
) -> Vec<Reported> {
    let mut original: HashMap<String, PropertyNode> = HashMap::new();
    let mut initial = State::new();
    let mut uninit: Vec<String> = Vec::new();
    for node in nodes {
        initial.insert(node.name.clone(), node.initial);
        if node.initial != Tri::Yes && !uninit.contains(&node.name) {
            uninit.push(node.name.clone());
        }
        original.insert(node.name.clone(), node);
    }
    let mut walked: Vec<(String, Gathered, Option<State>)> = Vec::new();
    let mut in_order: Vec<&MethodAst> = ctx.methods.values().collect();
    in_order.sort_by_key(|m| m.order);
    let mut carried = State::new();
    for m in in_order {
        let (gathered, end) = ctx.walk(m, false, &carried);
        if m.constructor
            && let Some(end) = &end
        {
            carried.clone_from(end);
        }
        walked.push((m.name.clone(), gathered, end));
    }
    let end_of = |name: &str| {
        walked
            .iter()
            .find(|(n, ..)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, _, end)| end.as_ref())
    };
    let collect = |methods: &[String], mut props: Vec<String>| {
        for m in methods {
            if let Some(end) = end_of(m) {
                props.retain(|p| get(end, p) != Tri::Yes);
            }
        }
        props
    };
    let mut premature: Vec<(String, i64, usize)> = Vec::new();
    let mut additional: Vec<(String, i64)> = Vec::new();
    let remaining = match constructor {
        None => uninit,
        Some(constructor) => {
            let constructors = vec![constructor.clone()];
            let in_constructor: Vec<String> = {
                let left = collect(&constructors, uninit.clone());
                uninit
                    .iter()
                    .filter(|p| !left.contains(p))
                    .cloned()
                    .collect()
            };
            let mut map: HashMap<String, State> = HashMap::new();
            map.insert(constructor, initial.clone());
            let mut methods = constructors.clone();
            loop {
                let (before_map, before_methods) = (map.clone(), methods.clone());
                for (in_method, gathered, _) in &walked {
                    for call in &gathered.calls {
                        if !methods.contains(in_method) {
                            continue;
                        }
                        if in_method != "__construct" && constructors.contains(in_method) {
                            for p in &in_constructor {
                                map.entry(in_method.clone())
                                    .or_default()
                                    .insert(p.clone(), Tri::Yes);
                            }
                        }
                        let base = map.get(in_method).unwrap_or(&initial).clone();
                        let now = initialized_in(&call.state, &base);
                        if let Some(known) = map.get_mut(&call.method) {
                            for (p, t) in now {
                                let old = get(known, &p);
                                known.insert(p, old.and(t));
                            }
                            continue;
                        }
                        if !ctx.methods.contains_key(&call.method.to_ascii_lowercase()) {
                            continue;
                        }
                        map.insert(call.method.clone(), now);
                        methods.push(call.method.clone());
                    }
                }
                if map == before_map && methods == before_methods {
                    break;
                }
            }
            for (function, gathered, _) in &walked {
                let Some(state) = map.get(function) else {
                    continue;
                };
                for usage in &gathered.usages {
                    if !state.contains_key(&usage.property) {
                        continue;
                    }
                    let known = get(state, &usage.property);
                    match usage.write {
                        Some(via_offset) => {
                            let mut has = known;
                            if constructors.contains(function) {
                                has = has.or(usage.state);
                            }
                            if has != Tri::No && !via_offset {
                                additional.push((usage.property.clone(), usage.line));
                            }
                        }
                        None => {
                            let mut has = known.or(usage.state);
                            if has != Tri::Yes
                                && let Some(parent) = usage.parent
                            {
                                has = has.or(parent);
                            }
                            if has != Tri::Yes {
                                premature.push((usage.property.clone(), usage.line, usage.source));
                            }
                        }
                    }
                }
            }
            let mut called: Vec<String> = map.keys().cloned().collect();
            called.sort();
            collect(&called, uninit)
        }
    };
    let mut out: Vec<Reported> = Vec::new();
    if ctx.c.on(crate::rules::Switches::UNINITIALIZED_PROPERTIES) {
        let plain = |name: &str| {
            original
                .get(name)
                .is_some_and(|n| !n.readonly && !n.readonly_by_phpdoc)
        };
        for name in remaining.iter().filter(|n| plain(n)) {
            let message = format!(
                "Class {display} has an uninitialized property ${name}. Give it default value or assign it in the constructor."
            );
            out.push(("property.uninitialized", message, original[name].line, None));
        }
        for (name, line, source) in premature.iter().filter(|(n, ..)| plain(n)) {
            let file = ctx.sources[*source]
                .label
                .clone()
                .map(|l| (l, text(&ctx.sources[*source].file.name)));
            out.push((
                "property.uninitialized",
                format!("Access to an uninitialized property {display}::${name}."),
                *line,
                file,
            ));
        }
    }
    for (by_phpdoc, ident, assign, label) in [
        (
            false,
            "property.uninitializedReadonly",
            "assign.readOnlyProperty",
            "readonly",
        ),
        (
            true,
            "property.uninitializedReadonlyByPhpDoc",
            "assign.readOnlyPropertyByPhpDoc",
            "@readonly",
        ),
    ] {
        let applies = |name: &str| {
            original.get(name).is_some_and(|n| {
                if by_phpdoc {
                    n.readonly_by_phpdoc && !n.readonly
                } else {
                    n.readonly
                }
            })
        };
        for name in remaining.iter().filter(|n| applies(n)) {
            out.push((
                ident,
                format!("Class {display} has an uninitialized {label} property ${name}. Assign it in the constructor."),
                original[name].line,
                None,
            ));
        }
        for (name, line, source) in premature.iter().filter(|(n, ..)| applies(n)) {
            let file = ctx.sources[*source]
                .label
                .clone()
                .map(|l| (l, text(&ctx.sources[*source].file.name)));
            out.push((
                ident,
                format!("Access to an uninitialized {label} property {display}::${name}."),
                *line,
                file,
            ));
        }
        for (name, line) in additional.iter().filter(|(n, _)| applies(n)) {
            let message = if by_phpdoc {
                format!("@readonly property {display}::${name} is already assigned.")
            } else {
                format!("Readonly property {display}::${name} is already assigned.")
            };
            out.push((assign, message, *line, None));
        }
    }
    out
}

impl Checker<'_, '_> {
    fn report_line(
        &mut self,
        line: i64,
        identifier: &str,
        message: String,
        file: Option<(String, String)>,
    ) {
        if self.trait_context.is_some() && !self.walk.contains(super::Walk::IN_TARGET) {
            return;
        }
        let (label, trait_file) = match file {
            Some((label, path)) => (label, Some(path)),
            None => (self.file_name(), None),
        };
        self.errors.push(Error {
            identifier: Some(identifier.to_string()),
            file_path: self.file_name(),
            trait_file_path: trait_file,
            ..Error::new(message, label, Some(line))
        });
    }
}
