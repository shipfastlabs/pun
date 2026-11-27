use std::collections::HashSet;
use std::sync::LazyLock;

use mago_codex::metadata::class_like::ClassLikeMetadata;
use mago_codex::symbol::SymbolKind;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::walker::MutWalker;
use mago_word::word;

use super::Checker;
use super::scope::Guard;
use super::types::flow::unparen;

pub(super) const TIP: &str =
    "Learn more: https://phpstan.org/blog/solving-phpstan-access-to-undefined-property";

#[derive(Default)]
pub(super) struct State {
    allowed: HashSet<(u32, u32)>,
    assigned: HashSet<(u32, u32)>,
    expression_assign: HashSet<(u32, u32)>,
    dim_assigned: HashSet<(u32, u32)>,
    this: crate::bitset::BitSet,
    /// Per function-like: the type the last `@var` tag above a statement gave `$this` (None: its native type).
    this_tags: Vec<Option<pun_phpstan::types::Type>>,
}

pub(super) fn undefined_allowed(c: &Checker, span: Span) -> bool {
    c.property_access.allowed.contains(&key(span))
}

pub(super) fn dim_assigned(c: &Checker, span: Span) -> bool {
    c.property_access.dim_assigned.contains(&key(span))
}

fn key(s: Span) -> (u32, u32) {
    (s.start.offset, s.end.offset)
}

struct Contexts<'s> {
    state: &'s mut State,
}

