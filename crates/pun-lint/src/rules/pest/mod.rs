mod no_before_all_in_describe;
mod no_expect_exception;
mod no_invalid_repeat;
mod no_negated_boolean_matcher;
mod no_redundant_type_expectation;
mod no_redundant_uses;
mod no_static_test_closure;
mod prefer_chained_expect;
mod prefer_defer;
mod prefer_each_modifier;
mod prefer_expect;
mod prefer_extend;
mod prefer_filesystem_matchers;
mod prefer_not_modifier;
mod prefer_separate_expects;
mod prefer_sequence;
mod prefer_specific_expectations;
mod prefer_strict_equality_matchers;
mod prefer_to_be_alpha;
mod prefer_to_be_alpha_numeric;
mod prefer_to_be_between;
mod prefer_to_be_digits;
mod prefer_to_be_directory;
mod prefer_to_be_empty;
mod prefer_to_be_file;
mod prefer_to_be_in;
mod prefer_to_be_infinite;
mod prefer_to_be_instance_of;
mod prefer_to_be_json;
mod prefer_to_be_list;
mod prefer_to_be_lowercase;
mod prefer_to_be_nan;
mod prefer_to_be_truthy_falsy;
mod prefer_to_be_uppercase;
mod prefer_to_be_url;
mod prefer_to_be_uuid;
mod prefer_to_contain;
mod prefer_to_contain_equal;
mod prefer_to_contain_only_instances_of;
mod prefer_to_end_with;
mod prefer_to_equal_canonicalizing;
mod prefer_to_equal_with_delta;
mod prefer_to_have_count;
mod prefer_to_have_key;
mod prefer_to_have_keys;
mod prefer_to_have_length;
mod prefer_to_have_method_on_class;
mod prefer_to_have_properties;
mod prefer_to_have_property;
mod prefer_to_have_same_size;
mod prefer_to_match;
mod prefer_to_match_array;
mod prefer_to_match_object;
mod prefer_to_start_with;
mod prefer_to_throw;
mod prefer_type_matchers;
mod require_type_matcher_first;

use crate::problem::{Edit, Safety, Span};
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::symbol::SymbolKind;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::object::TObject;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_syntax::cst::{
    Access, Argument, ArgumentList, ArrowFunction, Call, ClassLikeMemberSelector, Closure,
    Expression, FunctionCall, Identifier, Keyword, MethodCall, Node, NodeKind, PropertyAccess,
    Statement,
};
use std::fmt::Write as _;

use super::types::facts::facts;
use crate::rules::phpunit::expr_text;
use crate::rules::support::expressions::text;
use crate::syntax::{node_span, unparenthesize};
use crate::{Context, Report};

rules! {
    pub(super) fn all {
        no_before_all_in_describe => NoBeforeAllInDescribe,
        no_expect_exception => NoExpectException::default(),
        no_invalid_repeat => NoInvalidRepeat,
        no_negated_boolean_matcher => NoNegatedBooleanMatcher,
        no_redundant_type_expectation => NoRedundantTypeExpectation,
        no_redundant_uses => NoRedundantUses,
        no_static_test_closure => NoStaticTestClosure,
        prefer_defer => PreferDefer,
        prefer_each_modifier => PreferEachModifier,
        prefer_expect => PreferExpect,
        prefer_extend => PreferExtend,
        prefer_filesystem_matchers => PreferFilesystemMatchers,
        prefer_not_modifier => PreferNotModifier,
        prefer_sequence => PreferSequence,
        prefer_specific_expectations => PreferSpecificExpectations,
        prefer_strict_equality_matchers => PreferStrictEqualityMatchers,
        prefer_to_be_alpha => PreferToBeAlpha,
        prefer_to_be_alpha_numeric => PreferToBeAlphaNumeric,
        prefer_to_be_between => PreferToBeBetween,
        prefer_to_be_digits => PreferToBeDigits,
        prefer_to_be_directory => PreferToBeDirectory,
        prefer_to_be_empty => PreferToBeEmpty,
        prefer_to_be_file => PreferToBeFile,
        prefer_to_be_in => PreferToBeIn,
        prefer_to_be_infinite => PreferToBeInfinite,
        prefer_to_be_instance_of => PreferToBeInstanceOf,
        prefer_to_be_json => PreferToBeJson,
        prefer_to_be_list => PreferToBeList,
        prefer_to_be_lowercase => PreferToBeLowercase,
        prefer_to_be_nan => PreferToBeNan,
        prefer_to_be_truthy_falsy => PreferToBeTruthyFalsy,
        prefer_to_be_uppercase => PreferToBeUppercase,
        prefer_to_be_url => PreferToBeUrl,
        prefer_to_be_uuid => PreferToBeUuid,
        prefer_to_contain => PreferToContain,
        prefer_to_contain_equal => PreferToContainEqual,
        prefer_to_contain_only_instances_of => PreferToContainOnlyInstancesOf,
        prefer_to_end_with => PreferToEndWith,
        prefer_to_equal_canonicalizing => PreferToEqualCanonicalizing,
        prefer_to_equal_with_delta => PreferToEqualWithDelta,
        prefer_to_have_count => PreferToHaveCount,
        prefer_to_have_key => PreferToHaveKey,
        prefer_to_have_keys => PreferToHaveKeys,
        prefer_to_have_length => PreferToHaveLength,
        prefer_to_have_method_on_class => PreferToHaveMethodOnClass,
        prefer_to_have_properties => PreferToHaveProperties,
        prefer_to_have_property => PreferToHaveProperty,
        prefer_to_have_same_size => PreferToHaveSameSize,
        prefer_to_match => PreferToMatch,
        prefer_to_match_array => PreferToMatchArray,
        prefer_to_match_object => PreferToMatchObject,
        prefer_to_start_with => PreferToStartWith,
        prefer_to_throw => PreferToThrow::default(),
        prefer_type_matchers => PreferTypeMatchers,
        prefer_chained_expect => PreferChainedExpect::default(),
        require_type_matcher_first => RequireTypeMatcherFirst,
        prefer_separate_expects => PreferSeparateExpects,
    }
}

