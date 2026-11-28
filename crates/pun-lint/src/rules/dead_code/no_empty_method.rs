use crate::problem::{Edit, Fix, Safety, Span};
use mago_syntax::cst::*;

use crate::rules::support::class_members::remove_members;
use crate::rules::support::class_members::{
    ClassInfo, doc_tags, has_parent_method_or_interface_method, is_class_method_used,
    is_controller_class_method, is_final, is_magic, is_named_constructor, is_private, is_public,
    member_doc, with_class_info,
};
use crate::rules::support::names::has_attribute;
use crate::rules::support::statement_edits::line_indent;
use crate::rules::types::facts::facts;
use crate::rules::types::statement_count;
use crate::syntax::node_span;
use crate::{Context, Report, Rule};

pub struct NoEmptyMethod;

impl Rule for NoEmptyMethod {
    fn id(&self) -> &'static str {
        "lint/dead-code/no-empty-method"
    }

    fn targets(&self) -> Option<&'static [NodeKind]> {
        Some(&[NodeKind::Class, NodeKind::AnonymousClass])
    }

    fn visit<'ast, 'arena>(
        &self,
        node: Node<'ast, 'arena>,
        ancestors: &[Node<'ast, 'arena>],
        ctx: &Context<'_>,
        out: &mut Vec<Report>,
    ) {
        let facts = facts(ctx);
        with_class_info(node, ancestors, &facts, |class| {
            let mut removed = vec![false; class.members.len()];
            let mut methods: Vec<&Method<'_>> = Vec::new();
            for (index, member) in class.members.iter().enumerate() {
                let ClassLikeMember::Method(method) = member else {
                    continue;
                };
                let MethodBody::Concrete(body) = &method.body else {
                    continue;
                };
                if statement_count(ctx.source, body) > 0
                    || method
                        .modifiers
                        .iter()
                        .any(|m| matches!(m, Modifier::Abstract(_)))
                {
                    continue;
                }
                if is_final(method) && !class.is_final {
                    continue;
                }
                if !method.attribute_lists.is_empty() {
                    continue;
                }
                // A child class can call parent::__construct(), so a non-private constructor stays
                // in a named class that isn't final (Rector 2.9.2). An anonymous class keeps its
                // empty public methods, as in Rector 2.8.4: a Laravel migration's empty down() is
                // the framework's convention (shipfastlabs/pun#189).
                let constructor = method.name.value.eq_ignore_ascii_case(b"__construct");
                if !class.is_final
                    && (!is_magic(method.name.value) || (constructor && !class.is_anonymous))
                    && !is_private(method)
                {
                    continue;
                }
                if should_skip(ctx, ancestors, &facts, &class, method, index) {
                    continue;
                }
                removed[index] = true;
                methods.push(method);
            }
            if !removed.contains(&true) {
                return;
            }
            let edits = if removed.iter().all(|r| *r) && class.is_anonymous {
                let open = class.left_brace.start.offset as usize;
                let from = ctx.source[..open].trim_end().len();
                let indent = line_indent(ctx.source, node_span(&node).start);
                vec![Edit::new(
                    Span::new(from, class.right_brace.end.offset as usize),
                    format!("\n{indent}{{\n{indent}}}"),
                )]
            } else {
                remove_members(
                    ctx.source,
                    class.members,
                    class.left_brace,
                    class.right_brace,
                    &removed,
                )
            };
            for method in methods {
                out.push(Report {
                    span: node_span(method),
                    message: format!(
                        "Remove the empty method `{}()`, which nothing requires.",
                        String::from_utf8_lossy(method.name.value)
                    ),
                    help: None,
                    fix: Some(Fix {
                        safety: Safety::Safe,
                        edits: edits.clone(),
                    }),
                });
            }
        });
    }
}

fn should_skip(
    ctx: &Context<'_>,
    ancestors: &[Node<'_, '_>],
    facts: &crate::rules::types::facts::Facts,
    class: &ClassInfo<'_, '_>,
    method: &Method<'_>,
    index: usize,
) -> bool {
    let name = method.name.value;
    let is = |other: &[u8]| name.eq_ignore_ascii_case(other);
    if is_class_method_used(facts, ancestors, class, name) || is_named_constructor(class, method) {
        return true;
    }
    if class.is_anonymous && class.extends.is_some() && is(b"__construct") {
        return true;
    }
    if has_parent_method_or_interface_method(facts, class, name) {
        return true;
    }
    let doc = member_doc(ctx.source, class, index);
    if method
        .parameter_list
        .parameters
        .iter()
        .any(|p| !p.modifiers.is_empty())
        || doc.is_some_and(|d| doc_tags(d).contains(&"@deprecated"))
        || is_controller_class_method(ctx.source, class, method, index)
    {
        return true;
    }
    if is(b"__clone") {
        return !is_public(method);
    }
    if is(b"__invoke") {
        return true;
    }
    let Node::Class(c) = class.node else {
        return false;
    };
    is(b"__construct") && has_attribute(ancestors, &c.attribute_lists, "Attribute") && doc.is_some()
}
