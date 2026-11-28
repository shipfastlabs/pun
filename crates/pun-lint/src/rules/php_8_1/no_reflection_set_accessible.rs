//! Rector 2.8.4's `RemoveReflectionSetAccessibleCallsRector`.

use mago_codex::ttype::atomic::{TAtomic, object::TObject};
use mago_syntax::cst::{
    Argument, Call, ClassLikeMemberSelector, Expression, Literal, MethodCall, Node, NodeKind,
    Statement, Variable,
};

use crate::problem::Safety;
use crate::rules::support::statement_edits::{enclosing, remove};
use crate::rules::support::values::is_object_of;
use crate::rules::types::facts::facts;
use crate::syntax::{node_span, unparenthesize};
use crate::{Context, Report, Rule};

pub struct NoReflectionSetAccessible;

impl Rule for NoReflectionSetAccessible {
    fn id(&self) -> &'static str {
        "lint/php-8.1/no-reflection-set-accessible"
    }

    fn min_php_version(&self) -> crate::PhpVersion {
        crate::PhpVersion::from_id(80100)
    }

    fn targets(&self) -> Option<&'static [NodeKind]> {
        Some(&[NodeKind::Statement])
    }

    fn visit<'ast, 'arena>(
        &self,
        node: Node<'ast, 'arena>,
        ancestors: &[Node<'ast, 'arena>],
        ctx: &Context<'_>,
        out: &mut Vec<Report>,
    ) {
        let Node::Statement(statement @ Statement::Expression(e)) = node else {
            return;
        };
        let Expression::Call(Call::Method(call)) = e.expression else {
            return;
        };
        let ClassLikeMemberSelector::Identifier(name) = &call.method else {
            return;
        };
        if !name.value.eq_ignore_ascii_case(b"setAccessible") {
            return;
        }
        let at = node_span(call).start;
        if !is_object_of(ancestors, call.object, "ReflectionProperty", at)
            && !is_object_of(ancestors, call.object, "ReflectionMethod", at)
            && !is_inferred_reflection(ctx, call)
        {
            return;
        }
        let (list, index) = enclosing(ctx.source, statement, ancestors);
        out.push(Report::fixed(
            node_span(statement),
            "Remove the `setAccessible()` call, which has no effect since PHP 8.1.",
            Safety::Safe,
            remove(ctx, &list, &[index]),
        ));
    }
}

fn is_inferred_reflection(ctx: &Context<'_>, call: &MethodCall<'_>) -> bool {
    // Don't drop receiver evaluation or argument side effects when widening the rule.
    let object = unparenthesize(call.object);
    let [Argument::Positional(argument)] = call.argument_list.arguments.as_slice() else {
        return false;
    };
    if argument.ellipsis.is_some()
        || !matches!(object, Expression::Variable(Variable::Direct(_)))
        || !matches!(
            unparenthesize(argument.value),
            Expression::Literal(Literal::True(_) | Literal::False(_))
        )
    {
        return false;
    }

    let facts = facts(ctx);
    let Some(ty) = facts.native_type(node_span(object)) else {
        return false;
    };
    matches!(ty.types.as_ref(), [TAtomic::Object(TObject::Named(class))]
        if class.name.as_bytes().eq_ignore_ascii_case(b"ReflectionProperty")
            || class.name.as_bytes().eq_ignore_ascii_case(b"ReflectionMethod"))
}