pub(crate) fn method_call<'a>(expr: &'a Expression<'a>) -> Option<&'a MethodCall<'a>> {
    match unparenthesize(expr) {
        Expression::Call(Call::Method(call)) => Some(call),
        _ => None,
    }
}

pub(crate) fn property_fetch<'a>(expr: &'a Expression<'a>) -> Option<&'a PropertyAccess<'a>> {
    match unparenthesize(expr) {
        Expression::Access(Access::Property(fetch)) => Some(fetch),
        _ => None,
    }
}

pub(crate) fn func_call<'a>(expr: &'a Expression<'a>) -> Option<&'a FunctionCall<'a>> {
    match unparenthesize(expr) {
        Expression::Call(Call::Function(call)) => Some(call),
        _ => None,
    }
}

pub(crate) fn identifier<'a>(selector: &ClassLikeMemberSelector<'a>) -> Option<&'a [u8]> {
    match selector {
        ClassLikeMemberSelector::Identifier(id) => Some(id.value),
        _ => None,
    }
}

pub(crate) fn is_name(selector: &ClassLikeMemberSelector<'_>, name: &str) -> bool {
    identifier(selector).is_some_and(|n| n.eq_ignore_ascii_case(name.as_bytes()))
}

pub(crate) fn function_name<'a>(call: &FunctionCall<'a>) -> Option<&'a [u8]> {
    match call.function {
        Expression::Identifier(Identifier::FullyQualified(id)) => Some(&id.value[1..]),
        Expression::Identifier(id) => Some(id.value()),
        _ => None,
    }
}

pub(crate) fn chain_root<'a>(call: &'a MethodCall<'a>) -> Option<&'a FunctionCall<'a>> {
    let mut current = call.object;
    loop {
        current = match unparenthesize(current) {
            Expression::Call(Call::Method(m)) => m.object,
            Expression::Access(Access::Property(p)) => p.object,
            Expression::Call(Call::Function(f)) => return Some(f),
            _ => return None,
        };
    }
}

pub(crate) fn expect_call<'a>(call: &'a MethodCall<'a>) -> Option<&'a FunctionCall<'a>> {
    chain_root(call).filter(|root| crate::rules::support::expressions::named(root, "expect"))
}

pub(crate) fn is_expect_chain(call: &MethodCall<'_>) -> bool {
    expect_call(call).is_some()
}

pub(crate) fn expect_argument<'a>(call: &'a MethodCall<'a>) -> Option<&'a Expression<'a>> {
    expect_call(call).and_then(|e| first_value(&e.argument_list))
}

pub(crate) fn first_value<'a>(list: &'a ArgumentList<'a>) -> Option<&'a Expression<'a>> {
    list.arguments.first().map(Argument::value)
}

#[derive(Clone, Copy)]
pub(crate) struct Holder<'a> {
    pub span: Span,
    pub arguments: &'a ArgumentList<'a>,
    method: Option<(&'a Expression<'a>, &'a [u8])>,
    name_end: usize,
}

impl<'a> Holder<'a> {
    fn function(call: &'a FunctionCall<'a>) -> Self {
        Holder {
            span: node_span(call),
            arguments: &call.argument_list,
            method: None,
            name_end: node_span(call.function).end,
        }
    }

    fn method(call: &'a MethodCall<'a>) -> Self {
        Holder {
            span: node_span(call),
            arguments: &call.argument_list,
            method: Some((call.object, identifier(&call.method).unwrap_or_default())),
            name_end: 0,
        }
    }

    pub(crate) fn set_arguments(&self, ctx: &Context<'_>, value: &Expression<'_>) -> Vec<Edit> {
        let value = expr_text(ctx, value);
        match (self.arguments.arguments.as_slice(), self.method) {
            ([only], _) => vec![Edit::new(node_span(only), value)],
            (_, None) => vec![Edit::new(
                Span::new(self.name_end, self.span.end),
                format!("({value})"),
            )],
            (_, Some((object, name))) => reprint_from_object(
                object,
                self.span.end,
                format!("->{}({value})", String::from_utf8_lossy(name)),
            ),
        }
    }
}

pub(crate) fn subject_holder<'a>(call: &'a MethodCall<'a>) -> Option<Holder<'a>> {
    let root = expect_call(call)?;
    let mut current = call.object;
    loop {
        current = match unparenthesize(current) {
            Expression::Call(Call::Method(m)) if is_name(&m.method, "and") => {
                return Some(Holder::method(m));
            }
            Expression::Call(Call::Method(m)) => m.object,
            Expression::Access(Access::Property(p)) => p.object,
            _ => return Some(Holder::function(root)),
        };
    }
}

pub(crate) fn matcher_subject<'a>(call: &'a MethodCall<'a>) -> Option<&'a Expression<'a>> {
    subject_holder(call).and_then(|h| first_value(h.arguments))
}

pub(crate) fn applied_directly(call: &MethodCall<'_>, holder: &Holder<'_>) -> bool {
    let mut var = unparenthesize(call.object);
    if let Some(not) = property_fetch(var).filter(|p| is_name(&p.property, "not")) {
        var = unparenthesize(not.object);
    }
    node_span(var) == holder.span
}

pub(crate) fn has_not(call: &MethodCall<'_>) -> bool {
    property_fetch(call.object).is_some_and(|p| is_name(&p.property, "not"))
}

pub(crate) fn set_first_argument(
    ctx: &Context<'_>,
    list: &ArgumentList<'_>,
    value: &Expression<'_>,
) -> Vec<Edit> {
    (list.arguments.iter().take(1))
        .map(|first| Edit::new(node_span(first), expr_text(ctx, value)))
        .collect()
}