impl Contexts<'_> {
    fn allow(&mut self, e: &Expression) {
        let e = unparen(e);
        if !matches!(e, Expression::ArrayAppend(_)) {
            self.state.allowed.insert(key(e.span()));
        }
        match e {
            Expression::ArrayAccess(a) => self.allow(a.array),
            Expression::ArrayAppend(a) => self.allow(a.array),
            Expression::Access(Access::Property(p)) => self.allow(p.object),
            Expression::Access(Access::NullSafeProperty(p)) => self.allow(p.object),
            Expression::Call(Call::NullSafeMethod(m)) => self.allow(m.object),
            Expression::Access(Access::StaticProperty(p))
                if !super::method_callable::named_class(p.class) =>
            {
                self.allow(p.class);
            }
            Expression::List(l) => l.elements.iter().for_each(|el| self.element(el)),
            Expression::Array(a) => a.elements.iter().for_each(|el| self.element(el)),
            _ => {}
        }
    }

    fn element(&mut self, el: &ArrayElement) {
        match el {
            ArrayElement::KeyValue(kv) => self.allow(kv.value),
            ArrayElement::Value(v) => self.allow(v.value),
            _ => {}
        }
    }

    fn assigned(&mut self, e: &Expression) {
        match unparen(e) {
            Expression::ArrayAccess(a) => self.assigned(a.array),
            Expression::ArrayAppend(a) => self.assigned(a.array),
            e @ Expression::Access(Access::Property(_) | Access::StaticProperty(_)) => {
                self.state.assigned.insert(key(e.span()));
                self.state.expression_assign.insert(key(e.span()));
            }
            Expression::List(l) => l.elements.iter().for_each(|el| self.destructured(el)),
            Expression::Array(a) => a.elements.iter().for_each(|el| self.destructured(el)),
            _ => {}
        }
    }

    fn dim_assigned(&mut self, e: &Expression) {
        match unparen(e) {
            e @ Expression::ArrayAccess(a) => {
                self.state.dim_assigned.insert(key(e.span()));
                self.dim_assigned(a.array);
            }
            e @ Expression::ArrayAppend(a) => {
                self.state.dim_assigned.insert(key(e.span()));
                self.dim_assigned(a.array);
            }
            Expression::List(l) => l.elements.iter().for_each(|el| self.dim_assigned_item(el)),
            Expression::Array(a) => a.elements.iter().for_each(|el| self.dim_assigned_item(el)),
            _ => {}
        }
    }

    fn dim_assigned_item(&mut self, el: &ArrayElement) {
        match el {
            ArrayElement::KeyValue(kv) => self.dim_assigned(kv.value),
            ArrayElement::Value(v) => self.dim_assigned(v.value),
            _ => {}
        }
    }

    fn expression_assigned(&mut self, e: &Expression) {
        let mut root = unparen(e);
        loop {
            match root {
                Expression::UnaryPrefix(u) if u.operator.is_reference() => {
                    root = unparen(u.operand);
                }
                Expression::ArrayAccess(a) => root = unparen(a.array),
                Expression::ArrayAppend(a) => root = unparen(a.array),
                _ => break,
            }
        }
        match root {
            Expression::List(_) | Expression::Array(_) => {
                let assigned = self.state.assigned.clone();
                self.assigned(root);
                self.state.assigned = assigned;
            }
            _ => {
                self.state.expression_assign.insert(key(root.span()));
            }
        }
    }

    fn destructured(&mut self, el: &ArrayElement) {
        let value = match el {
            ArrayElement::KeyValue(kv) => kv.value,
            ArrayElement::Value(v) => v.value,
            _ => return,
        };
        self.allow(value);
        self.assigned(value);
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Contexts<'_> {
    fn walk_in_isset_construct(&mut self, i: &'ast IssetConstruct<'arena>, (): &mut ()) {
        for v in &i.values {
            self.allow(v);
        }
    }
    fn walk_in_empty_construct(&mut self, e: &'ast EmptyConstruct<'arena>, (): &mut ()) {
        self.allow(e.value);
    }
    fn walk_in_unset(&mut self, u: &'ast Unset<'arena>, (): &mut ()) {
        for v in &u.values {
            self.allow(v);
        }
    }
    fn walk_in_binary(&mut self, b: &'ast Binary<'arena>, (): &mut ()) {
        if b.operator.is_null_coalesce() {
            self.allow(b.lhs);
        }
    }
    fn walk_in_assignment(&mut self, a: &'ast Assignment<'arena>, (): &mut ()) {
        if matches!(a.operator, AssignmentOperator::Assign(_)) {
            self.assigned(a.lhs);
            self.dim_assigned(a.lhs);
            if let Expression::UnaryPrefix(u) = unparen(a.rhs)
                && u.operator.is_reference()
            {
                self.expression_assigned(u.operand);
                self.dim_assigned(u.operand);
            }
        } else if matches!(a.operator, AssignmentOperator::Coalesce(_)) {
            self.expression_assigned(a.lhs);
            self.dim_assigned(a.lhs);
        }
    }
    fn walk_in_foreach(&mut self, f: &'ast Foreach<'arena>, (): &mut ()) {
        if let ForeachTarget::KeyValue(t) = &f.target {
            self.expression_assigned(t.key);
            self.dim_assigned(t.key);
        }
        self.expression_assigned(f.target.value());
        self.dim_assigned(f.target.value());
    }
}

pub(super) fn state(program: &Program) -> State {
    let mut state = State::default();
    Contexts { state: &mut state }.walk_program(program, &mut ());
    state
}

fn fetch_guard(e: &Expression) -> Option<Guard> {
    let Expression::Access(Access::Property(p)) = unparen(e) else {
        return None;
    };
    let (Expression::Variable(Variable::Direct(v)), ClassLikeMemberSelector::Identifier(id)) =
        (unparen(p.object), &p.property)
    else {
        return None;
    };
    Some(var_guard(v.name, id.value))
}

/// The key of a variable or a chain of named property fetches on one (`$queued->job`), as `fetch_guard()` writes it.
fn object_key(e: &Expression) -> Option<String> {
    match unparen(e) {
        Expression::Variable(Variable::Direct(v)) => {
            Some(String::from_utf8_lossy(v.name).into_owned())
        }
        Expression::Access(Access::Property(p)) => {
            let ClassLikeMemberSelector::Identifier(id) = &p.property else {
                return None;
            };
            Some(format!(
                "{}->{}",
                object_key(p.object)?,
                String::from_utf8_lossy(id.value)
            ))
        }
        _ => None,
    }
}

fn var_guard(var: &[u8], name: &[u8]) -> Guard {
    Guard::Expr(format!(
        "{}->{}",
        String::from_utf8_lossy(var),
        String::from_utf8_lossy(name)
    ))
}

