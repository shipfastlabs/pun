use mago_syntax::cst::{Argument, Expression, Literal, Node, NodeKind};

use super::{Is, Trinary, expect_argument, is_name, matcher_subject, set_call_arguments, type_is};
use crate::problem::Safety;
use crate::rules::support::statements::tokens;
use crate::syntax::{node_span, unparenthesize};
use crate::{Context, Report, Rule};

pub struct PreferSpecificExpectations;

impl Rule for PreferSpecificExpectations {
    fn id(&self) -> &'static str {
        "lint/pest/prefer-specific-expectations"
    }

    fn targets(&self) -> Option<&'static [NodeKind]> {
        Some(&[NodeKind::MethodCall])
    }

    fn visit<'ast, 'arena>(
        &self,
        node: Node<'ast, 'arena>,
        _ancestors: &[Node<'ast, 'arena>],
        ctx: &Context<'_>,
        out: &mut Vec<Report>,
    ) {
        let Node::MethodCall(call) = node else {
            return;
        };
        let strict = is_name(&call.method, "toBe");
        if (!strict && !is_name(&call.method, "toEqual")) || expect_argument(call).is_none() {
            return;
        }
        let value = match call.argument_list.arguments.as_slice() {
            [Argument::Positional(argument)] if argument.ellipsis.is_none() => argument.value,
            [Argument::Named(argument)] if argument.name.value == b"expected" => argument.value,
            _ => return,
        };
        let Some((matcher, required_type)) = matcher(unparenthesize(value), strict) else {
            return;
        };
        let start = node_span(call.object).end;
        let end = node_span(call).end;
        let tokens = tokens(ctx);
        let after_subject = tokens.partition_point(|token| token.start < start);
        if tokens[after_subject..]
            .iter()
            .take_while(|token| token.start < end)
            .any(|token| token.kind.is_comment())
        {
            return;
        }
        if required_type.is_some_and(|kind| {
            matcher_subject(call).is_none_or(|subject| type_is(ctx, subject, kind) != Trinary::Yes)
        }) {
            return;
        }

        out.push(Report::fixed(
            node_span(call),
            format!("Use `{matcher}()` instead of comparing to a literal."),
            if required_type == Some(Is::String) {
                // PHPUnit's assertEmpty() also accepts the string "0".
                Safety::Unsafe
            } else {
                Safety::Safe
            },
            set_call_arguments(call, matcher, &[]),
        ));
    }
}

fn matcher(value: &Expression<'_>, strict: bool) -> Option<(&'static str, Option<Is>)> {
    let boolean = (!strict).then_some(Is::Boolean);
    Some(match value {
        Expression::Literal(Literal::True(_)) => ("toBeTrue", boolean),
        Expression::Literal(Literal::False(_)) => ("toBeFalse", boolean),
        Expression::Literal(Literal::Null(_)) if strict => ("toBeNull", None),
        Expression::ConstantAccess(constant) => {
            let name = constant.name.value().strip_prefix(b"\\")?;
            if name.eq_ignore_ascii_case(b"true") {
                ("toBeTrue", boolean)
            } else if name.eq_ignore_ascii_case(b"false") {
                ("toBeFalse", boolean)
            } else if strict && name.eq_ignore_ascii_case(b"null") {
                ("toBeNull", None)
            } else {
                return None;
            }
        }
        Expression::Array(array) if array.elements.is_empty() => ("toBeEmpty", Some(Is::Array)),
        Expression::LegacyArray(array) if array.elements.is_empty() => {
            ("toBeEmpty", Some(Is::Array))
        }
        Expression::Literal(Literal::String(string)) if string.value == Some(b"") => {
            ("toBeEmpty", Some(Is::String))
        }
        _ => return None,
    })
}