pub(crate) fn set_call_arguments(
    call: &MethodCall<'_>,
    name: &str,
    values: &[String],
) -> Vec<Edit> {
    let old = call.argument_list.arguments.as_slice();
    if old.len() == values.len() {
        let mut edits: Vec<Edit> = (old.iter().zip(values))
            .map(|(a, v)| Edit::new(node_span(a), v.clone()))
            .collect();
        edits.push(rename(call, name));
        return edits;
    }
    if old.is_empty() {
        let at = call.argument_list.left_parenthesis.end.offset as usize;
        return vec![
            rename(call, name),
            Edit::new(Span::new(at, at), values.join(", ")),
        ];
    }
    reprint_from_object(
        call.object,
        node_span(call).end,
        format!("->{name}({})", values.join(", ")),
    )
}

pub(crate) fn texts(ctx: &Context<'_>, values: &[&Expression<'_>]) -> Vec<String> {
    values
        .iter()
        .map(|v| expr_text(ctx, v).to_string())
        .collect()
}

pub(crate) fn reprint_from_object(object: &Expression<'_>, end: usize, rest: String) -> Vec<Edit> {
    let outer = node_span(object);
    let inner = node_span(unparenthesize(object));
    let mut edits = vec![Edit::new(Span::new(inner.end, end), rest)];
    if outer.start != inner.start {
        edits.insert(0, Edit::new(Span::new(outer.start, inner.start), ""));
    }
    edits
}

pub(crate) fn arguments_text(ctx: &Context<'_>, values: &[&Expression<'_>]) -> String {
    values
        .iter()
        .map(|v| expr_text(ctx, v))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn rename(call: &MethodCall<'_>, name: &str) -> Edit {
    Edit::new(node_span(&call.method), name)
}

pub(crate) struct Extracted<'a> {
    pub holder: Holder<'a>,
    pub function: &'a FunctionCall<'a>,
    pub method: &'a [u8],
}

pub(crate) fn extract_function<'a>(
    call: &'a MethodCall<'a>,
    functions: &[&str],
    valid_methods: &[&str],
) -> Option<Extracted<'a>> {
    let method = identifier(&call.method)?;
    if !valid_methods.iter().any(|m| m.as_bytes() == method) {
        return None;
    }
    let holder = subject_holder(call)?;
    if !applied_directly(call, &holder) {
        return None;
    }
    let function = func_call(first_value(holder.arguments)?)?;
    let name = function_name(function)?;
    if !functions.iter().any(|f| f.as_bytes() == name) {
        return None;
    }
    Some(Extracted {
        holder,
        function,
        method,
    })
}

pub(crate) fn needs_not(method: &[u8], call: &MethodCall<'_>) -> bool {
    (method == b"toBeFalse") != has_not(call)
}