pub(super) fn offset_guard(var: &[u8]) -> Guard {
    Guard::Expr(format!("{}[]", String::from_utf8_lossy(var)))
}

fn this_name<'a>(p: &'a PropertyAccess) -> Option<&'a [u8]> {
    let is_this = matches!(unparen(p.object), Expression::Variable(Variable::Direct(v)) if v.name == b"$this");
    match &p.property {
        ClassLikeMemberSelector::Identifier(id) if is_this => Some(id.value),
        _ => None,
    }
}

fn guard(name: &[u8]) -> Guard {
    var_guard(b"$this", name)
}

pub(super) fn narrowed(
    e: &Expression,
    truthy: bool,
    resolve: &dyn Fn(&Identifier) -> String,
    out: &mut Vec<Guard>,
) {
    let e = unparen(e);
    if let Some(g) = fetch_guard(e) {
        out.push(g);
        return;
    }
    if let Some(g) = static_fetch(e, resolve) {
        out.push(g);
        return;
    }
    match e {
        Expression::Construct(Construct::Isset(i)) if truthy => {
            for v in &i.values {
                chain(v, resolve, out);
            }
        }
        Expression::Construct(Construct::Empty(x)) if !truthy => chain(x.value, resolve, out),
        Expression::Binary(b) if b.operator.is_comparison() || b.operator.is_instanceof() => {
            narrowed(b.lhs, truthy, resolve, out);
            narrowed(b.rhs, truthy, resolve, out);
        }
        Expression::Call(Call::Function(f)) => {
            if truthy && let Some(g) = property_exists(f, resolve) {
                out.push(g);
                return;
            }
            if let Some(first) = f.argument_list.arguments.first() {
                narrowed(first.value(), truthy, resolve, out);
            }
        }
        _ => {}
    }
}

fn chain(e: &Expression, resolve: &dyn Fn(&Identifier) -> String, out: &mut Vec<Guard>) {
    let e = unparen(e);
    if let Some(g) = fetch_guard(e) {
        out.push(g);
    }
    if let Some(g) = static_fetch(e, resolve) {
        out.push(g);
    }
    match e {
        Expression::ArrayAccess(a) => {
            if let Expression::Variable(Variable::Direct(v)) = unparen(a.array) {
                out.push(offset_guard(v.name));
            }
            chain(a.array, resolve, out);
        }
        Expression::Access(Access::Property(p)) => chain(p.object, resolve, out),
        Expression::Access(Access::NullSafeProperty(p)) => chain(p.object, resolve, out),
        _ => {}
    }
}

fn class_key(e: &Expression, resolve: &dyn Fn(&Identifier) -> String) -> Option<String> {
    Some(match unparen(e) {
        Expression::Self_(_) => "self".to_string(),
        Expression::Static(_) => "static".to_string(),
        Expression::Parent(_) => "parent".to_string(),
        Expression::Identifier(id) => resolve(id).to_ascii_lowercase(),
        _ => return None,
    })
}

fn static_fetch(e: &Expression, resolve: &dyn Fn(&Identifier) -> String) -> Option<Guard> {
    let Expression::Access(Access::StaticProperty(p)) = e else {
        return None;
    };
    static_key(p, resolve)
}

fn static_key(p: &StaticPropertyAccess, resolve: &dyn Fn(&Identifier) -> String) -> Option<Guard> {
    let class = class_key(p.class, resolve)?;
    Some(Guard::Expr(format!(
        "{class}::{}",
        static_selector_key(&p.property)?
    )))
}

static IDENTIFIER: LazyLock<pcre2::bytes::Regex> = LazyLock::new(|| {
    pcre2::bytes::Regex::new(r"^[a-zA-Z_\x80-\xff][a-zA-Z0-9_\x80-\xff]*$")
        .expect("identifier regexp")
});

