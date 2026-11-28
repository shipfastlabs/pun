use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_syntax::cst::{Argument, Expression, Node, NodeKind};

use crate::rules::support::names::function_call_matches_any;
use crate::rules::types::facts::facts;
use crate::syntax::node_span;
use crate::{Context, Report, Rule};

const FUNCTIONS: [(&str, &[&str], usize); 4] = [
    ("in_array", &["needle", "haystack", "strict"], 2),
    ("array_search", &["needle", "haystack", "strict"], 2),
    ("base64_decode", &["string", "strict"], 1),
    ("array_keys", &["array", "filter_value", "strict"], 2),
];

pub struct RequireStrictFunctionCall;

impl Rule for RequireStrictFunctionCall {
    fn id(&self) -> &'static str {
        "lint/functions/require-strict-function-call"
    }

    fn targets(&self) -> Option<&'static [NodeKind]> {
        Some(&[NodeKind::FunctionCall])
    }

    fn visit<'ast, 'arena>(
        &self,
        node: Node<'ast, 'arena>,
        ancestors: &[Node<'ast, 'arena>],
        ctx: &Context<'_>,
        out: &mut Vec<Report>,
    ) {
        let Node::FunctionCall(call) = node else {
            return;
        };
        let names = FUNCTIONS.map(|(name, _, _)| name);
        let Some(name) = function_call_matches_any(ancestors, call, &names) else {
            return;
        };
        let (_, params, position) = FUNCTIONS.iter().find(|(n, _, _)| *n == name).unwrap();
        let mut slots: Vec<(usize, &Expression<'_>)> = Vec::new();
        for (index, argument) in call.argument_list.arguments.iter().enumerate() {
            let slot = match argument {
                Argument::Positional(_) => index,
                Argument::Named(named) => {
                    match params.iter().position(|p| p.as_bytes() == named.name.value) {
                        Some(slot) => slot,
                        None => return,
                    }
                }
            };
            slots.push((slot, argument.value()));
        }
        let last = slots.iter().map(|(slot, _)| slot + 1).max().unwrap_or(0);
        if name == "array_keys" && last < 2 {
            return;
        }
        let problem = match slots.iter().find(|(slot, _)| slot == position) {
            None => "set",
            Some((_, value)) => match is_true(ctx, value) {
                Some(false) => "true",
                Some(true) | None => return,
            },
        };
        out.push(Report {
            span: node_span(call),
            message: format!(
                "Call to function {name}() requires parameter #{} to be {problem}.",
                position + 1
            ),
            help: None,
            fix: None,
        });
    }
}

/// Whether the strict flag is `true`, or `None` when the analysis has no type for it: it doesn't type code it never
/// reaches, so the flag can't be told wrong there. PHPStan runs no rule on that code.
fn is_true(ctx: &Context<'_>, value: &Expression<'_>) -> Option<bool> {
    let facts = facts(ctx);
    let t = facts.native_type(node_span(value))?;
    Some(
        t.types
            .iter()
            .all(|a| matches!(a, TAtomic::Scalar(TScalar::Bool(b)) if b.is_true())),
    )
}