pub(crate) fn build_matcher_call(
    ctx: &Context<'_>,
    call: &MethodCall<'_>,
    holder: Holder<'_>,
    value: &Expression<'_>,
    matcher: &str,
    args: &[&Expression<'_>],
    not: bool,
) -> Vec<Edit> {
    let not = if not { "->not" } else { "" };
    let mut edits = set_first_argument(ctx, holder.arguments, value);
    edits.push(Edit::new(
        Span::new(holder.span.end, node_span(call).end),
        format!("{not}->{matcher}({})", arguments_text(ctx, args)),
    ));
    edits
}

pub(crate) fn function_matcher<'a>(
    ctx: &Context<'_>,
    call: &'a MethodCall<'a>,
    function: &str,
    matcher: &str,
    shape: impl Fn(&[&'a Expression<'a>]) -> Option<(&'a Expression<'a>, Vec<&'a Expression<'a>>)>,
    out: &mut Vec<Report>,
) {
    let Some(extracted) = extract_function(call, &[function], &["toBeTrue", "toBeFalse"]) else {
        return;
    };
    let values: Vec<&Expression<'_>> = (extracted.function.argument_list.arguments.iter())
        .map(Argument::value)
        .collect();
    let Some((subject, args)) = shape(&values) else {
        return;
    };
    let not = needs_not(extracted.method, call);
    let edits = build_matcher_call(ctx, call, extracted.holder, subject, matcher, &args, not);
    out.push(Report::fixed(
        node_span(call),
        format!("Use `{matcher}()` instead of checking `{function}()`."),
        Safety::Unsafe,
        edits,
    ));
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Trinary {
    Yes,
    Maybe,
    No,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Is {
    String,
    Integer,
    Float,
    Array,
    Object,
    Iterable,
    Boolean,
}

pub(crate) fn type_is(ctx: &Context<'_>, expr: &Expression<'_>, kind: Is) -> Trinary {
    let facts = facts(ctx);
    let Some(union) = facts.full_type(node_span(unparenthesize(expr))) else {
        return Trinary::Maybe;
    };
    (union
        .types
        .iter()
        .map(|t| atomic_is(&facts.codebase, t, kind)))
    .reduce(|a, b| if a == b { a } else { Trinary::Maybe })
    .unwrap_or(Trinary::Maybe)
}

fn atomic_is(codebase: &CodebaseMetadata, atomic: &TAtomic, kind: Is) -> Trinary {
    use Trinary::{Maybe, No, Yes};
    match (atomic, kind) {
        (TAtomic::Mixed(_), _) => Maybe,
        (TAtomic::Scalar(scalar), _) => match (scalar, kind) {
            (TScalar::String(_) | TScalar::ClassLikeString(_), Is::String) => Yes,
            (TScalar::Integer(_), Is::Integer) => Yes,
            (TScalar::Float(_), Is::Float) => Yes,
            (TScalar::Bool(_), Is::Boolean) => Yes,
            (TScalar::Generic, Is::String | Is::Integer | Is::Float | Is::Boolean) => Maybe,
            (TScalar::Numeric, Is::String | Is::Integer | Is::Float) => Maybe,
            (TScalar::ArrayKey, Is::String | Is::Integer) => Maybe,
            _ => No,
        },
        (TAtomic::Array(_), Is::Array | Is::Iterable) => Yes,
        (TAtomic::Iterable(_), Is::Iterable) => Yes,
        (TAtomic::Iterable(_), Is::Array | Is::Object) => Maybe,
        (TAtomic::Object(_), Is::Object) => Yes,
        (TAtomic::Object(TObject::Named(named)), Is::Iterable) => {
            match codebase.get_class_like(named.name.as_bytes()) {
                None => Maybe,
                Some(class)
                    if class.name.as_bytes().eq_ignore_ascii_case(b"Traversable")
                        || class
                            .all_parent_interfaces
                            .iter()
                            .any(|i| i.as_bytes().eq_ignore_ascii_case(b"Traversable")) =>
                {
                    Yes
                }
                Some(class) if class.kind == SymbolKind::Interface => Maybe,
                Some(_) => No,
            }
        }
        (TAtomic::Object(_), Is::Iterable) => Maybe,
        (TAtomic::Callable(_), Is::String | Is::Array | Is::Object) => Maybe,
        (TAtomic::Null | TAtomic::Void | TAtomic::Never | TAtomic::Resource(_), _) => No,
        (
            TAtomic::Array(_) | TAtomic::Iterable(_) | TAtomic::Object(_) | TAtomic::Callable(_),
            _,
        ) => No,
        _ => Maybe,
    }
}

const CLOSURE_FUNCTIONS: [(&str, usize); 7] = [
    ("it", 1),
    ("test", 1),
    ("describe", 1),
    ("beforeEach", 0),
    ("afterEach", 0),
    ("beforeAll", 0),
    ("afterAll", 0),
];

#[derive(Clone, Copy)]
pub(crate) enum Callback<'a> {
    Closure(&'a Closure<'a>),
    Arrow(&'a ArrowFunction<'a>),
}

impl<'a> Callback<'a> {
    fn node(self) -> Node<'a, 'a> {
        match self {
            Callback::Closure(c) => Node::Closure(c),
            Callback::Arrow(a) => Node::ArrowFunction(a),
        }
    }
}

pub(crate) fn pest_callback<'a>(
    call: &'a FunctionCall<'a>,
) -> Option<(Callback<'a>, Keyword<'a>, Option<Keyword<'a>>)> {
    let name = function_name(call)?;
    let (_, index) = CLOSURE_FUNCTIONS
        .iter()
        .find(|(f, _)| f.as_bytes() == name)?;
    let value = call.argument_list.arguments.get(*index)?.value();
    match unparenthesize(value) {
        Expression::Closure(c) => Some((Callback::Closure(c), c.function, c.r#static)),
        Expression::ArrowFunction(a) => Some((Callback::Arrow(a), a.r#fn, a.r#static)),
        _ => None,
    }
}

pub(crate) fn requires_instance_binding(callback: Callback<'_>) -> bool {
    let mut found = false;
    callback
        .node()
        .visit_children(|child| found = found || uses_this(child));
    found
}

fn uses_this(node: Node<'_, '_>) -> bool {
    match node {
        Node::DirectVariable(v) => v.name == b"$this",
        Node::FunctionCall(call) => {
            let callback = pest_callback(call).map(|(c, ..)| node_span(&c.node()));
            call.argument_list.arguments.iter().any(|a| {
                let value = a.value();
                Some(node_span(unparenthesize(value))) != callback
                    && uses_this(Node::Expression(value))
            })
        }
        Node::Closure(c) => c.r#static.is_none() && requires_instance_binding(Callback::Closure(c)),
        Node::ArrowFunction(a) => requires_instance_binding(Callback::Arrow(a)),
        _ => {
            let mut found = false;
            node.visit_children(|child| found = found || uses_this(child));
            found
        }
    }
}

pub(crate) fn has_comments(ctx: &Context<'_>, start: usize) -> bool {
    crate::rules::support::statements::tokens(ctx)
        .iter()
        .rev()
        .find(|t| t.end <= start && t.kind != mago_syntax::token::TokenKind::Whitespace)
        .is_some_and(|t| t.kind.is_comment())
}

pub(crate) fn root_func_call<'a>(call: &'a MethodCall<'a>) -> Option<&'a FunctionCall<'a>> {
    let mut current: &Expression<'_> = call.object;
    while let Some(m) = method_call(current) {
        current = m.object;
    }
    while let Some(p) = property_fetch(current) {
        current = p.object;
    }
    func_call(current)
}

pub(crate) fn shift(ctx: &Context<'_>, span: Span, delta: isize) -> String {
    let text = &ctx.source[span.start..span.end];
    if delta == 0 || !text.contains('\n') {
        return text.to_string();
    }
    let (from, to) = if delta < 0 {
        (
            format!("\n{}", " ".repeat(delta.unsigned_abs())),
            "\n".to_string(),
        )
    } else {
        (
            "\n".to_string(),
            format!("\n{}", " ".repeat(delta as usize)),
        )
    };
    let mut out = String::with_capacity(text.len());
    let mut at = span.start;
    for &t in crate::rules::support::statements::tokens(ctx) {
        use mago_syntax::token::TokenKind as K;
        if t.end <= span.start || t.start >= span.end {
            continue;
        }
        let end = t.end.min(span.end);
        let token = &ctx.source[at..end];
        at = end;
        if matches!(
            t.kind,
            K::LiteralString | K::PartialLiteralString | K::StringPart
        ) {
            out.push_str(token);
        } else {
            out.push_str(&token.replace(&from, &to));
        }
    }
    out
}

pub(crate) fn indent_at(ctx: &Context<'_>, pos: usize) -> usize {
    crate::rules::support::statements::line_indent(ctx.source, pos).len()
}

pub(crate) fn kept_text(ctx: &Context<'_>, span: Span, indent: usize) -> String {
    shift(
        ctx,
        span,
        indent as isize - indent_at(ctx, span.start) as isize,
    )
}

pub(crate) fn comment_spans(ctx: &Context<'_>, start: usize) -> Vec<Span> {
    let tokens = crate::rules::support::statements::tokens(ctx);
    let before = tokens
        .iter()
        .rev()
        .find(|t| t.end <= start && !t.kind.is_trivia())
        .map_or(0, |t| t.end);
    tokens
        .iter()
        .filter(|t| t.start >= before && t.end <= start && t.kind.is_comment())
        .map(|t| Span::new(t.start, t.end))
        .collect()
}

fn reformat_comment(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let text = text.trim_end_matches('\n');
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() == 1 {
        return text.to_string();
    }
    if lines[1..].iter().all(|l| {
        let t = l.trim_start();
        t.starts_with('*') && t.len() < l.len()
    }) {
        return std::iter::once(lines[0].to_string())
            .chain(lines[1..].iter().map(|l| format!(" {}", l.trim_start())))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let last = lines[lines.len() - 1];
    if (lines[0].trim_end() == "/*" || lines[0].trim_end() == "/**") && last.trim_start() == "*/" {
        let blanks = &last[..last.len() - last.trim_start().len()];
        return lines
            .iter()
            .map(|l| l.strip_prefix(blanks).unwrap_or(l))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let ws = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c');
    let rest = &text[if text.starts_with("/**") { 3 } else { 2 }..];
    let opener = text.len() - rest.trim_start_matches(ws).len();
    let shortest = (lines[1..].iter())
        .map(|l| l.len() - l.trim_start_matches(ws).len())
        .min()
        .unwrap_or(0);
    match shortest.checked_sub(opener).filter(|n| *n > 0) {
        Some(remove) => std::iter::once(lines[0])
            .chain(lines[1..].iter().map(|l| &l[remove..]))
            .collect::<Vec<_>>()
            .join("\n"),
        None => text.to_string(),
    }
}

pub(crate) fn p_comments(ctx: &Context<'_>, comments: &[Span], indent: usize) -> String {
    let nl = format!("\n{}", " ".repeat(indent));
    comments
        .iter()
        .map(|c| reformat_comment(&ctx.source[c.start..c.end]).replace('\n', &nl))
        .collect::<Vec<_>>()
        .join(&nl)
}

pub(crate) fn print_args(ctx: &Context<'_>, list: &ArgumentList<'_>, indent: usize) -> String {
    let spans: Vec<Span> = list.arguments.iter().map(node_span).collect();
    print_nodes(ctx, &spans, false, indent)
}

pub(crate) fn print_nodes(
    ctx: &Context<'_>,
    spans: &[Span],
    trailing_comma: bool,
    indent: usize,
) -> String {
    if !spans.iter().any(|s| has_comments(ctx, s.start)) {
        return spans
            .iter()
            .map(|s| kept_text(ctx, *s, indent))
            .collect::<Vec<_>>()
            .join(", ");
    }
    let inner = indent + 4;
    let nl = format!("\n{}", " ".repeat(inner));
    let mut out = String::new();
    for (i, span) in spans.iter().enumerate() {
        if has_comments(ctx, span.start) {
            out.push_str(&nl);
            out.push_str(&p_comments(ctx, &comment_spans(ctx, span.start), inner));
        }
        out.push_str(&nl);
        out.push_str(&kept_text(ctx, *span, inner));
        if trailing_comma || i + 1 < spans.len() {
            out.push(',');
        }
    }
    out.push('\n');
    out.push_str(&" ".repeat(indent));
    out
}

pub(crate) fn print_member(
    ctx: &Context<'_>,
    selector: &ClassLikeMemberSelector<'_>,
    indent: usize,
) -> String {
    match selector {
        ClassLikeMemberSelector::Expression(e) => {
            format!("{{{}}}", kept_text(ctx, node_span(e.expression), indent))
        }
        ClassLikeMemberSelector::Variable(v) => format!("{{{}}}", text(ctx, v)),
        _ => text(ctx, selector).to_string(),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Link<'a> {
    Method(&'a MethodCall<'a>),
    Property(&'a PropertyAccess<'a>),
}

pub(crate) fn chain_links<'a>(call: &'a MethodCall<'a>) -> Vec<Link<'a>> {
    let mut links = vec![Link::Method(call)];
    let mut next = call.object;
    loop {
        match unparenthesize(next) {
            Expression::Call(Call::Method(m)) => {
                links.push(Link::Method(m));
                next = m.object;
            }
            Expression::Access(Access::Property(p)) => {
                links.push(Link::Property(p));
                next = p.object;
            }
            _ => break,
        }
    }
    links.reverse();
    links
}

pub(crate) fn print_links(ctx: &Context<'_>, links: &[Link<'_>], indent: usize) -> String {
    links
        .iter()
        .map(|link| match link {
            Link::Method(m) => format!(
                "->{}({})",
                print_member(ctx, &m.method, indent),
                print_args(ctx, &m.argument_list, indent)
            ),
            Link::Property(p) => format!("->{}", print_member(ctx, &p.property, indent)),
        })
        .collect()
}

pub(crate) fn comments_start(ctx: &Context<'_>, start: usize) -> usize {
    let tokens = crate::rules::support::statements::tokens(ctx);
    let before = tokens
        .iter()
        .rev()
        .find(|t| t.end <= start && !t.kind.is_trivia())
        .map_or(0, |t| t.end);
    tokens
        .iter()
        .find(|t| t.start >= before && t.end <= start && t.kind.is_comment())
        .map_or(start, |t| t.start)
}

pub(crate) enum Item<'a> {
    Old(usize),
    Changed(usize, String),
    New {
        print: Box<dyn Fn(usize) -> String + 'a>,
        comments: Vec<Span>,
    },
}

impl Item<'_> {
    fn old(&self) -> Option<usize> {
        match self {
            Item::Old(i) | Item::Changed(i, _) => Some(*i),
            Item::New { .. } => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Diff {
    Keep(usize, usize),
    Remove(usize),
    Add(usize),
    Replace(usize, usize),
}

fn diff(n: usize, new: &[Item<'_>]) -> Vec<Diff> {
    let m = new.len();
    let equal = |x: usize, y: usize| new[y].old() == Some(x);
    let offset = (n + m + 1) as isize;
    let at = |k: isize| (k + offset) as usize;
    let mut v = vec![0isize; 2 * (n + m) + 3];
    v[at(1)] = 0;
    let mut trace = Vec::new();
    let (mut x, mut y) = (0isize, 0isize);
    'outer: for d in 0..=(n + m) as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            x = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
                v[at(k + 1)]
            } else {
                v[at(k - 1)] + 1
            };
            y = x - k;
            while (x as usize) < n && (y as usize) < m && equal(x as usize, y as usize) {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x as usize >= n && y as usize >= m {
                break 'outer;
            }
            k += 2;
        }
    }
    let mut result = Vec::new();
    for d in (0..trace.len() as isize).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[at(prev_k)];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            result.push(Diff::Keep(x as usize - 1, y as usize - 1));
            x -= 1;
            y -= 1;
        }
        if d == 0 {
            break;
        }
        while x > prev_x {
            result.push(Diff::Remove(x as usize - 1));
            x -= 1;
        }
        while y > prev_y {
            result.push(Diff::Add(y as usize - 1));
            y -= 1;
        }
    }
    result.reverse();
    let mut out = Vec::new();
    let mut i = 0;
    while i < result.len() {
        if !matches!(result[i], Diff::Remove(_)) {
            out.push(result[i]);
            i += 1;
            continue;
        }
        let j = i + result[i..]
            .iter()
            .take_while(|d| matches!(d, Diff::Remove(_)))
            .count();
        let k = j + result[j..]
            .iter()
            .take_while(|d| matches!(d, Diff::Add(_)))
            .count();
        if j - i == k - j {
            for n in 0..j - i {
                if let (Diff::Remove(o), Diff::Add(a)) = (result[i + n], result[j + n]) {
                    out.push(Diff::Replace(o, a));
                }
            }
        } else {
            out.extend_from_slice(&result[i..k]);
        }
        i = k;
    }
    out
}

#[derive(Clone, Copy)]
pub(crate) struct OldStmt {
    pub comments: usize,
    pub start: usize,
    pub end: usize,
    pub nop: bool,
}

pub(crate) fn old_statements(
    ctx: &Context<'_>,
    list: &crate::rules::support::statements::List<'_, '_>,
) -> Vec<OldStmt> {
    let mut olds: Vec<OldStmt> = list
        .stmts
        .iter()
        .map(|s| {
            let span = node_span(*s);
            OldStmt {
                comments: comments_start(ctx, span.start),
                start: span.start,
                end: span.end,
                nop: false,
            }
        })
        .collect();
    if list.nop {
        let after = olds.last().map_or(list.inner.start, |s| s.end);
        let tokens = crate::rules::support::statements::tokens(ctx);
        let comments: Vec<_> = tokens
            .iter()
            .filter(|t| t.start >= after && t.end <= list.inner.end && t.kind.is_comment())
            .collect();
        let (start, end) = (comments[0].start, comments[comments.len() - 1].end);
        olds.push(OldStmt {
            comments: start,
            start,
            end,
            nop: true,
        });
    }
    olds
}

fn old_comments(ctx: &Context<'_>, old: &OldStmt) -> Vec<Span> {
    let end = if old.nop { old.end } else { old.start };
    crate::rules::support::statements::tokens(ctx)
        .iter()
        .filter(|t| t.start >= old.comments && t.end <= end && t.kind.is_comment())
        .map(|t| Span::new(t.start, t.end))
        .collect()
}

pub(crate) fn print_list(
    ctx: &Context<'_>,
    olds: &[OldStmt],
    braces: bool,
    mut pos: usize,
    items: &[Item<'_>],
) -> Option<(String, usize)> {
    let tokens = crate::rules::support::statements::tokens(ctx);
    let tag_in = |from: usize, to: usize| {
        use mago_syntax::token::TokenKind as K;
        tokens.iter().any(|t| {
            t.start >= from
                && t.end <= to
                && matches!(
                    t.kind,
                    K::OpenTag | K::CloseTag | K::EchoTag | K::ShortOpenTag
                )
        })
    };
    if olds.len() == 1 && items.len() != 1 && !braces {
        return None;
    }
    let mut result = String::new();
    let mut before_first_keep = true;
    let mut skip_removed = false;
    let mut delayed: Vec<&Item<'_>> = Vec::new();
    let mut last_indent = 0;
    let print_new = |item: &Item<'_>, indent: usize| match item {
        Item::Old(i) => kept_text(ctx, Span::new(olds[*i].start, olds[*i].end), indent),
        Item::Changed(i, text) => shift_text(
            text,
            indent as isize - indent_at(ctx, olds[*i].start) as isize,
        ),
        Item::New { print, .. } => print(indent),
    };
    let comments_of = |item: &Item<'_>| match item {
        Item::New { comments, .. } => comments.clone(),
        Item::Old(i) | Item::Changed(i, _) => old_comments(ctx, &olds[*i]),
    };
    let new_comments = |item: &Item<'_>, indent: usize| {
        let comments = comments_of(item);
        (!comments.is_empty()).then(|| p_comments(ctx, &comments, indent))
    };
    for (i, d) in diff(olds.len(), items).into_iter().enumerate() {
        match d {
            Diff::Keep(o, n) | Diff::Replace(o, n) => {
                before_first_keep = false;
                let old = olds[o];
                last_indent = indent_at(ctx, old.start);
                let comment_start = old.comments;
                if skip_removed {
                    if tag_in(pos, old.start) {
                        return None;
                    }
                } else {
                    result.push_str(&ctx.source[pos..comment_start]);
                }
                let nl = format!("\n{}", " ".repeat(last_indent));
                for item in delayed.drain(..) {
                    if let Some(c) = new_comments(item, last_indent) {
                        result.push_str(&c);
                        result.push_str(&nl);
                    }
                    result.push_str(&print_new(item, last_indent));
                    result.push_str(&nl);
                }
                if matches!(d, Diff::Keep(..))
                    || (comments_of(&items[n]).is_empty() && old.comments == old.start)
                {
                    result.push_str(&ctx.source[comment_start..old.start]);
                } else if let Some(c) = new_comments(&items[n], last_indent) {
                    result.push_str(&c);
                    result.push_str(&nl);
                }
                skip_removed = false;
                result.push_str(&print_new(&items[n], last_indent));
                pos = old.end;
            }
            Diff::Add(n) => {
                if before_first_keep {
                    delayed.push(&items[n]);
                    continue;
                }
                let nl = format!("\n{}", " ".repeat(last_indent));
                result.push_str(&nl);
                if let Some(c) = new_comments(&items[n], last_indent) {
                    result.push_str(&c);
                    result.push_str(&nl);
                }
                result.push_str(&print_new(&items[n], last_indent));
            }
            Diff::Remove(o) => {
                let old = olds[o];
                if i == 0 {
                    result.push_str(&ctx.source[pos..old.comments]);
                    skip_removed = true;
                } else if tag_in(pos, old.comments) {
                    return None;
                }
                pos = old.end;
            }
        }
    }
    if skip_removed || !delayed.is_empty() {
        return None;
    }
    if items.iter().any(|i| i.old().is_some_and(|o| olds[o].nop)) {
        result = result
            .split('\n')
            .map(|l| {
                if l.trim_matches([' ', '\t']).is_empty() {
                    ""
                } else {
                    l
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    Some((result, pos))
}

pub(crate) fn p_stmts(
    ctx: &Context<'_>,
    olds: &[OldStmt],
    items: &[Item<'_>],
    indent: usize,
) -> String {
    let nl = format!("\n{}", " ".repeat(indent));
    let mut out = String::new();
    for item in items {
        let comments = match item {
            Item::New { comments, .. } => comments.clone(),
            Item::Old(i) | Item::Changed(i, _) => old_comments(ctx, &olds[*i]),
        };
        if !comments.is_empty() {
            out.push_str(&nl);
            out.push_str(&p_comments(ctx, &comments, indent));
        }
        match item {
            Item::Old(i) if olds[*i].nop => {}
            Item::Old(i) => {
                out.push_str(&nl);
                out.push_str(&kept_text(
                    ctx,
                    Span::new(olds[*i].start, olds[*i].end),
                    indent,
                ));
            }
            Item::Changed(i, text) => {
                let delta = indent as isize - indent_at(ctx, olds[*i].start) as isize;
                out.push_str(&nl);
                out.push_str(&shift_text(text, delta));
            }
            Item::New { print, .. } => {
                out.push_str(&nl);
                out.push_str(&print(indent));
            }
        }
    }
    out
}

fn shift_text(text: &str, delta: isize) -> String {
    match delta {
        0 => text.to_string(),
        d if d < 0 => text.replace(&format!("\n{}", " ".repeat(d.unsigned_abs())), "\n"),
        d => text.replace('\n', &format!("\n{}", " ".repeat(d as usize))),
    }
}

pub(crate) fn rewrite_list(
    ctx: &Context<'_>,
    holder: Node<'_, '_>,
    ancestors: &[Node<'_, '_>],
    list: &crate::rules::support::statements::List<'_, '_>,
    items: &[Item<'_>],
) -> Option<Vec<Edit>> {
    use crate::rules::support::statements::Frame;
    let parent = ancestors.last().copied();
    let grandparent = ancestors.len().checked_sub(2).map(|i| ancestors[i]);
    let olds = old_statements(ctx, list);
    let braces = matches!(list.frame, Frame::Braces { .. });
    let program = matches!(holder, Node::Program(_));
    let count_changed = program
        && olds.iter().filter(|o| !o.nop).count()
            != items
                .iter()
                .filter(|i| i.old().is_none_or(|o| !olds[o].nop))
                .count();
    if let Some((mut text, mut end)) = print_list(ctx, &olds, braces, list.inner.start, items) {
        if count_changed && !ctx.source.ends_with('\n') {
            text.push_str(&ctx.source[end..]);
            text.push('\n');
            end = ctx.source.len();
        }
        return Some(vec![Edit::new(Span::new(list.inner.start, end), text)]);
    }
    let parent = match parent {
        Some(Node::MethodBody(_)) => grandparent,
        p => p,
    };
    match (holder, parent) {
        (Node::Block(_), Some(Node::Closure(closure))) => {
            let span = node_span(closure);
            let indent = indent_at(ctx, span.start);
            let mut out = String::new();
            for attributes in &closure.attribute_lists {
                out.push_str(&kept_text(ctx, node_span(attributes), indent));
                out.push(' ');
            }
            if closure.r#static.is_some() {
                out.push_str("static ");
            }
            out.push_str("function ");
            if closure.ampersand.is_some() {
                out.push('&');
            }
            out.push('(');
            out.push_str(&print_params(ctx, &closure.parameter_list, indent));
            out.push(')');
            if let Some(uses) = &closure.use_clause {
                let vars: Vec<&str> = uses.variables.iter().map(|v| text(ctx, v)).collect();
                let _ = write!(out, " use ({})", vars.join(", "));
            }
            if let Some(hint) = &closure.return_type_hint {
                out.push_str(": ");
                out.push_str(&kept_text(ctx, node_span(&hint.hint), indent));
            }
            out.push_str(" {");
            out.push_str(&p_stmts(ctx, &olds, items, indent + 4));
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            out.push('}');
            Some(vec![Edit::new(span, out)])
        }
        (Node::Block(_), Some(Node::Function(function))) => {
            let span = node_span(function);
            let indent = indent_at(ctx, span.start);
            let head = format!(
                "function {}{}",
                if function.ampersand.is_some() {
                    "&"
                } else {
                    ""
                },
                text(ctx, &function.name)
            );
            let out = print_function(
                ctx,
                &olds,
                items,
                indent,
                (&function.attribute_lists, head),
                &function.parameter_list,
                function.return_type_hint.as_ref(),
            );
            Some(vec![Edit::new(span, out)])
        }
        (Node::Block(_), Some(Node::Method(method))) => {
            use mago_syntax::cst::Modifier as M;
            let span = node_span(method);
            let indent = indent_at(ctx, span.start);
            let has = |f: fn(&M<'_>) -> bool| method.modifiers.iter().any(f);
            let mut head = String::new();
            for (present, word) in [
                (has(|m| matches!(m, M::Final(_))), "final "),
                (has(|m| matches!(m, M::Abstract(_))), "abstract "),
                (has(|m| matches!(m, M::Public(_))), "public "),
                (has(|m| matches!(m, M::Protected(_))), "protected "),
                (has(|m| matches!(m, M::Private(_))), "private "),
                (has(|m| matches!(m, M::Static(_))), "static "),
            ] {
                if present {
                    head.push_str(word);
                }
            }
            head.push_str("function ");
            if method.ampersand.is_some() {
                head.push('&');
            }
            head.push_str(text(ctx, &method.name));
            let out = print_function(
                ctx,
                &olds,
                items,
                indent,
                (&method.attribute_lists, head),
                &method.parameter_list,
                method.return_type_hint.as_ref(),
            );
            Some(vec![Edit::new(span, out)])
        }
        (Node::Program(_), _) => {
            use mago_syntax::token::TokenKind as K;
            let tokens = crate::rules::support::statements::tokens(ctx);
            tokens
                .first()
                .filter(|t| t.kind == K::OpenTag && t.start == 0)?;
            let tag = tokens.iter().skip(1).find(|t| {
                matches!(
                    t.kind,
                    K::CloseTag | K::InlineText | K::OpenTag | K::EchoTag | K::ShortOpenTag
                )
            });
            tag.is_none_or(|t| t.kind == K::CloseTag && matches!(&ctx.source[t.end..], "" | "\n"))
                .then_some(())?;
            let mut out = format!("<?php\n{}", p_stmts(ctx, &olds, items, 0));
            if count_changed && !out.ends_with('\n') {
                out.push('\n');
            }
            Some(vec![Edit::new(Span::new(0, ctx.source.len()), out)])
        }
        _ => None,
    }
}

fn print_params(
    ctx: &Context<'_>,
    list: &mago_syntax::cst::FunctionLikeParameterList<'_>,
    indent: usize,
) -> String {
    let params: Vec<Span> = list.parameters.iter().map(node_span).collect();
    if !params.iter().any(|p| has_comments(ctx, p.start)) {
        return params
            .iter()
            .map(|p| kept_text(ctx, *p, indent))
            .collect::<Vec<_>>()
            .join(", ");
    }
    let inner = indent + 4;
    let nl = format!("\n{}", " ".repeat(inner));
    let mut out = String::new();
    for p in &params {
        if has_comments(ctx, p.start) {
            out.push_str(&nl);
            out.push_str(&p_comments(ctx, &comment_spans(ctx, p.start), inner));
        }
        out.push_str(&nl);
        out.push_str(&kept_text(ctx, *p, inner));
        if !std::ptr::eq(p, params.last().unwrap()) {
            out.push(',');
        }
    }
    out.push('\n');
    out.push_str(&" ".repeat(indent));
    out
}

pub(crate) const STATEMENT_LISTS: &[NodeKind] = &[
    NodeKind::Program,
    NodeKind::Block,
    NodeKind::NamespaceImplicitBody,
    NodeKind::SwitchExpressionCase,
    NodeKind::SwitchDefaultCase,
    NodeKind::IfColonDelimitedBody,
    NodeKind::IfColonDelimitedBodyElseIfClause,
    NodeKind::IfColonDelimitedBodyElseClause,
    NodeKind::WhileColonDelimitedBody,
    NodeKind::ForColonDelimitedBody,
    NodeKind::ForeachColonDelimitedBody,
    NodeKind::DeclareColonDelimitedBody,
];

pub(crate) fn statement_list<'ast, 'arena>(
    ctx: &Context<'_>,
    node: Node<'ast, 'arena>,
) -> Option<crate::rules::support::statements::List<'ast, 'arena>> {
    if !STATEMENT_LISTS.contains(&node.kind()) {
        return None;
    }
    let tokens = crate::rules::support::statements::tokens(ctx);
    let mut list = crate::rules::support::statements::holder_list(ctx.source, tokens, node);
    list.stmts.retain(|s| match s {
        Statement::Inline(inline) => {
            !(matches!(inline.value, b"\n" | b"\r\n")
                && ctx.source[..node_span(inline).start].ends_with("?>"))
        }
        _ => true,
    });
    Some(list)
}

pub(crate) fn statement_expression<'a>(statement: &'a Statement<'a>) -> Option<&'a Expression<'a>> {
    match statement {
        Statement::Expression(s) => Some(s.expression),
        _ => None,
    }
}

fn print_function(
    ctx: &Context<'_>,
    olds: &[OldStmt],
    items: &[Item<'_>],
    indent: usize,
    (attributes, head): (
        &mago_syntax::cst::Sequence<'_, mago_syntax::cst::AttributeList<'_>>,
        String,
    ),
    params: &mago_syntax::cst::FunctionLikeParameterList<'_>,
    return_type: Option<&mago_syntax::cst::FunctionLikeReturnTypeHint<'_>>,
) -> String {
    let nl = format!("\n{}", " ".repeat(indent));
    let mut out = String::new();
    for list in attributes {
        out.push_str(&kept_text(ctx, node_span(list), indent));
        out.push_str(&nl);
    }
    out.push_str(&head);
    out.push('(');
    out.push_str(&print_params(ctx, params, indent));
    out.push(')');
    if let Some(hint) = return_type {
        out.push_str(": ");
        out.push_str(&kept_text(ctx, node_span(&hint.hint), indent));
    }
    out.push_str(&nl);
    out.push('{');
    out.push_str(&p_stmts(ctx, olds, items, indent + 4));
    out.push_str(&nl);
    out.push('}');
    out
}