fn static_selector_key(v: &Variable) -> Option<String> {
    match v {
        Variable::Direct(v) => Some(String::from_utf8_lossy(v.name).into_owned()),
        Variable::Nested(v) => Some(format!("${{{}}}", static_selector_key(v.variable)?)),
        Variable::Indirect(v) => match unparen(v.expression) {
            Expression::Variable(v) => Some(format!("${{{}}}", static_selector_key(v)?)),
            Expression::Literal(Literal::String(s)) => {
                let value = s.value?;
                Some(if IDENTIFIER.is_match(value).unwrap_or(false) {
                    format!("${}", String::from_utf8_lossy(value))
                } else {
                    format!(
                        "${{string({})}}",
                        value.iter().map(|b| format!("{b:02x}")).collect::<String>()
                    )
                })
            }
            _ => None,
        },
    }
}

fn exists_key(class: &str, name: &str) -> Guard {
    Guard::Expr(format!("property_exists({class}::class, '{name}')"))
}

fn property_exists(f: &FunctionCall, resolve: &dyn Fn(&Identifier) -> String) -> Option<Guard> {
    let Expression::Identifier(id) = f.function else {
        return None;
    };
    let function = match id {
        Identifier::Local(l) => String::from_utf8_lossy(l.value).into_owned(),
        _ => resolve(id),
    };
    if !function
        .trim_start_matches('\\')
        .eq_ignore_ascii_case("property_exists")
    {
        return None;
    }
    let mut args = f.argument_list.arguments.iter();
    let (object, name) = (args.next()?, args.next()?);
    let Expression::Literal(Literal::String(s)) = unparen(name.value()) else {
        return None;
    };
    let name = String::from_utf8_lossy(s.value?).into_owned();
    match unparen(object.value()) {
        Expression::Access(Access::ClassConstant(cc)) => match &cc.constant {
            ClassLikeConstantSelector::Identifier(c) if c.value.eq_ignore_ascii_case(b"class") => {
                Some(exists_key(&class_key(cc.class, resolve)?, &name))
            }
            _ => None,
        },
        object => Some(var_guard(object_key(object)?.as_bytes(), name.as_bytes())),
    }
}

impl Checker<'_, '_> {
    pub(super) fn enter_this(&mut self, defined: bool) {
        self.property_access.this.push(defined);
        self.property_access.this_tags.push(None);
    }

    /// A closure takes `$this` with its type from the enclosing scope (`enterAnonymousFunction()`).
    pub(super) fn enter_closure_this(&mut self, is_static: bool) {
        let parent = self.property_access.this.last().unwrap_or(false);
        self.property_access.this.push(parent && !is_static);
        let tag = self.property_access.this_tags.last().cloned().flatten();
        self.property_access
            .this_tags
            .push(tag.filter(|_| !is_static));
    }

    /// Whether a `@var` tag above an earlier statement of this function-like gave `$this` a type.
    pub(super) fn this_tagged(&self) -> bool {
        self.property_access
            .this_tags
            .last()
            .is_some_and(Option::is_some)
    }

    /// Records the type a `@var` tag gives `$this`, and returns the type it had.
    pub(super) fn retype_this(
        &mut self,
        tag: pun_phpstan::types::Type,
    ) -> Option<pun_phpstan::types::Type> {
        self.property_access.this_tags.last_mut()?.replace(tag)
    }

    pub(super) fn incremented(&mut self, operand: &Expression) {
        if let Expression::Access(Access::StaticProperty(p)) = unparen(operand) {
            static_check(self, p);
        }
        if matches!(unparen(operand), Expression::Variable(Variable::Direct(v)) if v.name == b"$this")
            && let Some(last) = self.property_access.this.len().checked_sub(1)
        {
            self.property_access.this.set(last, false);
        }
    }

    pub(super) fn this_defined(&self) -> bool {
        self.property_access.this.last().unwrap_or(false)
    }

    pub(super) fn allows_dynamic_properties(&self, k: &ClassLikeMetadata) -> bool {
        allows_dynamic_properties(self, k)
    }

    pub(super) fn leave_this(&mut self) {
        self.property_access.this.pop();
        self.property_access.this_tags.pop();
    }

    fn has_expression_type(&self, name: &[u8]) -> bool {
        let g = guard(name);
        self.guards.contains(&g)
    }

    pub(super) fn has_fetch_type(&self, object: &Expression, name: &str) -> bool {
        object_key(object).is_some_and(|object| {
            self.guards
                .contains(&var_guard(object.as_bytes(), name.as_bytes()))
        })
    }
}

pub(super) fn assigned(c: &mut Checker, a: &Assignment) {
    remember(c, a.lhs);
}

fn remember(c: &mut Checker, e: &Expression) {
    let e = unparen(e);
    if let Some(g) = fetch_guard(e) {
        c.guards.push(g);
        return;
    }
    if let Expression::Access(Access::StaticProperty(p)) = e
        && !matches!(p.property, Variable::Direct(_))
        && let Some(g) = static_key(p, &|id| c.resolved(id))
    {
        c.guards.push(g);
        return;
    }
    let items = match e {
        Expression::ArrayAccess(x) => return remember(c, x.array),
        Expression::ArrayAppend(x) => return remember(c, x.array),
        Expression::List(l) => &l.elements,
        Expression::Array(a) => &a.elements,
        _ => return,
    };
    for el in items {
        match el {
            ArrayElement::KeyValue(kv) => remember(c, kv.value),
            ArrayElement::Value(v) => remember(c, v.value),
            _ => {}
        }
    }
}

pub(super) fn read(c: &mut Checker, p: &PropertyAccess) {
    if let Some(name) = super::types::dynamic_names::member(&p.property) {
        super::types::dynamic_names::check(
            c,
            name,
            true,
            "Property",
            p.object,
            false,
            "property.nameNotString",
        );
    }
    if c.property_access.assigned.contains(&key(p.span())) {
        return;
    }
    check(c, p, false);
}

pub(super) fn assign(c: &mut Checker, a: &Assignment) {
    if matches!(a.operator, AssignmentOperator::Assign(_)) {
        written(c, a.lhs);
    }
}

fn written(c: &mut Checker, e: &Expression) {
    match unparen(e) {
        Expression::ArrayAccess(x) => written(c, x.array),
        Expression::ArrayAppend(x) => written(c, x.array),
        Expression::Access(Access::Property(p)) => check(c, p, true),
        Expression::Access(Access::StaticProperty(p)) => static_check(c, p),
        Expression::List(l) => l.elements.iter().for_each(|el| written_item(c, el)),
        Expression::Array(a) => a.elements.iter().for_each(|el| written_item(c, el)),
        _ => {}
    }
}

fn written_item(c: &mut Checker, el: &ArrayElement) {
    match el {
        ArrayElement::KeyValue(kv) => written(c, kv.value),
        ArrayElement::Value(v) => written(c, v.value),
        _ => {}
    }
}

pub(super) fn allows_dynamic_properties(c: &Checker, k: &ClassLikeMetadata) -> bool {
    allows_dynamic_properties_at(c, k, &mut HashSet::from([k.name]))
}

fn allows_dynamic_properties_at(
    c: &Checker,
    k: &ClassLikeMetadata,
    seen: &mut HashSet<mago_word::Word>,
) -> bool {
    if k.kind == SymbolKind::Enum {
        return false;
    }
    if c.php_version < 80200 {
        return true;
    }
    if ["__get", "__set", "__isset"]
        .iter()
        .any(|m| has_native_method(c, k, m))
    {
        return true;
    }
    if super::require_tags::tag_types(c, k, super::require_tags::EXTENDS)
        .into_iter()
        .any(|ty| {
            let pun_phpstan::types::Type::Object(name, _, _) = ty else {
                return false;
            };
            c.codebase
                .get_class_like(name.as_bytes())
                .is_some_and(|required| {
                    seen.insert(required.name) && allows_dynamic_properties_at(c, required, seen)
                })
        })
    {
        return true;
    }
    if k.flags.is_readonly() {
        return false;
    }
    if c.codebase.is_instance_of(k.name.as_bytes(), b"stdClass") {
        return true;
    }
    std::iter::once(&k.name)
        .chain(k.all_parent_classes.iter())
        .filter_map(|n| c.codebase.get_class_like(n.as_bytes()))
        .any(|p| {
            p.attributes.iter().any(|a| {
                a.name
                    .as_bytes()
                    .eq_ignore_ascii_case(b"AllowDynamicProperties")
            })
        })
}

fn has_native_method(c: &Checker, k: &ClassLikeMetadata, name: &str) -> bool {
    super::members::method(c, k, name.as_bytes()).is_some()
}

pub(super) fn native_property(c: &Checker, k: &ClassLikeMetadata, property: &[u8]) -> Option<bool> {
    let p = c
        .codebase
        .get_declaring_property(k.name.as_bytes(), property)?;
    Some(p.flags.is_static() || is_static_hooked(c, p))
}

fn is_static_hooked(c: &Checker, p: &mago_codex::metadata::property::PropertyMetadata) -> bool {
    let (Some(span), Some(name)) = (p.span, p.name_span) else {
        return false;
    };
    if p.hooks.is_empty() {
        return false;
    }
    c.file_contents(span.file_id).is_some_and(|contents| {
        let head = &contents[span.start.offset as usize..name.start.offset as usize];
        let modifiers = head.rsplit(|&b| b == b']').next().unwrap_or(head);
        modifiers
            .split(|b| b.is_ascii_whitespace())
            .any(|w| w.eq_ignore_ascii_case(b"static"))
    })
}

pub(super) fn has_instance_property(c: &Checker, k: &ClassLikeMetadata, property: &[u8]) -> bool {
    if native_property(c, k, property) == Some(false) {
        return true;
    }
    if k.kind == SymbolKind::Enum {
        return false;
    }
    allows_dynamic_properties(c, k)
        && (k.magic_property_ids.contains_key(&word(property)) || is_universal_object_crate(c, k))
}

fn is_universal_object_crate(c: &Checker, k: &ClassLikeMetadata) -> bool {
    let larastan: &[&[u8]] = match c.larastan {
        Some(_) => &[
            b"illuminate\\http\\request",
            b"illuminate\\support\\optional",
        ],
        None => &[],
    };
    std::iter::once(b"stdclass".as_slice())
        .chain(larastan.iter().copied())
        .any(|crate_| k.name.as_bytes() == crate_ || k.all_parent_classes.contains(&word(crate_)))
}

fn check(c: &mut Checker, p: &PropertyAccess, write: bool) {
    let Some(name) = this_name(p) else {
        let allowed = !write && c.property_access.allowed.contains(&key(p.span()));
        super::types::property_fetch(c, p.span(), p.object, &p.property, false, allowed);
        return;
    };
    if !c.property_access.this.last().unwrap_or(false) {
        return;
    }
    let Some(class) = c.class_scope() else {
        return;
    };
    let mut property = Vec::with_capacity(name.len() + 1);
    property.push(b'$');
    property.extend_from_slice(name);
    let allowed = !write && c.property_access.allowed.contains(&key(p.span()));
    let has = has_instance_property(c, class, &property);
    let has_static = native_property(c, class, &property) == Some(true);
    if has {
        return;
    }
    if c.has_property(
        &pun_phpstan::types::Type::Object(
            String::from_utf8_lossy(class.name.as_bytes()).into_owned(),
            vec![],
            false,
        ),
        &property,
        false,
    )
    .yes()
    {
        return;
    }
    let is_final = class.flags.is_final()
        || class
            .original_name
            .as_bytes()
            .starts_with(b"{anonymous-class:");
    let maybe = allows_dynamic_properties(c, class) || !is_final;
    if maybe && !has_static && allowed {
        return;
    }
    if c.has_expression_type(name) {
        return;
    }
    if (c.level == 0 || c.larastan.is_some())
        && (has_native_method(c, class, "__get") || has_native_method(c, class, "__set"))
    {
        return;
    }
    let name_line = p.property.span();
    let display_name = String::from_utf8_lossy(name).into_owned();
    let mut parent = class
        .direct_parent_class
        .and_then(|n| c.codebase.get_class_like(n.as_bytes()));
    while let Some(k) = parent {
        if has_instance_property(c, k, &property) {
            if k.properties
                .get(&word(&property))
                .is_some_and(|x| x.read_visibility.is_private())
            {
                let message = format!(
                    "Access to private property ${display_name} of parent class {}.",
                    c.display(k)
                );
                c.report(name_line, "property.private", message, None);
            }
            return;
        }
        parent = k
            .direct_parent_class
            .and_then(|n| c.codebase.get_class_like(n.as_bytes()));
    }
    if has_static {
        let declaring = c
            .codebase
            .get_declaring_property_class(class.name.as_bytes(), &property)
            .and_then(|d| c.codebase.get_class_like(d.as_bytes()))
            .map_or_else(|| c.display(class), |d| c.display(d));
        c.report(
            name_line,
            "staticProperty.nonStaticAccess",
            format!("Non-static access to static property {declaring}::${display_name}."),
            None,
        );
        return;
    }
    let described = c.describe_object(class);
    c.report(
        p.span(),
        "property.notFound",
        format!("Access to an undefined property {described}::${display_name}."),
        Some(TIP),
    );
}

pub(super) fn null_safe_read(c: &mut Checker, p: &NullSafePropertyAccess) {
    if let Some(name) = super::types::dynamic_names::member(&p.property) {
        super::types::dynamic_names::check(
            c,
            name,
            true,
            "Property",
            p.object,
            true,
            "property.nameNotString",
        );
    }
    let allowed = c.property_access.allowed.contains(&key(p.span()));
    super::types::property_fetch(c, p.span(), p.object, &p.property, true, allowed);
}

pub(super) fn static_read(c: &mut Checker, p: &StaticPropertyAccess) {
    if let Some(name) = super::types::dynamic_names::static_property(&p.property) {
        super::types::dynamic_names::check(
            c,
            name,
            true,
            "Static property",
            p.class,
            false,
            "staticProperty.nameNotString",
        );
    }
    if c.property_access.assigned.contains(&key(p.span())) {
        return;
    }
    static_check(c, p);
}

enum Holder<'a> {
    Class(&'a ClassLikeMetadata, bool),
    Done,
}

pub(super) fn has_static_property(c: &Checker, k: &ClassLikeMetadata, property: &[u8]) -> bool {
    native_property(c, k, property) == Some(true)
}

fn static_check(c: &mut Checker, p: &StaticPropertyAccess) {
    if let Variable::Direct(v) = &p.property {
        return static_check_named(c, p, v.name);
    }
    let names = super::types::dynamic_names::static_property(&p.property)
        .and_then(|at| c.phpstan_type_at(at))
        .map_or_else(Vec::new, |ty| ty.constant_strings());
    for mut property in names {
        property.insert(0, b'$');
        static_check_named(c, p, &property);
    }
}

fn static_check_named(c: &mut Checker, p: &StaticPropertyAccess, property: &[u8]) {
    let name = String::from_utf8_lossy(&property[1..]).into_owned();
    let at = super::types::dynamic_names::static_property(&p.property)
        .unwrap_or_else(|| p.property.span());
    let holder = match (unparen(p.class), c.class_scope()) {
        (e @ (Expression::Self_(_) | Expression::Static(_)), scope) => {
            let written = if matches!(e, Expression::Self_(_)) {
                "self"
            } else {
                "static"
            };
            let Some(k) = scope else {
                c.report(
                    at,
                    &format!("outOfClass.{written}"),
                    format!(
                        "Accessing {}::${name} outside of class scope.",
                        c.written(e.span())
                    ),
                    None,
                );
                return;
            };
            Holder::Class(k, written == "static")
        }
        (e @ Expression::Parent(_), scope) => {
            let Some(scope) = scope else {
                c.report(
                    at,
                    "outOfClass.parent",
                    format!(
                        "Accessing {}::${name} outside of class scope.",
                        c.written(e.span())
                    ),
                    None,
                );
                return;
            };
            let parent = scope
                .direct_parent_class
                .and_then(|p| c.codebase.get_class_like(p.as_bytes()));
            match parent {
                None => {
                    let action = format!(
                        "{}::{}() accesses parent::${name}",
                        c.display(scope),
                        c.function_name()
                    );
                    c.report_no_parent(at, scope, &action);
                    return;
                }
                Some(k) => Holder::Class(k, false),
            }
        }
        (Expression::Identifier(id), _) => {
            let class = c.resolved(id);
            match c.class_like(&class) {
                super::reflection::Lookup::NotFound => {
                    if !c.in_class_exists(&class) {
                        c.report(
                            at,
                            "class.notFound",
                            format!(
                                "Access to static property ${name} on an unknown class {class}."
                            ),
                            Some(super::DISCOVERING_SYMBOLS_TIP),
                        );
                    }
                    return;
                }
                super::reflection::Lookup::Opaque => {
                    super::class_names::check(
                        c,
                        &[super::class_names::Pair::name(class, id.span())],
                        true,
                    );
                    Holder::Done
                }
                super::reflection::Lookup::Found(k) => {
                    let member = super::internal::static_property_member(c, k, name.as_bytes());
                    let location = super::class_names::Location::StaticPropertyAccess(member);
                    super::class_names::check_at(
                        c,
                        &[super::class_names::Pair::name(class, id.span())],
                        Some(&location),
                        true,
                    );
                    Holder::Class(k, false)
                }
            }
        }
        _ => {
            let named = StaticPropertyAccess {
                class: p.class,
                double_colon: p.double_colon,
                property: Variable::Direct(DirectVariable {
                    span: at,
                    name: property,
                }),
            };
            return super::types::static_property_fetch(c, &named, property);
        }
    };
    let Holder::Class(class, is_static) = holder else {
        return;
    };
    let declared = c
        .codebase
        .get_declaring_property(class.name.as_bytes(), property)
        .filter(|_| has_static_property(c, class, property));
    let Some(declared) = declared else {
        let resolve = |id: &Identifier| c.resolved(id);
        if static_key(p, &resolve).is_some_and(|g| c.guards.contains(&g)) {
            return;
        }
        let mut parent = class
            .direct_parent_class
            .and_then(|n| c.codebase.get_class_like(n.as_bytes()));
        while let Some(k) = parent {
            if has_static_property(c, k, property) {
                if k.properties
                    .get(&word(property))
                    .is_some_and(|x| x.read_visibility.is_private())
                {
                    let message = format!(
                        "Access to private static property ${name} of parent class {}.",
                        c.display(k)
                    );
                    c.report(at, "staticProperty.private", message, None);
                }
                return;
            }
            parent = k
                .direct_parent_class
                .and_then(|n| c.codebase.get_class_like(n.as_bytes()));
        }
        if has_instance_property(c, class, property) {
            let declaring = c
                .codebase
                .get_declaring_property_class(class.name.as_bytes(), property)
                .and_then(|d| c.codebase.get_class_like(d.as_bytes()))
                .map_or_else(|| c.display(class), |d| c.display(d));
            c.report(
                at,
                "property.staticAccess",
                format!("Static access to instance property {declaring}::${name}."),
                None,
            );
            return;
        }
        let exists = class_key(p.class, &resolve).map(|k| exists_key(&k, &name));
        if exists.is_some_and(|g| c.guards.contains(&g)) {
            return;
        }
        let described = if is_static {
            format!("static({})", c.display(class))
        } else {
            c.display(class)
        };
        c.report(
            at,
            "staticProperty.notFound",
            format!("Access to an undefined static property {described}::${name}."),
            None,
        );
        return;
    };
    let visibility = declared.read_visibility;
    let declaring = c
        .codebase
        .get_declaring_property_class(class.name.as_bytes(), property)
        .and_then(|d| c.codebase.get_class_like(d.as_bytes()))
        .filter(|d| d.kind != SymbolKind::Trait)
        .unwrap_or(class);
    let declaring_name = String::from_utf8_lossy(declaring.original_name.as_bytes()).into_owned();
    if super::calls::can_call(c, c.class_scope(), &declaring_name, visibility) {
        return;
    }
    let v = super::restricted(visibility);
    let message = format!(
        "Access to {v} property ${name} of class {}.",
        c.display(declaring)
    );
    c.report(at, &format!("staticProperty.{v}"), message, None);
}

pub(super) fn in_expression_assign(c: &Checker, e: Span) -> bool {
    c.property_access.expression_assign.contains(&key(e))
}
