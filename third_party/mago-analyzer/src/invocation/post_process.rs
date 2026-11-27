use mago_allocator::Arena;
use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use mago_algebra::AssertionMap;
use mago_algebra::assertion_set::AssertionSet;
use mago_algebra::assertion_set::Conjunction;
use mago_algebra::assertion_set::Disjunction;
use mago_algebra::assertion_set::add_and_assertion;
use mago_algebra::assertion_set::add_and_clause;
use mago_algebra::clause::Clause;
use mago_algebra::find_satisfying_assignments;
use mago_algebra::saturate_clauses;
use mago_codex::assertion::Assertion;
use mago_codex::identifier::function_like::FunctionLikeIdentifier;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::function_like::ReflectedSideEffects;
use mago_codex::ttype::TType;
use mago_codex::ttype::add_union_type;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::array::TArray;
use mago_codex::ttype::atomic::array::key::ArrayKey;
use mago_codex::ttype::atomic::array::keyed::TKeyedArray;
use mago_codex::ttype::atomic::array::list::TList;
use mago_codex::ttype::atomic::callable::TCallable;
use mago_codex::ttype::atomic::generic::TGenericParameter;
use mago_codex::ttype::atomic::resource::TResource;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_codex::ttype::atomic::scalar::bool::TBool;
use mago_codex::ttype::combiner::CombinerOptions;
use mago_codex::ttype::comparator::ComparisonResult;
use mago_codex::ttype::comparator::union_comparator::can_expression_types_be_identical;
use mago_codex::ttype::comparator::union_comparator::is_contained_by;
use mago_codex::ttype::expander::TypeExpansionOptions;
use mago_codex::ttype::expander::expand_union;
use mago_codex::ttype::get_array_parameters;
use mago_codex::ttype::get_empty_keyed_array;
use mago_codex::ttype::get_mixed;
use mago_codex::ttype::get_never;
use mago_codex::ttype::template::TemplateResult;
use mago_codex::ttype::template::bounds::get_most_specific_type_from_bounds;
use mago_codex::ttype::template::bounds::get_relevant_bounds;
use mago_codex::ttype::union::TUnion;
use mago_reporting::Annotation;
use mago_reporting::Issue;
use mago_span::HasSpan;
use mago_span::Span;
use mago_syntax::cst::Argument;
use mago_syntax::cst::ArrayAccess;
use mago_syntax::cst::ArrayAppend;
use mago_syntax::cst::BinaryOperator;
use mago_syntax::cst::Expression;
use mago_syntax::cst::Literal;
use mago_word::Word;
use mago_word::WordMap;
use mago_word::WordSet;
use mago_word::concat_word;
use mago_word::word;

use crate::artifacts::AnalysisArtifacts;
use crate::artifacts::BooleanExpressionScope;
use crate::artifacts::BooleanExpressionScopes;
use crate::artifacts::VariableThrowKind;
use crate::code::IssueCode;
use crate::context::Context;
use crate::context::block::BlockContext;
use crate::context::block::PossiblyThrownGenericException;
use crate::context::block::ReferenceConstraint;
use crate::context::block::ReferenceConstraintSource;
use crate::error::AnalysisError;
use crate::expression::assignment::PropertyWriteKind;
use crate::expression::assignment::assign_to_expression;
use crate::formula::get_formula;
use crate::formula::negate_or_synthesize;
use crate::invocation::Invocation;
use crate::invocation::InvocationArgumentsSource;
use crate::invocation::InvocationTarget;
use crate::invocation::MethodInvocationKind;
use crate::invocation::arguments::is_argument_mutated_by_reference;
use crate::invocation::arguments::is_argument_referenceable;
use crate::invocation::arguments::is_array_multisort;
use crate::invocation::resolver::resolve_invocation_type;
use crate::plugin::provider::assertion::InvocationAssertions;
use crate::reconciler::assertion_reconciler::intersect_union_with_union;
use crate::utils::exception::get_throw_type_atomics;
use crate::utils::exception::get_throw_type_object_names;
use crate::utils::exception::is_structural_throw_type;
use crate::utils::expression::expression_is_nullsafe;
use crate::utils::expression::get_access_base_expressions;
use crate::utils::expression::get_block_expression_id;
use crate::utils::expression::get_non_nullsafe_expression_id;
use crate::utils::expression::get_root_expression_id;
use crate::utils::expression::uses_variable_as_offset;
use crate::utils::misc::unwrap_expression;

pub fn post_invocation_process<'ctx, 'arena, A>(
    context: &mut Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &mut AnalysisArtifacts,
    invoication: &Invocation<'ctx, '_, 'arena>,
    this_variable: Option<&[u8]>,
    template_result: &TemplateResult,
    parameters: &WordMap<TUnion>,
    apply_assertions: bool,
) -> Result<(), AnalysisError>
where
    A: Arena,
{
    update_by_reference_argument_types(context, block_context, artifacts, invoication, template_result, parameters, false)?;

    post_invocation_metadata(
        context,
        block_context,
        artifacts,
        invoication,
        this_variable,
        template_result,
        parameters,
        apply_assertions,
    )
}

/// Callee metadata is resolved independently of the source call's argument effects.
pub(crate) fn post_invocation_metadata<'ctx, 'arena, A>(
    context: &mut Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &mut AnalysisArtifacts,
    invoication: &Invocation<'ctx, '_, 'arena>,
    this_variable: Option<&[u8]>,
    template_result: &TemplateResult,
    parameters: &WordMap<TUnion>,
    apply_assertions: bool,
) -> Result<(), AnalysisError>
where
    A: Arena,
{
    clear_object_property_narrowings(context, block_context, invoication, this_variable);

    if block_context.flags.inside_try() && invoication.target.get_function_like_metadata().is_none() {
        artifacts.variable_throw_scopes.entry((invoication.span, VariableThrowKind::Implicit)).or_default()
            .insert(mago_word::word(b"Throwable"), block_context.variable_scope());
    }
    let Some(identifier) = invoication.target.get_function_like_identifier() else {
        return Ok(());
    };

    let Some(metadata) = invoication.target.get_function_like_metadata() else {
        return Ok(());
    };

    // Pun: MethodCallHandler assigns a method's `@phpstan-this-out` type, resolved for the call, to the receiver
    if let Some(this_variable) = this_variable
        && let Some(this_out) = metadata.method_metadata.as_ref().and_then(|method| method.this_out_type.as_ref())
    {
        let resolved = resolve_invocation_type(context, invoication, template_result, parameters, this_out.type_union.clone());
        let variable = mago_word::word(this_variable);
        let resolved = Rc::new(resolved);
        if let Some(existing) = block_context.locals.get(&variable).cloned() {
            block_context.remove_descendants(context, variable, &existing, Some(&resolved));
        }
        block_context.locals.insert(variable, resolved);
    }

    let callable_kind_str = match identifier {
        FunctionLikeIdentifier::Function(_) => "function",
        FunctionLikeIdentifier::Method(_, _) => "method",
        FunctionLikeIdentifier::Closure(_) => "closure",
    };
    let full_callable_name = OnceCell::new();

    if metadata.flags.is_deprecated() && !block_context.is_enclosing_scope_deprecated() {
        let full_callable_name =
            full_callable_name.get_or_init(|| display_callable_name(context, identifier, metadata.original_name));
        let issue_kind = match identifier {
            FunctionLikeIdentifier::Function(_) => IssueCode::DeprecatedFunction,
            FunctionLikeIdentifier::Method(_, _) => IssueCode::DeprecatedMethod,
            FunctionLikeIdentifier::Closure(_) => IssueCode::DeprecatedClosure,
        };

        context.collector.report_with_code(
            issue_kind,
            Issue::warning(format!("Call to deprecated {callable_kind_str}: {full_callable_name}."))
                .with_annotation(
                    Annotation::primary(invoication.target.span()).with_message(format!("This {callable_kind_str} is deprecated")),
                )
                .with_note(format!(
                    "The {callable_kind_str} {full_callable_name} is marked as deprecated and may be removed or its behavior changed in future versions."
                ))
                .with_help(format!(
                    "Consult the documentation for {full_callable_name} for alternatives or migration instructions."
                )),
        );
    }

    // Report if named arguments are used where not allowed
    if metadata.flags.forbids_named_arguments()
        && let InvocationArgumentsSource::ArgumentList(argument_list) = invoication.arguments_source
    {
        for argument in &argument_list.arguments {
            let message = match argument {
                Argument::Named(_) => "Named argument used here",
                Argument::Positional(positional)
                    if positional.ellipsis.is_some()
                        && artifacts
                            .get_expression_type(&positional.value)
                            .is_some_and(|argument_type| argument_type.types.iter().any(array_has_string_keys)) =>
                {
                    "This unpacked array passes its string keys as named arguments"
                }
                _ => continue,
            };
            let full_callable_name =
                full_callable_name.get_or_init(|| display_callable_name(context, identifier, metadata.original_name));

            context.collector.report_with_code(
                IssueCode::NamedArgumentNotAllowed,
                Issue::error(format!("Named arguments are not allowed for {full_callable_name}."))
                    .with_annotation(Annotation::primary(argument.span()).with_message(message))
                    .with_annotation(Annotation::secondary(invoication.target.span()).with_message(format!(
                        "The {callable_kind_str} {full_callable_name} only accepts positional arguments"
                    )))
                    .with_help("Convert this named argument to a positional argument."),
            );
        }
    }

    let records_thrown_exceptions = context.settings.check_throws || context.settings.find_overly_wide_throw_types;
    if records_thrown_exceptions || block_context.flags.inside_try() {
        let external_throw_types = match identifier {
            _ if !records_thrown_exceptions => None,
            FunctionLikeIdentifier::Function(name) => context.plugin_registry.get_external_function_thrown_exceptions(
                context.codebase,
                context.source_file,
                artifacts,
                name.as_bytes(),
                invoication,
                context.external_analysis_session,
            ),
            FunctionLikeIdentifier::Method(class_name, method_name) => {
                context.plugin_registry.get_external_method_thrown_exceptions(
                    context.codebase,
                    context.source_file,
                    artifacts,
                    class_name.as_bytes(),
                    method_name.as_bytes(),
                    invoication,
                    context.external_analysis_session,
                )
            }
            FunctionLikeIdentifier::Closure(_) => None,
        };

        if let Some(exceptions) = external_throw_types {
            for exception in exceptions {
                block_context.possibly_thrown_exceptions.entry(exception).or_default().insert(invoication.span);
                if block_context.flags.inside_try() {
                    artifacts
                        .variable_throw_scopes
                        .entry((invoication.span, VariableThrowKind::Declared))
                        .or_default()
                        .insert(exception, block_context.variable_scope());
                }
            }
        } else {
            let backed_enum_from_argument_is_valid = records_thrown_exceptions
                && backed_enum_from_argument_is_valid(context, block_context, artifacts, invoication, identifier);
            let thrown_types = context.codebase.get_function_like_thrown_types(
                invoication.target.get_method_context().map(|context| context.class_like_metadata),
                metadata,
            );

            // FuncCallHandler/NewHandler's implicit throw point, on the post-argument scope. An exception
            // constructor inside `throw` does not itself add an undocumented-Throwable entry.
            if thrown_types.is_empty() && block_context.flags.inside_try() {
                let exception_constructor = block_context.flags.inside_throw()
                    && matches!(identifier, FunctionLikeIdentifier::Method(class, method)
                        if method.as_bytes().eq_ignore_ascii_case(b"__construct")
                            && context.codebase.is_instance_of(class.as_bytes(), b"Throwable"));
                let no_argument_builtin = matches!(identifier, FunctionLikeIdentifier::Function(_))
                    && metadata.flags.is_built_in() && invoication.arguments_source.is_empty()
                    && metadata.parameters.iter().all(|parameter| parameter.flags.has_default() || parameter.flags.is_variadic());
                if !exception_constructor && !no_argument_builtin {
                    artifacts.variable_throw_scopes.entry((invoication.span, VariableThrowKind::Implicit)).or_default()
                        .insert(mago_word::word(b"Throwable"), block_context.variable_scope());
                }
            }

            let is_recursive_call = block_context.scope.get_function_like().is_some_and(|function_like| {
                function_like.name == metadata.name && function_like.span == metadata.span
            });

            for thrown_exception_type in thrown_types {
                let mut thrown_type_union = thrown_exception_type.type_union.clone();
                let mut unresolved_generic_throw_types = Vec::new();
                if is_recursive_call {
                    let mut resolved_throw_types = Vec::new();
                    for exception_atomic in thrown_type_union.types.iter() {
                        let TAtomic::GenericParameter(parameter) = exception_atomic else {
                            resolved_throw_types.push(exception_atomic.clone());
                            continue;
                        };

                        if !exception_atomic.extends_or_implements(context.codebase, b"Throwable")
                            || has_template_binding_within_constraint(context, parameter, template_result)
                        {
                            resolved_throw_types.push(exception_atomic.clone());
                            continue;
                        }

                        unresolved_generic_throw_types.push(exception_atomic.clone());
                    }

                    thrown_type_union.types = resolved_throw_types.into();
                }

                let mut resolved_exception_types = Vec::new();
                if !thrown_type_union.types.is_empty() {
                    resolved_exception_types.push(resolve_invocation_type(
                        context,
                        invoication,
                        template_result,
                        parameters,
                        thrown_type_union,
                    ));
                }

                for exception_atomic in unresolved_generic_throw_types {
                    for thrown_atomic in get_throw_type_atomics(&exception_atomic) {
                        resolved_exception_types.push(TUnion::from_atomic(thrown_atomic));
                    }
                }

                for resolved_exception_type in resolved_exception_types {
                    for exception_atomic in resolved_exception_type.types.into_owned() {
                        if block_context.flags.inside_try() {
                            for exception in exception_atomic.get_all_object_names() {
                                artifacts
                                    .variable_throw_scopes
                                    .entry((invoication.span, VariableThrowKind::Declared))
                                    .or_default()
                                    .insert(exception, block_context.variable_scope());
                            }
                        }

                        if !records_thrown_exceptions {
                            continue;
                        }

                        for thrown_atomic in get_throw_type_atomics(&exception_atomic) {
                            if is_structural_throw_type(&thrown_atomic)
                                && thrown_atomic.extends_or_implements(context.codebase, b"Throwable")
                            {
                                let object_names = get_throw_type_object_names(context.codebase, &thrown_atomic);
                                let thrown_generic = PossiblyThrownGenericException {
                                    object_names,
                                    atomic_type: thrown_atomic,
                                    span: invoication.span,
                                };
                                block_context.possibly_thrown_generic_exceptions.insert(thrown_generic);
                            } else {
                                for exception in thrown_atomic.get_all_object_names() {
                                    if backed_enum_from_argument_is_valid
                                        && exception.as_bytes().eq_ignore_ascii_case(b"valueerror")
                                    {
                                        continue;
                                    }

                                    block_context
                                        .possibly_thrown_exceptions
                                        .entry(exception)
                                        .or_default()
                                        .insert(invoication.span);
                                }
                            }
                        }
                    }
                }
            }

            if records_thrown_exceptions {
                collect_plugin_throw_types(context, block_context, artifacts, invoication, identifier);
            }
        }

        if records_thrown_exceptions {
            record_immediate_callback_throw_types(context, block_context, artifacts, invoication, identifier);
        }
    }

    if !apply_assertions {
        return Ok(());
    }

    let range = (invoication.span.start.offset, invoication.span.end.offset);

    // PHPStan narrows arguments from declared contracts or extensions, not a callee's body.
    // The scanner/populator distinguish inferred predicates from declared/inherited assertions.
    let empty_assertions = BTreeMap::new();
    let (if_true_assertions, if_false_assertions) = if metadata.assertions_inferred {
        (&empty_assertions, &empty_assertions)
    } else {
        (&metadata.if_true_assertions, &metadata.if_false_assertions)
    };

    let resolved_if_true_assertions = resolve_invocation_assertion(
        context,
        block_context,
        artifacts,
        invoication,
        this_variable,
        if_true_assertions,
        template_result,
        parameters,
        false,
    );

    block_context
        .stable_method_calls
        .extend(resolved_if_true_assertions.keys().filter(|variable| variable.as_bytes().ends_with(b"()")).copied());

    for (variable, assertions) in resolved_if_true_assertions {
        artifacts.if_true_assertions.entry(range).or_default().entry(variable).or_default().extend(assertions);
    }
    for (variable, assertions) in coalesce_argument_assertions(context, block_context, artifacts, invoication, if_true_assertions) {
        artifacts.if_true_assertions.entry(range).or_default().entry(variable).or_insert(assertions);
    }

    let resolved_if_false_assertions = resolve_invocation_assertion(
        context,
        block_context,
        artifacts,
        invoication,
        this_variable,
        if_false_assertions,
        template_result,
        parameters,
        false,
    );

    block_context
        .stable_method_calls
        .extend(resolved_if_false_assertions.keys().filter(|variable| variable.as_bytes().ends_with(b"()")).copied());

    for (variable, assertions) in resolved_if_false_assertions {
        artifacts.if_false_assertions.entry(range).or_default().entry(variable).or_default().extend(assertions);
    }

    record_null_context_assertions(
        context,
        block_context,
        artifacts,
        invoication,
        this_variable,
        &metadata.assertions,
        template_result,
        parameters,
    );

    apply_plugin_assertions(
        context,
        block_context,
        artifacts,
        invoication,
        identifier,
        this_variable,
        template_result,
        parameters,
        range,
    )?;

    Ok(())
}

fn backed_enum_from_argument_is_valid<'ctx, 'arena, A>(
    context: &Context<'ctx, 'arena, A>,
    block_context: &BlockContext<'ctx>,
    artifacts: &AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    identifier: &FunctionLikeIdentifier,
) -> bool
where
    A: Arena,
{
    let FunctionLikeIdentifier::Method(_, method_name) = identifier else {
        return false;
    };

    if !method_name.as_bytes().eq_ignore_ascii_case(b"from") {
        return false;
    }

    let Some(method_context) = invocation.target.get_method_context() else {
        return false;
    };

    if method_context.invocation_kind != MethodInvocationKind::Static {
        return false;
    }

    let enum_metadata = method_context.class_like_metadata;
    if !enum_metadata.kind.is_enum() || enum_metadata.enum_type.is_none() || enum_metadata.enum_cases.is_empty() {
        return false;
    }

    let mut backing_values = Vec::with_capacity(enum_metadata.enum_cases.len());
    for enum_case in enum_metadata.enum_cases.values() {
        let Some(value_type) = enum_case.value_type.as_ref() else {
            return false;
        };

        if !value_type.is_literal()
            || (value_type.get_literal_int_value().is_none() && value_type.get_literal_string_value().is_none())
        {
            return false;
        }

        backing_values.push(value_type.clone());
    }

    let (Some(argument), argument_id) =
        get_argument_for_parameter(context, block_context, invocation, Some(0), Some(word(b"value")))
    else {
        return false;
    };

    let Some(mut argument_type) = artifacts
        .get_expression_type(argument)
        .cloned()
        .or_else(|| argument_id.and_then(|id| block_context.locals.get(&id).map(|ty| ty.as_ref().clone())))
    else {
        return false;
    };

    let mut backing_value_type = TUnion::from_vec(backing_values);
    let expansion_options = TypeExpansionOptions::default();
    expand_union(context.codebase, &mut argument_type, &expansion_options);
    expand_union(context.codebase, &mut backing_value_type, &expansion_options);

    is_contained_by(
        context.codebase,
        &argument_type,
        &backing_value_type,
        false,
        false,
        false,
        &mut ComparisonResult::default(),
    )
}

fn record_immediate_callback_throw_types<'ctx, 'arena, A>(
    context: &Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    identifier: &FunctionLikeIdentifier,
) where
    A: Arena,
{
    let Some(callback_parameter_offset) = immediate_callback_parameter_offset(identifier) else {
        return;
    };

    let (Some(callback_expression), _) =
        get_argument_for_parameter(context, block_context, invocation, Some(callback_parameter_offset), None)
    else {
        return;
    };

    let Some(callback_type) = artifacts.get_expression_type(callback_expression) else {
        return;
    };

    let array_map_callback_will_not_run = context.settings.find_overly_wide_throw_types
        && array_map_callback_is_definitely_not_invoked(artifacts, invocation, identifier);

    if !array_map_callback_will_not_run {
        for callable in callback_type.types.iter().filter_map(|atomic| match atomic {
            TAtomic::Callable(TCallable::Alias(identifier)) => Some(*identifier),
            TAtomic::Callable(TCallable::Signature(signature)) => signature.source,
            _ => None,
        }) {
            record_callback_throw_types(context, block_context, callable, invocation.span);
        }
    }

    if !matches!(identifier, FunctionLikeIdentifier::Function(name) if name.as_bytes() == b"preg_replace_callback_array")
    {
        return;
    }

    for atomic in callback_type.types.iter() {
        let TAtomic::Array(array) = atomic else {
            continue;
        };

        let callback_types = match array {
            TArray::List(list) => {
                let mut unions = vec![list.element_type.as_ref()];
                if let Some(known_elements) = &list.known_elements {
                    unions.extend(known_elements.values().map(|(_, element)| element));
                }
                unions
            }
            TArray::Keyed(keyed) => {
                let mut unions = Vec::new();
                if let Some((_, value_type)) = &keyed.parameters {
                    unions.push(value_type.as_ref());
                }
                if let Some(known_items) = &keyed.known_items {
                    unions.extend(known_items.values().map(|(_, item)| item));
                }
                unions
            }
        };

        for callback_type in callback_types {
            for callable in callback_type.types.iter().filter_map(|atomic| match atomic {
                TAtomic::Callable(TCallable::Alias(identifier)) => Some(*identifier),
                TAtomic::Callable(TCallable::Signature(signature)) => signature.source,
                _ => None,
            }) {
                record_callback_throw_types(context, block_context, callable, invocation.span);
            }
        }
    }
}

fn array_map_callback_is_definitely_not_invoked(
    artifacts: &AnalysisArtifacts,
    invocation: &Invocation<'_, '_, '_>,
    identifier: &FunctionLikeIdentifier,
) -> bool {
    if !matches!(identifier, FunctionLikeIdentifier::Function(name) if name.as_bytes().eq_ignore_ascii_case(b"array_map"))
    {
        return false;
    }

    let mut array_arguments = Vec::new();
    let mut positional_parameter_index = 0;
    let mut saw_named_argument = false;

    for argument in invocation.arguments_source.iter_arguments() {
        if argument.is_unpacked() {
            return false;
        }

        match argument.get_parameter_name() {
            Some(b"callback") => saw_named_argument = true,
            Some(b"array") | Some(b"arrays") => {
                saw_named_argument = true;
                let Some(expression) = argument.value() else {
                    return false;
                };
                array_arguments.push(expression);
            }
            Some(_) => return false,
            None => {
                if saw_named_argument || !argument.is_positional() {
                    return false;
                }

                if positional_parameter_index == 0 {
                    positional_parameter_index = 1;
                    continue;
                }

                positional_parameter_index += 1;
                let Some(expression) = argument.value() else {
                    return false;
                };
                array_arguments.push(expression);
            }
        }
    }

    !array_arguments.is_empty()
        && array_arguments.iter().all(|expression| {
            artifacts.get_expression_type(expression).is_some_and(|array_type| {
                !array_type.types.is_empty()
                    && array_type.types.iter().all(|atomic| matches!(atomic, TAtomic::Array(array) if array.is_empty()))
            })
        })
}

fn record_callback_throw_types<'ctx, A>(
    context: &Context<'ctx, '_, A>,
    block_context: &mut BlockContext<'ctx>,
    callable: FunctionLikeIdentifier,
    span: Span,
) where
    A: Arena,
{
    let Some(callback_metadata) = context.codebase.get_function_like(&callable) else {
        return;
    };

    let class_like = match callable {
        FunctionLikeIdentifier::Method(class_name, _) => context.codebase.get_class_like(class_name.as_bytes()),
        FunctionLikeIdentifier::Function(_) | FunctionLikeIdentifier::Closure(_) => None,
    };

    for thrown_type in context.codebase.get_function_like_thrown_types(class_like, callback_metadata) {
        for atomic_type in thrown_type.type_union.types.iter() {
            if !atomic_type.extends_or_implements(context.codebase, b"Throwable") {
                continue;
            }

            for thrown_atomic in get_throw_type_atomics(atomic_type) {
                if is_structural_throw_type(&thrown_atomic)
                    && thrown_atomic.extends_or_implements(context.codebase, b"Throwable")
                {
                    let thrown_generic = PossiblyThrownGenericException {
                        object_names: get_throw_type_object_names(context.codebase, &thrown_atomic),
                        atomic_type: thrown_atomic,
                        span,
                    };
                    block_context.possibly_thrown_generic_exceptions.insert(thrown_generic);
                } else {
                    for exception in thrown_atomic.get_all_object_names() {
                        block_context.possibly_thrown_exceptions.entry(exception).or_default().insert(span);
                    }
                }
            }
        }
    }
}

fn immediate_callback_parameter_offset(identifier: &FunctionLikeIdentifier) -> Option<usize> {
    let FunctionLikeIdentifier::Function(name) = identifier else {
        return None;
    };

    match name.as_bytes() {
        b"array_all"
        | b"array_any"
        | b"array_filter"
        | b"array_find"
        | b"array_find_key"
        | b"array_reduce"
        | b"array_walk"
        | b"array_walk_recursive"
        | b"iterator_apply"
        | b"preg_replace_callback"
        | b"uasort"
        | b"uksort"
        | b"usort" => Some(1),
        b"array_map" | b"preg_replace_callback_array" => Some(0),
        b"call_user_func" | b"call_user_func_array" | b"forward_static_call" | b"forward_static_call_array" => Some(0),
        _ => None,
    }
}

fn has_template_binding_within_constraint<A>(
    context: &Context<'_, '_, A>,
    parameter: &TGenericParameter,
    template_result: &TemplateResult,
) -> bool
where
    A: Arena,
{
    let Some(bounds) = template_result
        .lower_bounds
        .get(&parameter.parameter_name)
        .and_then(|bounds| bounds.get(&parameter.defining_entity))
    else {
        return false;
    };

    let bound_type = get_most_specific_type_from_bounds(bounds, context.codebase);

    if bound_type.is_never() {
        return false;
    }

    if !bound_type.types.is_empty()
        && bound_type.types.iter().all(|atomic| {
            matches!(
                atomic,
                TAtomic::GenericParameter(bound_parameter)
                    if bound_parameter.parameter_name == parameter.parameter_name
                        && bound_parameter.defining_entity == parameter.defining_entity
            )
        })
    {
        return false;
    }

    let bound_is_within_constraint = is_contained_by(
        context.codebase,
        &bound_type,
        &parameter.constraint,
        false,
        false,
        false,
        &mut ComparisonResult::default(),
    );
    if !bound_is_within_constraint {
        return false;
    }

    let constraint_is_within_bound = is_contained_by(
        context.codebase,
        &parameter.constraint,
        &bound_type,
        false,
        false,
        false,
        &mut ComparisonResult::default(),
    );

    if !constraint_is_within_bound {
        return true;
    }

    let has_explicit_inference = get_relevant_bounds(bounds).iter().any(|bound| bound.argument_offset.is_some());
    let has_explicit_default = template_result.template_types.get(&parameter.parameter_name).is_some_and(|templates| {
        templates
            .iter()
            .any(|template| template.defining_entity == parameter.defining_entity && template.default.is_some())
    });

    !bound_type.has_template_types() && (has_explicit_inference || has_explicit_default)
}

fn display_callable_name<A>(
    context: &Context<'_, '_, A>,
    identifier: &FunctionLikeIdentifier,
    original_name: Word,
) -> String
where
    A: Arena,
{
    match identifier {
        FunctionLikeIdentifier::Function(_) => format!("`{original_name}`"),
        FunctionLikeIdentifier::Method(class_name, _) => {
            let class_display =
                context.codebase.get_class_like(class_name.as_bytes()).map(|m| m.original_name).unwrap_or(*class_name);

            format!("`{class_display}::{original_name}`")
        }
        FunctionLikeIdentifier::Closure(name) => format!("`{name}`"),
    }
}

fn record_null_context_assertions<'ctx, 'arena, A>(
    context: &mut Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &mut AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    this_variable: Option<&[u8]>,
    assertions: &BTreeMap<Word, Conjunction<Assertion>>,
    template_result: &TemplateResult,
    parameters: &WordMap<TUnion>,
) where
    A: Arena,
{
    if assertions.is_empty() {
        return;
    }
    // ExpressionResult carries specifications separately from the walked scope.
    // In particular, a call on the left of && does not narrow its skip edge.
    let mut specification_context = block_context.clone();
    specification_context.clauses.clear();
    let type_assertions = resolve_invocation_assertion(
        context,
        &mut specification_context,
        artifacts,
        invocation,
        this_variable,
        assertions,
        template_result,
        parameters,
        true,
    );

    block_context
        .stable_method_calls
        .extend(type_assertions.keys().filter(|variable| variable.as_bytes().ends_with(b"()")).copied());

    let range = (invocation.span.start.offset, invocation.span.end.offset);
    // AssertFunctionTypeSpecifyingExtension asks its argument for truthy specifications
    // in every context. Declared unconditional @assert tags, unlike this extension,
    // contribute only in null context.
    if invocation.target.get_function_like_metadata().is_some_and(|metadata| {
        metadata.flags.is_built_in() && metadata.name.as_bytes().eq_ignore_ascii_case(b"assert")
    }) {
        crate::reconciler::reconcile_keyed_types(
            context,
            &type_assertions,
            AssertionMap::default(),
            &mut specification_context,
            &mut WordSet::default(),
            &type_assertions.keys().copied().collect(),
            &invocation.span,
            false,
            false,
        );
        let scope = |block: &BlockContext<'_>| BooleanExpressionScope {
            locals: block.locals.clone(),
            variable_state: block.variable_state.clone(),
            variables_possibly_in_scope: block.variables_possibly_in_scope.clone(),
            stable_method_calls: block.stable_method_calls.clone(),
            stable_method_call_assertions: block.stable_method_call_assertions.clone(),
        };
        let narrowed = scope(&specification_context);
        artifacts.boolean_expression_scopes.insert(range, BooleanExpressionScopes {
            post: scope(block_context),
            truthy: narrowed.clone(),
            falsey: narrowed,
        });
    }
    let clauses = artifacts.null_context_clauses.entry(range).or_default();
    clauses.extend(specification_context.clauses.iter().map(|clause| (**clause).clone()));
    for (variable, assertions) in type_assertions {
        for alternatives in assertions {
            let mut possibilities = AssertionMap::default();
            possibilities.insert(
                variable,
                alternatives.into_iter().map(|assertion| (assertion.to_hash(), assertion)).collect(),
            );
            clauses.push(Clause::new(possibilities, invocation.span, invocation.span, None, None, None));
        }
    }
}

/// PHPStan's OpenSslEncryptParameterOutTypeExtension: `openssl_encrypt()`'s `$tag` is a non-empty-string for a
/// constant gcm/ccm cipher, null for another constant cipher, and either when the cipher is not constant.
/// ponytail: a constant cipher the runtime OpenSSL does not support is null upstream; any gcm/ccm name counts here
fn openssl_encrypt_tag(
    invocation: &Invocation<'_, '_, '_>,
    parameter: Option<&mago_codex::misc::VariableIdentifier>,
    artifacts: &AnalysisArtifacts,
) -> Option<TUnion> {
    if !matches!(invocation.target.get_function_like_identifier(),
        Some(FunctionLikeIdentifier::Function(name)) if name.as_bytes().eq_ignore_ascii_case(b"openssl_encrypt"))
        || parameter?.0.as_bytes() != b"$tag"
    {
        return None;
    }
    let cipher = invocation.arguments_source.get_argument(1)?.value()?;
    let ciphers = artifacts.get_expression_type(cipher).map(|t| {
        t.types.iter().map(TAtomic::get_literal_string_value).collect::<Option<Vec<_>>>().unwrap_or_default()
    }).unwrap_or_default();
    if ciphers.is_empty() {
        return Some(TUnion::from_vec(vec![TAtomic::Scalar(TScalar::non_empty_string()), TAtomic::Null]));
    }
    let aead = |c: &[u8]| { let c = c.to_ascii_lowercase(); c.ends_with(b"gcm") || c.ends_with(b"ccm") };
    let mut types = Vec::new();
    if ciphers.iter().any(|c| aead(c)) {
        types.push(TAtomic::Scalar(TScalar::non_empty_string()));
    }
    if ciphers.iter().any(|c| !aead(c)) {
        types.push(TAtomic::Null);
    }
    Some(TUnion::from_vec(types))
}

/// FuncCallScopeEffectsHelper::getArrayFunctionAppendingType(): `array_push()` and `array_unshift()` with only
/// unpacked values that may be empty may add nothing, so a possibly empty array stays possibly empty. The stub's
/// `@param-out` always makes it non-empty.
fn appends_only_possibly_empty_values(invocation: &Invocation<'_, '_, '_>, artifacts: &AnalysisArtifacts, array_type: &TUnion) -> bool {
    let is_appending = matches!(
        invocation.target.get_function_like_identifier(),
        Some(FunctionLikeIdentifier::Function(name))
            if name.as_bytes().eq_ignore_ascii_case(b"array_push") || name.as_bytes().eq_ignore_ascii_case(b"array_unshift")
    );
    let values = invocation.arguments_source.get_arguments().into_iter().skip(1).collect::<Vec<_>>();

    is_appending
        && !values.is_empty()
        && !array_type.types.iter().all(TAtomic::is_non_empty_array)
        && values.iter().all(|argument| {
            argument.is_unpacked()
                && !argument
                    .value()
                    .and_then(|value| artifacts.get_expression_type(value))
                    .is_some_and(|value_type| value_type.types.iter().all(TAtomic::is_non_empty_array))
        })
}

/// Unpacking an array into by-reference parameters binds each of its values, so the out type replaces the values.
fn with_unpacked_values(array_type: &TUnion, value_type: TUnion, codebase: &CodebaseMetadata) -> TUnion {
    let value_type = Arc::new(value_type);

    TUnion::from_vec(
        array_type
            .types
            .iter()
            .map(|atomic| match atomic {
                TAtomic::Array(TArray::List(list)) => TAtomic::Array(TArray::List(if list.non_empty {
                    TList::new_non_empty(Arc::clone(&value_type))
                } else {
                    TList::new(Arc::clone(&value_type))
                })),
                TAtomic::Array(array @ TArray::Keyed(keyed)) => {
                    let mut values = TKeyedArray::new_with_parameters(
                        Arc::new(get_array_parameters(array, codebase).0),
                        Arc::clone(&value_type),
                    );
                    values.non_empty = keyed.non_empty;

                    TAtomic::Array(TArray::Keyed(values))
                }
                atomic => atomic.clone(),
            })
            .collect(),
    )
}

pub(crate) fn update_by_reference_argument_types<'ctx, 'arena, A>(
    context: &mut Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &mut AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    template_result: &TemplateResult,
    parameters: &WordMap<TUnion>,
    positional_writeback: bool,
) -> Result<(), AnalysisError>
where
    A: Arena,
{
    let constraint_type = invocation.target.is_method_call();

    for (parameter_offset, parameter_ref) in invocation.target.iter_parameters().enumerate() {
        if !parameter_ref.is_by_reference() {
            continue;
        }

        let (argument, argument_id) = if positional_writeback {
            // If ArgumentsNormalizer cannot reorder named operands, PHPStan's
            // OUT loop retains their written positions, not a per-name binding.
            let argument = invocation.arguments_source.get_argument(parameter_offset).and_then(|argument| argument.value());
            let id = argument.and_then(|argument| get_block_expression_id(argument, context, block_context));
            (argument, id)
        } else {
            get_argument_for_parameter(
                context,
                block_context,
                invocation,
                Some(parameter_offset),
                parameter_ref.get_name().map(|name| name.0),
            )
        };

        if let Some(argument) = argument {
            // An input reference cannot create its source. Query the post-argument running scope:
            // another argument may have assigned this variable after its initial read.
            // Derived reads need not have a separate local-certainty entry: their variable root
            // owns existence, while assignment below updates the offset and invalidates its parent.
            if block_context.variable_state.non_creating_references.contains(&argument.span())
                && get_root_expression_id(argument)
                    .or(argument_id)
                    .is_some_and(|name| !block_context.variable_certainty(name).is_defined())
            {
                continue;
            }
            let argument_type = artifacts.get_expression_type(argument).cloned().unwrap_or_else(get_mixed);
            if !is_argument_mutated_by_reference(
                &invocation.target,
                parameter_offset,
                &argument_type,
                parameter_ref.is_by_reference(),
            ) {
                continue;
            }

            if !is_argument_referenceable(argument, &argument_type) {
                continue;
            }

            let declared_had_templates = parameter_ref
                .get_out_type()
                .or_else(|| parameter_ref.get_type())
                .is_some_and(|declared| declared.has_template_types());

            let mut new_type = if parameter_offset > 0 && is_array_multisort(&invocation.target) {
                argument_type.clone()
            } else if let Some(tag) = openssl_encrypt_tag(invocation, parameter_ref.get_name(), artifacts) {
                tag
            } else if let Some(out) = parameter_ref.get_name().and_then(|name| {
                context.plugin_registry.get_function_parameter_type(
                    context.codebase,
                    context.source_file,
                    block_context,
                    artifacts,
                    invocation,
                    name.0.as_bytes(),
                    false,
                )
            }) {
                out
            } else {
                parameter_ref
                    .get_out_type()
                    .or_else(|| parameter_ref.get_type())
                    .cloned()
                    .map_or_else(get_mixed, |new_type| {
                        resolve_invocation_type(context, invocation, template_result, parameters, new_type)
                    })
            };

            // If the argument's current type is `never`, this call is unreachable
            // its by-reference effect cannot happen, so the variable's type must
            // remain `never`. Without this guard, calling a template-generic
            // function (e.g. `array_pop`) on a `never` argument resolves the
            // templates to their constraints (e.g `mixed`), and that widened
            // out-type leaks through loop widening into reachable iterations.
            if let Some(argument_id) = &argument_id
                && let Some(existing_type) = block_context.locals.get(argument_id)
                && existing_type.is_never()
            {
                continue;
            }

            if declared_had_templates
                && !invocation.target.get_function_like_metadata().is_some_and(|metadata| metadata.flags.is_built_in())
            {
                new_type.widen_literals();
            }

            if invocation.arguments_source.get_argument(parameter_offset).is_some_and(|argument| argument.is_unpacked())
            {
                new_type = with_unpacked_values(&argument_type, new_type, context.codebase);
            }

            if parameter_offset == 0 && appends_only_possibly_empty_values(invocation, artifacts, &argument_type) {
                new_type = add_union_type(new_type, &get_empty_keyed_array(), context.codebase, CombinerOptions::default());
            }

            new_type.set_by_reference(true);
            refresh_array_root_type(context, block_context, artifacts, argument);

            let new_type = Rc::new(new_type);
            if constraint_type && let Some(argument_id) = argument_id {
                if let Some(existing_type) = block_context.locals.get(&argument_id).cloned() {
                    block_context.remove_descendants(context, argument_id, &existing_type, Some(&new_type));
                }

                block_context.remove_variable_from_conflicting_clauses(context, argument_id, None);
                block_context.parent_conflicting_clause_variables.insert(argument_id);

                assign_to_expression(
                    context,
                    block_context,
                    artifacts,
                    argument,
                    Some(argument_id),
                    Some(argument),
                    Rc::clone(&new_type),
                    false,
                    PropertyWriteKind::Mutation,
                )?;

                block_context.assigned_variable_ids.insert(argument_id, argument.start_offset());
                block_context.by_reference_constraints.insert(
                    argument_id,
                    ReferenceConstraint::new(
                        argument.span(),
                        ReferenceConstraintSource::Argument,
                        Some(Rc::clone(&new_type)),
                    ),
                );

                record_by_reference_mutation_in_loop(artifacts, argument_id, new_type);
            } else {
                if let Some(argument_id) = &argument_id
                    && let Some(existing_type) = block_context.locals.get(argument_id).cloned()
                {
                    block_context.remove_descendants(context, *argument_id, &existing_type, Some(&new_type));
                    block_context.remove_variable_from_conflicting_clauses(context, *argument_id, None);
                    block_context.parent_conflicting_clause_variables.insert(*argument_id);
                }

                assign_to_expression(
                    context,
                    block_context,
                    artifacts,
                    argument,
                    argument_id,
                    Some(argument),
                    Rc::clone(&new_type),
                    false,
                    PropertyWriteKind::Mutation,
                )?;

                if let Some(argument_id) = argument_id {
                    block_context.assigned_variable_ids.insert(argument_id, argument.start_offset());
                    record_by_reference_mutation_in_loop(artifacts, argument_id, new_type);
                }
            }
        }
    }

    Ok(())
}

fn refresh_array_root_type<A>(
    context: &Context<'_, '_, A>,
    block_context: &BlockContext<'_>,
    artifacts: &mut AnalysisArtifacts,
    argument: &Expression<'_>,
) where
    A: Arena,
{
    let mut root = argument;
    while let Expression::ArrayAccess(ArrayAccess { array, .. }) | Expression::ArrayAppend(ArrayAppend { array, .. }) =
        unwrap_expression(root)
    {
        root = array;
    }

    if !std::ptr::eq(root, argument)
        && let Some(root_id) = get_block_expression_id(root, context, block_context)
        && let Some(root_type) = block_context.locals.get(&root_id)
    {
        artifacts.set_rc_expression_type(root, Rc::clone(root_type));
    }
}

/// Records a by-reference mutation in the enclosing loop scope so the multi-pass
/// analysis widens the variable's type on the next pass.
fn record_by_reference_mutation_in_loop(artifacts: &mut AnalysisArtifacts, variable_id: Word, new_type: Rc<TUnion>) {
    let Some(loop_scope) = artifacts.get_loop_scope_mut() else {
        return;
    };

    if loop_scope.parent_context_variables.contains_key(&variable_id) {
        loop_scope.possibly_redefined_loop_parent_variables.insert(variable_id, Rc::clone(&new_type));
        loop_scope.by_reference_loop_mutations.insert(variable_id, new_type);
    }
}

/// PHPStan's MethodCallHandler distinguishes known side effects from possible impurity.
/// The same distinction owns receiver invalidation and whether a call can be specified.
pub(crate) fn method_has_side_effects<A>(
    context: &Context<'_, '_, A>,
    target: &InvocationTarget<'_>,
) -> bool
where
    A: Arena,
{
    let metadata = target.get_function_like_metadata();
    let method_context = target.get_method_context();
    let invalidating_class = method_context.and_then(|method| {
        let declaring = method.declaring_method_id?;
        Some(method.class_like_metadata.appearing_method_ids.get(&method.requested_name)
            .unwrap_or(&declaring).get_class_name())
    });
    // FileTypeMapper remaps the trait's PHPDoc scope to its alias, leaving the
    // original method's native declaration but not its PHPDoc in the consuming class.
    let aliased_source = method_context.is_some_and(|method| {
        let Some(declaring) = method.declaring_method_id else { return false };
        let Some(storage) = context.codebase.get_class_like(declaring.get_class_name().as_bytes()) else { return false };
        let class = invalidating_class
            .and_then(|name| context.codebase.get_class_like(name.as_bytes()))
            .unwrap_or(storage);
        storage.kind.is_trait() && storage.name != class.name
            && class.trait_alias_map.iter().any(|(alias, source)| {
                *source == method.requested_name
                    && class.declaring_method_ids.get(alias) == Some(&declaring)
            })
    });
    metadata.is_none_or(|metadata| {
        let Some(method) = &metadata.method_metadata else { return false };
        let returns = if aliased_source {
            &metadata.return_type_declaration_metadata
        } else {
            &metadata.return_type_metadata
        }.as_ref().map(|ty| &ty.type_union);
        let constructor = method_context.map_or(method.is_constructor, |method| method.requested_name.as_bytes() == b"__construct");
        constructor
            || metadata.reflected_side_effects.map_or_else(
                || returns.is_some_and(TUnion::is_void)
                    || metadata.declared_purity().filter(|_| !aliased_source).map_or_else(
                        || returns.is_some_and(|ty| {
                            matches!(ty.types.as_ref(), [atomic] if atomic.is_this())
                        }),
                        |pure| !pure,
                    ),
                |effects| effects == ReflectedSideEffects::Yes,
            )
    })
}

/// Clears observations dependent on receivers or arguments that may have changed.
pub(crate) fn clear_object_property_narrowings<'ctx, 'arena, A>(
    context: &Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    invocation: &Invocation<'ctx, '_, 'arena>,
    receiver_variable: Option<&[u8]>,
) where
    A: Arena,
{
    let metadata = invocation.target.get_function_like_metadata();
    let suspends_fiber = metadata.is_some_and(|metadata| metadata.flags.suspends_fiber());
    let invalidating_class = invocation.target.get_method_context().and_then(|method| {
        let declaring = method.declaring_method_id?;
        Some(method.class_like_metadata.appearing_method_ids.get(&method.requested_name)
            .unwrap_or(&declaring).get_class_name())
    });
    let invalidated_method_receiver = receiver_variable
        .filter(|_| method_has_side_effects(context, &invocation.target))
        .map(Word::new);
    // PHPStan 2.3.1 keeps property fetches for static methods, not their method-call results.
    let invalidated_receiver = invalidated_method_receiver
        .filter(|_| metadata.and_then(|metadata| metadata.method_metadata.as_ref()).is_none_or(|method| !method.is_static));
    let preserves_property_narrowings = metadata.is_some_and(|metadata| {
        (metadata.flags.is_pure() || metadata.flags.is_mutation_free() || metadata.flags.is_external_mutation_free())
            && !suspends_fiber
    }) && invalidated_receiver.is_none();

    let invalidates_method_call = |key: Word| {
        references_method_call_key(key)
            && (suspends_fiber || invalidated_method_receiver.is_some_and(|receiver| is_descendant_of_any(key, &[receiver])))
    };
    block_context.stable_method_call_assertions.retain(|key, _| !invalidates_method_call(*key));
    block_context.stable_method_calls.retain(|key| !invalidates_method_call(*key));
    block_context.invalidate_expression_holders(|key, _| invalidates_method_call(key));
    block_context.locals.retain(|key, _| !invalidates_method_call(*key));
    let retains_method_clause = |clause: &Rc<Clause>| !clause.possibilities.keys().copied().any(invalidates_method_call);
    block_context.clauses.retain(retains_method_clause);
    block_context.reconciled_expression_clauses.retain(retains_method_clause);

    if preserves_property_narrowings {
        return;
    }

    let preserved: WordSet = block_context
        .locals
        .keys()
        .copied()
        .filter(|var_id| {
            is_property_or_index_key(*var_id)
                && (property_root_is_immutable(context, block_context, *var_id)
                    || (invalidated_receiver.is_some_and(|receiver| is_descendant_of_any(*var_id, &[receiver]))
                        && invalidating_class.is_some_and(|class| {
                            private_property_of_other_class(context, block_context, *var_id, class)
                        })))
        })
        .collect();
    let is_self_method_call = invalidated_receiver.is_some_and(|receiver| receiver.as_bytes() == b"$this");
    let clears_this = is_self_method_call || (suspends_fiber && block_context.scope.get_class_like_name().is_some());
    if is_self_method_call {
        block_context.definitely_uninitialized_property_ids.clear();
    }

    block_context.invalidate_expression_holders(|key, guard| {
        (clears_this && ((guard && key.as_bytes() == b"$this")
                || (key.as_bytes().starts_with(b"$this->") && (guard || !preserved.contains(&key)))))
            || is_superglobal_index_key(key)
            || is_superglobal_name(key.as_bytes())
    });
    let mut resource_ids = WordSet::default();
    block_context.locals.retain(|var_id, current_type| {
        if suspends_fiber
            && current_type
                .types
                .iter()
                .any(|atomic| matches!(atomic, TAtomic::Resource(resource) if resource.is_open()))
        {
            resource_ids.insert(*var_id);
            let mut widened = (**current_type).clone();
            for atomic in widened.types.to_mut() {
                if let TAtomic::Resource(resource) = atomic
                    && resource.is_open()
                {
                    *resource = TResource::new(None);
                }
            }
            *current_type = Rc::new(widened);
        }

        if clears_this && var_id.as_bytes().starts_with(b"$this->") && !preserved.contains(var_id) {
            return false;
        }
        if is_superglobal_index_key(*var_id) {
            return false;
        }
        if is_superglobal_name(var_id.as_bytes()) {
            if let Some(declared) = crate::common::global::get_global_variable_type(*var_id) {
                *current_type = declared;
                return true;
            }
            return false;
        }
        true
    });

    let retains_clause = |clause: &Rc<Clause>| {
        !clause.possibilities.keys().copied().any(|var_id| {
            !clause.wedge
                && (resource_ids.contains(&var_id)
                    || (clears_this
                        && var_id.as_bytes().starts_with(b"$this->")
                        && !preserved.contains(&var_id))
                    || is_superglobal_index_key(var_id)
                    || is_superglobal_name(var_id.as_bytes()))
        })
    };
    block_context.clauses.retain(retains_clause);
    block_context.reconciled_expression_clauses.retain(retains_clause);

    // If the callee imports any variables via `global $x;` anywhere in its body, it can
    // reassign them in the caller's global scope. Widen any literal narrowings we were
    // holding for those specific variables so later checks don't assume stale values.
    if let Some(metadata) = metadata
        && !metadata.globals_accessed.is_empty()
    {
        let mut touched_globals: foldhash::HashSet<Word> = foldhash::HashSet::default();
        for name in &metadata.globals_accessed {
            if let Some(existing) = block_context.locals.get(name).cloned() {
                let mut widened = (*existing).clone();
                widened.widen_scalars();
                block_context.locals.insert(*name, Rc::new(widened));
                touched_globals.insert(*name);
            }
        }

        if !touched_globals.is_empty() {
            block_context.clauses.retain(|clause| {
                clause.wedge || !clause.possibilities.keys().copied().any(|k| touched_globals.contains(&k))
            });
            block_context.reconciled_expression_clauses.retain(|clause| {
                clause.wedge || !clause.possibilities.keys().copied().any(|k| touched_globals.contains(&k))
            });
        }
    }

    let mut this_escapes = matches!(receiver_variable, Some(v) if v == b"$this");
    let mut escaped_roots: Vec<Word> = invalidated_receiver.into_iter().collect();
    let mut container_escapes = false;
    let mut has_object_argument = invalidated_receiver.is_some();
    // ArgumentsHandler: a method whose declared return type is `$this` or `static` of its declaring class, and that is
    // not `@phpstan-impure`, does not invalidate its object arguments (a fluent `$shipment->add($order)`).
    let keeps_arguments = invocation.target.get_method_context().is_some_and(|method| {
        let declaring = method.declaring_method_id.map(|id| id.get_class_name());
        metadata.is_some_and(|metadata| {
            metadata.declared_purity() != Some(false)
                && metadata.return_type_metadata.as_ref().is_some_and(|return_type| {
                    matches!(
                        return_type.type_union.types.as_ref(),
                        [TAtomic::Object(mago_codex::ttype::atomic::object::TObject::Named(named))]
                            if named.is_this
                                || named.is_static
                                    && declaring.is_some_and(|class| class.as_bytes().eq_ignore_ascii_case(named.name.as_bytes()))
                    )
                })
        })
    });
    for argument in invocation.arguments_source.iter_arguments().filter(|_| !keeps_arguments) {
        let Some(expression) = argument.value() else {
            continue;
        };

        let Some(argument_id) = get_block_expression_id(expression, context, block_context) else {
            continue;
        };

        let is_object = block_context.locals.get(&argument_id).is_some_and(|t| t.has_object_type());

        if argument_id.as_bytes() == b"$this" {
            this_escapes = true;
            has_object_argument = has_object_argument || is_object;

            continue;
        }

        if is_object {
            has_object_argument = true;

            if is_plain_variable(argument_id) {
                container_escapes = true;
            } else {
                escaped_roots.push(argument_id);
            }
        }
    }

    if !has_object_argument {
        block_context.retain_valid_class_type_relations();

        return;
    }

    if this_escapes {
        block_context.definitely_uninitialized_property_ids.clear();
        escaped_roots.push(Word::new(b"$this"));
    }

    let should_wipe = |var_id: Word, guard: bool| -> bool {
        if guard && escaped_roots.contains(&var_id) {
            return true;
        }
        if !is_property_or_index_key(var_id) {
            return false;
        }

        if !guard && preserved.contains(&var_id) {
            return false;
        }

        if container_escapes {
            if !this_escapes && var_id.as_bytes().starts_with(b"$this->") {
                return false;
            }

            return true;
        }

        is_descendant_of_any(var_id, &escaped_roots)
    };

    block_context.stable_method_calls.retain(|key| !should_wipe(*key, false));
    block_context.stable_method_call_assertions.retain(|key, _| !should_wipe(*key, false));
    block_context.invalidate_expression_holders(should_wipe);
    block_context.locals.retain(|var_id, _| !should_wipe(*var_id, false));

    block_context.clauses.retain(|clause| clause.wedge || !clause.possibilities.keys().copied().any(|key| should_wipe(key, false)));
    block_context
        .reconciled_expression_clauses
        .retain(|clause| clause.wedge || !clause.possibilities.keys().copied().any(|key| should_wipe(key, false)));
    block_context.retain_valid_class_type_relations();
}

fn array_has_string_keys(atomic: &TAtomic) -> bool {
    let TAtomic::Array(TArray::Keyed(keyed)) = atomic else {
        return false;
    };

    keyed.known_items.as_ref().is_some_and(|items| items.keys().any(|key| matches!(key, ArrayKey::String(_))))
        || keyed.get_generic_parameters().is_some_and(|(key_type, _)| key_type.types.iter().all(TAtomic::is_string))
}

fn references_method_call_key(key: Word) -> bool {
    let bytes = key.as_bytes();
    memchr::memchr_iter(b'(', bytes).any(|index| bytes.get(index + 1) == Some(&b')'))
}

fn is_property_or_index_key(var_id: Word) -> bool {
    let s = var_id.as_bytes();
    memchr::memmem::find(s, b"->").is_some() || (s.starts_with(b"$") && s.contains(&b'['))
}

fn is_plain_variable(var_id: Word) -> bool {
    let s = var_id.as_bytes();
    s.starts_with(b"$") && !s.contains(&b'[') && memchr::memmem::find(s, b"->").is_none()
}

fn is_descendant_of_any(var_id: Word, roots: &[Word]) -> bool {
    let key = var_id.as_bytes();
    roots.iter().any(|root| {
        let root = root.as_bytes();
        uses_variable_as_offset(key, root)
            || key.strip_prefix(root).is_some_and(|rest| rest.starts_with(b"->") || rest.first() == Some(&b'['))
    })
}

fn property_root_is_immutable<A>(context: &Context<'_, '_, A>, block_context: &BlockContext<'_>, var_id: Word) -> bool
where
    A: Arena,
{
    let bytes = var_id.as_bytes();
    let Some(mut arrow) = memchr::memmem::find(bytes, b"->") else {
        return false;
    };

    loop {
        let property_start = arrow + 2;
        let next_arrow = memchr::memmem::find(&bytes[property_start..], b"->").map(|offset| property_start + offset);
        let property_end = next_arrow.unwrap_or(bytes.len());
        let property = &bytes[property_start..property_end];
        if property.contains(&b'[') {
            return false;
        }

        let Some(root_type) = block_context.locals.get(&Word::new(&bytes[..arrow])) else {
            return false;
        };

        if root_type.types.is_empty()
            || !root_type.types.iter().all(|atom| atom_property_is_immutable(context, atom, property))
        {
            return false;
        }

        let Some(next_arrow) = next_arrow else {
            return true;
        };

        arrow = next_arrow;
    }
}

fn atom_property_is_immutable<A>(context: &Context<'_, '_, A>, atom: &TAtomic, property: &[u8]) -> bool
where
    A: Arena,
{
    let TAtomic::Object(object) = atom else {
        return false;
    };

    let Some(class_name) = object.get_name() else {
        return false;
    };

    let Some(class_metadata) = context.codebase.get_class_like(class_name.as_bytes()) else {
        return false;
    };

    if class_metadata.flags.is_readonly() || class_metadata.flags.is_mutation_free() {
        return true;
    }

    is_readonly_property(context, class_name.as_bytes(), property)
}

fn is_readonly_property<A>(context: &Context<'_, '_, A>, class_name: &[u8], property: &[u8]) -> bool
where
    A: Arena,
{
    context
        .codebase
        .get_declaring_property(class_name, concat_word!(b"$", property).as_bytes())
        .is_some_and(|property| property.flags.is_readonly())
}

/// MutatingScope::isPrivatePropertyOfDifferentClass uses the callee's reflected declaring class.
fn private_property_of_other_class<A>(
    context: &Context<'_, '_, A>,
    block_context: &BlockContext<'_>,
    key: Word,
    invalidating_class: Word,
) -> bool
where
    A: Arena,
{
    let bytes = key.as_bytes();
    let Some(arrow) = memchr::memmem::rfind(bytes, b"->") else { return false };
    let property = &bytes[arrow + 2..];
    if property.contains(&b'[') { return false; }
    let Some(root_type) = block_context.locals.get(&Word::new(&bytes[..arrow])) else { return false };
    !root_type.types.is_empty() && root_type.types.iter().all(|atom| {
        let TAtomic::Object(object) = atom else { return false };
        let Some(class) = object.get_name().and_then(|name| context.codebase.get_class_like(name.as_bytes())) else {
            return false;
        };
        let property = concat_word!("$", property);
        let Some(declaring) = class.declaring_property_ids.get(&property) else { return false };
        !declaring.as_bytes().eq_ignore_ascii_case(invalidating_class.as_bytes())
            && context.codebase.get_property(declaring.as_bytes(), property.as_bytes())
                .is_some_and(|property| property.read_visibility.is_private())
    })
}

/// A variable ID like `$_SESSION['user_id']` - an index access rooted at a PHP
/// superglobal. These are reachable from any function body, so a non-pure call
/// might mutate them regardless of its arguments.
fn is_superglobal_index_key(var_id: Word) -> bool {
    let s = var_id.as_bytes();
    if !s.starts_with(b"$_") && !s.starts_with(b"$GLOBALS[") {
        return false;
    }
    let Some(bracket_pos) = memchr::memchr(b'[', s) else {
        return false;
    };

    is_superglobal_name(&s[..bracket_pos])
}

fn is_superglobal_name(name: &[u8]) -> bool {
    matches!(
        name,
        b"$_SESSION"
            | b"$_GET"
            | b"$_POST"
            | b"$_COOKIE"
            | b"$_SERVER"
            | b"$_ENV"
            | b"$_FILES"
            | b"$_REQUEST"
            | b"$GLOBALS"
    )
}

fn resolve_invocation_assertion<'ctx, 'arena, A>(
    context: &mut Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    this_variable: Option<&[u8]>,
    assertions: &BTreeMap<Word, Conjunction<Assertion>>,
    template_result: &TemplateResult,
    parameters: &WordMap<TUnion>,
    is_unconditional_assert: bool,
) -> AssertionMap<Word, AssertionSet>
where
    A: Arena,
{
    let mut type_assertions: AssertionMap<Word, AssertionSet> = AssertionMap::default();
    if assertions.is_empty() {
        return type_assertions;
    }

    let mut pending_redundant_issues = Vec::new();
    let mut has_non_redundant_assertion = false;

    for (parameter_id, variable_assertions) in assertions {
        let (assertion_expression, mut assertion_variable) =
            resolve_argument_or_special_target(context, block_context, invocation, *parameter_id, this_variable);

        // TypeSpecifier::createForExpr(): an argument `$a ?? $b` asserted to be a type that `$b` can never be
        // narrows `$a`, so `str_starts_with($a ?? '', 'x')` makes `$a` a non-falsy string.
        if assertion_variable.is_none()
            && let Some(Expression::Binary(binary)) = assertion_expression.map(unwrap_expression)
            && matches!(binary.operator, BinaryOperator::NullCoalesce(_))
            && let Some(right) = artifacts.get_expression_type(binary.rhs)
            && !variable_assertions.is_empty()
            && variable_assertions.iter().all(|assertion| match assertion {
                Assertion::IsType(atomic) => !can_expression_types_be_identical(
                    context.codebase,
                    &TUnion::from_atomic(atomic.clone()),
                    right,
                    false,
                    false,
                ),
                _ => false,
            })
        {
            assertion_variable = get_block_expression_id(binary.lhs, context, block_context);
        }

        if matches!(assertion_expression, Some(Expression::Call(_)))
            && let [assertion] = variable_assertions.as_slice()
            && is_truthiness_assertion(assertion)
        {
            assertion_variable = None;
        }

        match assertion_variable {
            Some(assertion_variable) => {
                let non_nullsafe_assertion_variable = get_non_nullsafe_expression_id(assertion_variable);
                let assertion_variable = non_nullsafe_assertion_variable.unwrap_or(assertion_variable);
                let mut new_variable_possibilities: AssertionSet = vec![];
                let mut resolved_or_clause: Disjunction<Assertion> = Vec::new();

                let asserted_type = match block_context.locals.get(&assertion_variable) {
                    Some(local) if local.possibly_undefined() => assertion_expression
                        .and_then(|expression| artifacts.get_expression_type(expression))
                        .or(Some(local)),
                    Some(local) => Some(local.as_ref()),
                    None => non_nullsafe_assertion_variable
                        .and(assertion_expression)
                        .and_then(|expression| artifacts.get_expression_type(expression)),
                };
                let mut any_possible = false;
                let mut has_resolved_types = false;
                let mut all_negated = true;
                let mut always_redundant = true;

                for variable_assertion in variable_assertions {
                    all_negated = all_negated && variable_assertion.is_negation();

                    if variable_assertion.has_equality() {
                        always_redundant = false;
                    }

                    let Some(assertion_atomic) = variable_assertion.get_type() else {
                        add_and_assertion(&mut new_variable_possibilities, variable_assertion.clone());
                        always_redundant = false;

                        continue;
                    };

                    let resolved_assertion_type = resolve_invocation_type(
                        context,
                        invocation,
                        template_result,
                        parameters,
                        TUnion::from_atomic(assertion_atomic.to_owned()),
                    );

                    if !resolved_assertion_type.is_never() {
                        has_resolved_types = true;

                        if !any_possible
                            && let Some(asserted_type) = &asserted_type
                            && (can_expression_types_be_identical(
                                context.codebase,
                                asserted_type,
                                &resolved_assertion_type,
                                false,
                                false,
                            ) || generic_keyed_array_can_be_list(asserted_type, &resolved_assertion_type))
                        {
                            any_possible = true;
                        }

                        let assertion_loses_template_precision =
                            derived_assertion_loses_template_precision(assertion_atomic);

                        if always_redundant
                            && let Some(asserted_type) = &asserted_type
                            && !asserted_type.is_mixed()
                            && !resolved_assertion_type.has_template()
                            && !assertion_loses_template_precision
                        {
                            let mut comparison_result = ComparisonResult::default();
                            let is_subtype = is_contained_by(
                                context.codebase,
                                asserted_type,
                                &resolved_assertion_type,
                                false,
                                false,
                                true,
                                &mut comparison_result,
                            );

                            if !is_subtype {
                                always_redundant = false;
                            }
                        } else {
                            always_redundant = false;
                        }

                        for resolved_atomic in resolved_assertion_type.types.into_owned() {
                            resolved_or_clause.push(variable_assertion.with_type(resolved_atomic));
                        }
                    } else if let Some(asserted_type) = &asserted_type {
                        always_redundant = false;
                        match variable_assertion {
                            Assertion::IsType(_)
                                if !can_expression_types_be_identical(
                                    context.codebase,
                                    asserted_type,
                                    &resolved_assertion_type,
                                    false,
                                    false,
                                ) =>
                            {
                                let asserted_type_id = asserted_type.get_id();
                                let expected_type_id = resolved_assertion_type.get_id();

                                context.collector.report_with_code(
                                        IssueCode::ImpossibleTypeComparison,
                                        Issue::error(format!(
                                            "Impossible type assertion: `{assertion_variable}` of type `{asserted_type_id}` can never be `{expected_type_id}`."
                                        ))
                                        .with_annotation(
                                            Annotation::primary(invocation.span)
                                                .with_message(format!("Argument `{assertion_variable}` has type `{asserted_type_id}`")),
                                        )
                                        .with_note(format!(
                                            "The assertion expects `{assertion_variable}` to be `{expected_type_id}`, but no value of type `{asserted_type_id}` can satisfy this."
                                        ))
                                        .with_help("Check that the correct variable is being passed, or update the assertion type."),
                                    );
                            }
                            Assertion::IsIdentical(_) => {
                                let intersection = if let Some(intersection) =
                                    intersect_union_with_union(context, asserted_type, &resolved_assertion_type)
                                {
                                    intersection
                                } else {
                                    let asserted_type_id = asserted_type.get_id();
                                    let expected_type_id = resolved_assertion_type.get_id();

                                    context.collector.report_with_code(
                                        IssueCode::ImpossibleTypeComparison,
                                        Issue::error(format!(
                                            "Impossible type assertion: `{assertion_variable}` of type `{asserted_type_id}` can never be identical to `{expected_type_id}`."
                                        ))
                                        .with_annotation(
                                            Annotation::primary(invocation.span)
                                                .with_message(format!("Argument `{assertion_variable}` has type `{asserted_type_id}`")),
                                        )
                                        .with_note(format!(
                                            "The assertion expects `{assertion_variable}` to be identical to `{expected_type_id}`, but no value of type `{asserted_type_id}` can satisfy this."
                                        ))
                                        .with_help("Check that the correct variable is being passed, or update the assertion type."),
                                    );

                                    get_never()
                                };

                                for intersection_atomic in intersection.types.into_owned() {
                                    add_and_assertion(
                                        &mut new_variable_possibilities,
                                        Assertion::IsIdentical(intersection_atomic),
                                    );
                                }
                            }
                            _ => {
                                // ignore
                            }
                        }
                    }
                }

                if has_resolved_types
                    && (!any_possible || always_redundant)
                    && let Some(asserted_type) = &asserted_type
                {
                    let asserted_type_id = asserted_type.get_id();
                    let expected_type_id = resolved_or_clause
                        .iter()
                        .filter_map(|a| a.get_type().map(|t| t.get_id().to_string()))
                        .collect::<Vec<_>>()
                        .join("|");

                    let suppress_redundant = is_unconditional_assert
                        && (!invocation.target.is_pure_or_mutation_free()
                            || invocation.target.get_return_type().is_some_and(|t| !t.is_void() && !t.is_never()));

                    if all_negated {
                        if !any_possible && !suppress_redundant {
                            pending_redundant_issues.push(
                                Issue::warning(format!(
                                    "Redundant type assertion: `{assertion_variable}` of type `{asserted_type_id}` is always not `{expected_type_id}`."
                                ))
                                .with_annotation(
                                    Annotation::primary(invocation.span)
                                        .with_message(format!("Argument `{assertion_variable}` has type `{asserted_type_id}`")),
                                )
                                .with_note(format!(
                                    "The negated assertion against `{expected_type_id}` always holds because `{assertion_variable}` is `{asserted_type_id}`."
                                ))
                                .with_help("Consider removing this assertion as it has no effect."),
                            );
                        }
                    } else if always_redundant {
                        if suppress_redundant {
                            // Side effects or a meaningful return value mean removing the call
                            // would lose behavior. Skip the redundant warning.
                        } else {
                            pending_redundant_issues.push(
                                Issue::warning(format!(
                                    "Redundant type assertion: `{assertion_variable}` is already `{asserted_type_id}`."
                                ))
                                .with_annotation(
                                    Annotation::primary(invocation.span)
                                        .with_message(format!("Argument `{assertion_variable}` already has type `{asserted_type_id}`")),
                                )
                                .with_note(format!(
                                    "The assertion against `{expected_type_id}` always holds because `{assertion_variable}` is `{asserted_type_id}`."
                                ))
                                .with_help("Consider removing this assertion or replacing it with `default` if used in a `match` arm."),
                            );
                        }
                    } else {
                        context.collector.report_with_code(
                            IssueCode::ImpossibleTypeComparison,
                            Issue::error(format!(
                                "Impossible type assertion: `{assertion_variable}` of type `{asserted_type_id}` can never be `{expected_type_id}`."
                            ))
                            .with_annotation(
                                Annotation::primary(invocation.span)
                                    .with_message(format!("Argument `{assertion_variable}` has type `{asserted_type_id}`")),
                            )
                            .with_note(format!(
                                "The assertion expects `{assertion_variable}` to be `{expected_type_id}`, but no value of type `{asserted_type_id}` can satisfy this."
                            ))
                            .with_help("Check that the correct variable is being passed, or update the assertion type."),
                        );
                    }
                }

                let assertion_is_redundant =
                    has_resolved_types && if all_negated { !any_possible } else { always_redundant };
                if !assertion_is_redundant {
                    has_non_redundant_assertion = true;
                }

                if !resolved_or_clause.is_empty() {
                    let resolved_clause_is_conjunction = resolved_or_clause.iter().all(Assertion::is_negation);

                    if let Some(assertion_expression) = assertion_expression {
                        add_access_base_assertions(
                            context,
                            block_context,
                            artifacts,
                            assertion_expression,
                            &resolved_or_clause,
                            &mut type_assertions,
                        );
                    }

                    if resolved_clause_is_conjunction {
                        let has_multiple_exclusions = resolved_or_clause.len() > 1;
                        for assertion in resolved_or_clause {
                            if has_multiple_exclusions
                                && let Assertion::IsNotType(atomic) = &assertion
                                && let Some(asserted_type) = asserted_type
                            {
                                let excluded_type = TUnion::from_atomic(atomic.clone());
                                if !can_expression_types_be_identical(
                                    context.codebase,
                                    asserted_type,
                                    &excluded_type,
                                    false,
                                    false,
                                ) && !generic_keyed_array_can_be_list(asserted_type, &excluded_type)
                                {
                                    continue;
                                }
                            }

                            add_and_assertion(&mut new_variable_possibilities, assertion);
                        }
                    } else {
                        add_and_clause(&mut new_variable_possibilities, &resolved_or_clause);
                    }
                }

                if !new_variable_possibilities.is_empty() {
                    type_assertions.entry(assertion_variable).or_default().extend(new_variable_possibilities);
                }
            }
            None => {
                if let Some(assertion_expression) = assertion_expression {
                    if expression_is_nullsafe(assertion_expression) {
                        let mut resolved_assertions = Vec::new();

                        for assertion in variable_assertions {
                            let Some(atomic) = assertion.get_type() else {
                                resolved_assertions.push(assertion.clone());
                                continue;
                            };

                            let resolved_type = resolve_invocation_type(
                                context,
                                invocation,
                                template_result,
                                parameters,
                                TUnion::from_atomic(atomic.clone()),
                            );

                            resolved_assertions.extend(
                                resolved_type.types.into_owned().into_iter().map(|atomic| assertion.with_type(atomic)),
                            );
                        }

                        has_non_redundant_assertion |= add_access_base_assertions(
                            context,
                            block_context,
                            artifacts,
                            assertion_expression,
                            &resolved_assertions,
                            &mut type_assertions,
                        );
                    }

                    if variable_assertions.len() != 1 {
                        continue; // We only support single assertions for expressions
                        // maybe we should support more? idk for now, we are following
                        // psalm implementation.
                    }

                    let variable_assertion = &variable_assertions[0];

                    let clauses = match variable_assertion {
                        Assertion::IsNotType(TAtomic::Scalar(TScalar::Bool(TBool { value: Some(false) })))
                        | Assertion::IsType(TAtomic::Scalar(TScalar::Bool(TBool { value: Some(true) })))
                        | Assertion::Truthy => get_formula(
                            assertion_expression.span(),
                            assertion_expression.span(),
                            assertion_expression,
                            context.get_assertion_context_from_block(block_context),
                            artifacts,
                            &context.settings.algebra_thresholds(),
                            context.settings.formula_size_threshold,
                        ),
                        Assertion::IsNotType(TAtomic::Scalar(TScalar::Bool(TBool { value: Some(true) })))
                        | Assertion::IsType(TAtomic::Scalar(TScalar::Bool(TBool { value: Some(false) })))
                        | Assertion::Falsy => get_formula(
                            assertion_expression.span(),
                            assertion_expression.span(),
                            assertion_expression,
                            context.get_assertion_context_from_block(block_context),
                            artifacts,
                            &context.settings.algebra_thresholds(),
                            context.settings.formula_size_threshold,
                        )
                        .map(|clauses| {
                            negate_or_synthesize(
                                &clauses,
                                assertion_expression,
                                context.get_assertion_context_from_block(block_context),
                                artifacts,
                                &context.settings.algebra_thresholds(),
                                context.settings.formula_size_threshold,
                            )
                        }),

                        _ => {
                            continue; // Unsupported assertion kind for expression
                        }
                    };

                    let new_clauses = clauses.unwrap_or_default();
                    if !new_clauses.is_empty() {
                        has_non_redundant_assertion = true;
                    }

                    let previous_clause_count = block_context.clauses.len();
                    for clause in &new_clauses {
                        block_context.clauses.push(Rc::new(clause.clone()));
                    }

                    let clauses = saturate_clauses(
                        block_context.clauses.iter().map(Rc::as_ref),
                        &context.settings.algebra_thresholds(),
                    );

                    let (truths, _) = find_satisfying_assignments(&clauses, None, &mut WordSet::default());
                    let mut previous_truths = None;
                    for (variable, assertions) in truths {
                        if !new_clauses.iter().any(|clause| clause.possibilities.contains_key(&variable)) {
                            let previous_truths = previous_truths.get_or_insert_with(|| {
                                let previous_clauses = saturate_clauses(
                                    block_context.clauses[..previous_clause_count].iter().map(Rc::as_ref),
                                    &context.settings.algebra_thresholds(),
                                );

                                find_satisfying_assignments(&previous_clauses, None, &mut WordSet::default()).0
                            });

                            if previous_truths.get(&variable) == Some(&assertions) {
                                continue;
                            }
                        }

                        type_assertions.entry(variable).or_default().extend(assertions);
                    }
                }
            }
        }
    }

    if !has_non_redundant_assertion {
        for issue in pending_redundant_issues {
            context.collector.report_with_code(IssueCode::RedundantTypeComparison, issue);
        }
    }

    type_assertions
}

fn is_truthiness_assertion(assertion: &Assertion) -> bool {
    matches!(
        assertion,
        Assertion::Truthy
            | Assertion::Falsy
            | Assertion::IsType(TAtomic::Scalar(TScalar::Bool(TBool { value: Some(_) })))
            | Assertion::IsNotType(TAtomic::Scalar(TScalar::Bool(TBool { value: Some(_) })))
    )
}

fn add_access_base_assertions<'arena, A>(
    context: &Context<'_, 'arena, A>,
    block_context: &BlockContext<'_>,
    artifacts: &AnalysisArtifacts,
    expression: &Expression<'arena>,
    assertions: &[Assertion],
    type_assertions: &mut AssertionMap<Word, AssertionSet>,
) -> bool
where
    A: Arena,
{
    let excludes_null = if assertions.iter().all(Assertion::is_negation) {
        assertions.iter().any(assertion_excludes_null)
    } else {
        assertions.iter().all(assertion_excludes_null)
    };

    if !excludes_null {
        return false;
    }

    let mut added_assertion = false;
    for base in get_access_base_expressions(expression, artifacts) {
        let Some(base_id) = get_block_expression_id(base, context, block_context) else {
            continue;
        };

        let base_id = get_non_nullsafe_expression_id(base_id).unwrap_or(base_id);
        add_and_assertion(type_assertions.entry(base_id).or_default(), Assertion::IsNotType(TAtomic::Null));
        added_assertion = true;
    }

    added_assertion
}

fn assertion_excludes_null(assertion: &Assertion) -> bool {
    match assertion {
        Assertion::IsType(atomic) | Assertion::IsIdentical(atomic) => match atomic {
            TAtomic::Null | TAtomic::Void => false,
            TAtomic::Mixed(mixed) => mixed.is_non_null(),
            TAtomic::GenericParameter(parameter) => !parameter.constraint.can_be_null(),
            _ => true,
        },
        Assertion::IsNotType(atomic) | Assertion::IsNotIdentical(atomic) | Assertion::IsNotEqual(atomic) => {
            atomic.is_null()
        }
        Assertion::Truthy | Assertion::IsIsset | Assertion::IsEqualIsset | Assertion::NonEmpty => true,
        _ => false,
    }
}

fn generic_keyed_array_can_be_list(asserted_type: &TUnion, assertion_type: &TUnion) -> bool {
    if !assertion_type.types.iter().any(|atomic| matches!(atomic, TAtomic::Array(TArray::List(_)))) {
        return false;
    }

    asserted_type.types.iter().any(|atomic| {
        let TAtomic::Array(TArray::Keyed(keyed)) = atomic else {
            return false;
        };

        let Some((key_type, _)) = keyed.parameters.as_ref() else {
            return false;
        };

        key_type.types.iter().any(|key_atomic| {
            matches!(key_atomic, TAtomic::GenericParameter(parameter) if parameter.constraint.is_array_key())
        })
    })
}

fn derived_assertion_loses_template_precision(assertion: &TAtomic) -> bool {
    let TAtomic::Derived(derived) = assertion else {
        return false;
    };

    derived.get_target_type().is_some_and(TUnion::has_template_types)
}

/// TypeSpecifier::create() in a true context: a `$left ?? $right` argument whose right side the asserted type can never
/// be specifies `$left` instead (`isArray($a['k'] ?? null)` of an `@phpstan-assert-if-true array` narrows `$a['k']`).
fn coalesce_argument_assertions<'ctx, 'arena, A>(
    context: &Context<'ctx, 'arena, A>,
    block_context: &BlockContext<'ctx>,
    artifacts: &AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    assertions: &BTreeMap<Word, Conjunction<Assertion>>,
) -> Vec<(Word, AssertionSet)>
where
    A: Arena,
{
    let mut specified = Vec::new();
    for (parameter, parameter_assertions) in assertions {
        let (Some(argument), None) = get_argument_for_parameter(context, block_context, invocation, None, Some(*parameter))
        else {
            continue;
        };
        let Expression::Binary(binary) = crate::utils::misc::unwrap_expression(argument) else { continue };
        if !matches!(binary.operator, BinaryOperator::NullCoalesce(_)) {
            continue;
        }
        let (Some(left), Some(right_type)) = (
            crate::utils::expression::get_block_expression_id(binary.lhs, context, block_context),
            artifacts.get_expression_type(binary.rhs),
        ) else {
            continue;
        };
        let disjoint = parameter_assertions.iter().all(|assertion| match assertion {
            Assertion::IsType(atomic) => {
                let asserted = TUnion::from_atomic(atomic.clone());
                !asserted.has_template_types()
                    && !can_expression_types_be_identical(context.codebase, right_type, &asserted, false, false)
            }
            _ => false,
        });
        if disjoint && !parameter_assertions.is_empty() {
            specified.push((left, parameter_assertions.iter().map(|assertion| vec![assertion.clone()]).collect()));
        }
    }
    specified
}

/// Resolves an argument or a special assertion target from an invocation.
///
/// This function serves as a convenient wrapper that orchestrates the logic for handling
/// both special assertion targets (like `$this`) and standard function arguments.
///
/// It first attempts to resolve the target as a special `$this` or `self::` reference
/// using `resolve_special_assertion_target`. If successful, it returns the resolved ID,
/// and the expression part of the tuple will be `None`.
///
/// If the target is not a special reference, it then calls `get_argument_for_parameter`
/// to find the corresponding argument passed to the function call and returns its result.
///
/// # Arguments
/// * `context`: The analysis context.
/// * `block_context`: The context of the current block.
/// * `invocation`: The invocation being analyzed.
/// * `parameter_offset`: The zero-based index of the parameter.
/// * `parameter_name`: The name of the parameter or assertion target.
/// * `this_variable`: The name of the variable holding the object instance (`$this`), if any.
///
/// # Returns
/// A tuple `(Option<&'a Expression>, Option<Word>)`.
/// * If a special target is resolved, the tuple is `(None, Some(resolved_id))`.
/// * If a regular argument is found, it returns the result from `get_argument_for_parameter`.
/// * If nothing is found, it returns `(None, None)`.
fn resolve_argument_or_special_target<'ctx, 'ast, 'arena, A>(
    context: &Context<'ctx, 'arena, A>,
    block_context: &BlockContext<'ctx>,
    invocation: &Invocation<'ctx, 'ast, 'arena>,
    parameter_name: Word,
    this_variable: Option<&[u8]>,
) -> (Option<&'ast Expression<'arena>>, Option<Word>)
where
    A: Arena,
{
    // First, check if the name refers to a special assertion target like `$this->...`
    if let Some(resolved_id) = resolve_special_assertion_target(block_context, parameter_name, this_variable) {
        return (None, Some(resolved_id));
    }

    if let Some(offset) = memchr::memmem::find(parameter_name.as_bytes(), b"->") {
        let parameter = Word::new(&parameter_name.as_bytes()[..offset]);
        let suffix = &parameter_name.as_bytes()[offset..];
        let (expression, argument) =
            get_argument_for_parameter(context, block_context, invocation, None, Some(parameter));

        return (expression, argument.map(|argument| concat_word!(argument.as_bytes(), suffix)));
    }

    // If not a special target, treat it as a regular parameter and find its argument.
    get_argument_for_parameter(context, block_context, invocation, None, Some(parameter_name))
}

/// Resolves special assertion targets like `$this->...` or `self::...`.
///
/// This function checks if the provided target name corresponds to a property or constant
/// on the current object (`$this`) or class (`self`). If it matches, it rewrites the
/// target string with the appropriate contextual variable name (e.g., `$instance->...`).
/// It should be called before attempting to find an argument for a parameter, as these
/// targets do not correspond to passed arguments.
///
/// # Arguments
/// * `block_context`: The context of the current block, used to get class scope information.
/// * `target_name`: The string identifier for the assertion target (e.g., `'$this->prop'`).
/// * `this_variable`: The name of the variable holding the object instance (`$this`), if any.
///
/// # Returns
/// * `Some(Word)`: If the target is a special `$this` or `self` reference, containing the resolved variable ID.
/// * `None`: If the target is not a special reference and should be treated as a regular parameter.
fn resolve_special_assertion_target(
    block_context: &BlockContext<'_>,
    target_name: Word,
    this_variable: Option<&[u8]>,
) -> Option<Word> {
    let target_bytes = target_name.as_bytes();
    if let Some(this_variable) = this_variable
        && (target_bytes == InvocationAssertions::RECEIVER || target_bytes.starts_with(b"$this->"))
    {
        let mut out: Vec<u8> = Vec::with_capacity(target_bytes.len() - 5 + this_variable.len());
        out.extend_from_slice(this_variable);
        out.extend_from_slice(&target_bytes[5..]);
        return Some(Word::from(out.as_slice()));
    }

    if let Some(class) = block_context.scope.get_class_like_name()
        && target_bytes.starts_with(b"self::")
    {
        let class_bytes = class.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(target_bytes.len() - 6 + class_bytes.len());
        out.extend_from_slice(class_bytes);
        out.extend_from_slice(&target_bytes[6..]);
        return Some(Word::from(out.as_slice()));
    }

    None
}

/// Finds the argument expression passed to a function for a specific parameter.
///
/// This function is designed to robustly identify the argument for a given parameter,
/// mirroring PHP's own argument resolution rules. The caller can provide the parameter's
/// name, its zero-based offset, or both.
///
/// # Arguments
///
/// * `context`: The analysis context, needed for `get_expression_id`.
/// * `block_context`: The context of the current block, needed for `get_expression_id`.
/// * `invocation`: The invocation being analyzed, which contains the arguments.
/// * `metadata`: The metadata of the invoked function, used to look up parameter details.
/// * `parameter_offset`: An optional zero-based index of the parameter.
/// * `parameter_name`: An optional name of the parameter.
///
/// # Returns
/// A tuple containing:
/// * `Option<&'a Expression>`: The argument's expression CST node, if found.
/// * `Option<Word>`: The unique ID of the argument expression (e.g., a variable name), if it can be determined.
fn get_argument_for_parameter<'ctx, 'ast, 'arena, A>(
    context: &Context<'ctx, 'arena, A>,
    block_context: &BlockContext<'ctx>,
    invocation: &Invocation<'ctx, 'ast, 'arena>,
    mut parameter_offset: Option<usize>,
    mut parameter_name: Option<Word>,
) -> (Option<&'ast Expression<'arena>>, Option<Word>)
where
    A: Arena,
{
    // If neither name nor offset is provided, we can't do anything.
    if parameter_name.is_none() && parameter_offset.is_none() {
        return (None, None);
    }

    // Step 1: Ensure we have both the name and offset for the parameter.
    if parameter_name.is_none() {
        if let Some(parameter_ref) = parameter_offset.and_then(|offset| invocation.target.get_parameter(offset)) {
            parameter_name = parameter_ref.get_name().map(|name| name.0);
        }
    } else if parameter_offset.is_none()
        && let Some(name) = parameter_name
    {
        parameter_offset = invocation
            .target
            .iter_parameters()
            .position(|parameter| parameter.get_name().is_some_and(|name_variable| name_variable.0 == name));
    }

    // After attempting to fill in missing info, if we still lack a name or an offset,
    // the parameter is invalid for this function.
    let (_, Some(offset)) = (parameter_name, parameter_offset) else {
        return (None, None);
    };

    // Step 2: Resolve the argument with the correct precedence.
    let arguments = invocation.arguments_source;

    // a. Look for a named argument first.
    let find_by_name = || {
        let variable = parameter_name?;
        let variable_bytes = variable.as_bytes();
        let variable_name: &[u8] = variable_bytes.trim_prefix(b"$");

        arguments.iter_arguments().find(|argument| {
            if let Some(named_argument) = argument.get_named_argument() {
                named_argument.name.value == variable_name
            } else {
                false
            }
        })
    };

    // b. If not found by name, look for a positional argument at the correct offset.
    let find_by_position = || arguments.get_argument(offset).filter(|argument| argument.is_positional());

    let argument = find_by_name().or_else(find_by_position);

    let Some(argument) = argument else {
        // The corresponding argument could not be found.
        return (None, None);
    };

    let Some(argument_expression) = argument.value() else {
        // The argument is a placeholder, no expression to analyze
        return (None, None);
    };

    // If an argument was found, resolve its expression ID.
    let argument_id = get_block_expression_id(argument_expression, context, block_context);
    let argument_id = match argument_id {
        Some(id) => Some(id),
        None => {
            if let Expression::Binary(binary) = unwrap_expression(argument_expression)
                && matches!(binary.operator, BinaryOperator::NullCoalesce(_))
                && matches!(unwrap_expression(binary.rhs), Expression::Literal(Literal::Null(_)))
            {
                get_block_expression_id(binary.lhs, context, block_context)
            } else {
                None
            }
        }
    };

    (Some(argument_expression), argument_id)
}

fn collect_plugin_throw_types<'ctx, 'arena, A>(
    context: &Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    identifier: &FunctionLikeIdentifier,
) where
    A: Arena,
{
    let exceptions = match identifier {
        FunctionLikeIdentifier::Function(name) => context.plugin_registry.get_function_thrown_exceptions(
            context.codebase,
            context.source_file,
            block_context,
            artifacts,
            name.as_bytes(),
            invocation,
        ),
        FunctionLikeIdentifier::Method(class_name, method_name) => {
            context.plugin_registry.get_method_thrown_exceptions(
                context.codebase,
                context.source_file,
                block_context,
                artifacts,
                class_name.as_bytes(),
                method_name.as_bytes(),
                invocation,
            )
        }
        FunctionLikeIdentifier::Closure(_) => return,
    };

    for exception in exceptions {
        block_context.possibly_thrown_exceptions.entry(exception).or_default().insert(invocation.span);
    }
}

fn apply_plugin_assertions<'ctx, 'arena, A>(
    context: &mut Context<'ctx, 'arena, A>,
    block_context: &mut BlockContext<'ctx>,
    artifacts: &mut AnalysisArtifacts,
    invocation: &Invocation<'ctx, '_, 'arena>,
    identifier: &FunctionLikeIdentifier,
    this_variable: Option<&[u8]>,
    template_result: &TemplateResult,
    parameters: &WordMap<TUnion>,
    range: (u32, u32),
) -> Result<(), AnalysisError>
where
    A: Arena,
{
    let Some(assertions) = context.plugin_registry.get_function_like_assertions(
        context.codebase,
        context.source_file,
        block_context,
        artifacts,
        identifier,
        invocation,
        context.external_analysis_session,
    ) else {
        return Ok(());
    };

    let resolved_if_true_assertions = resolve_invocation_assertion(
        context,
        block_context,
        artifacts,
        invocation,
        this_variable,
        &assertions.if_true,
        template_result,
        parameters,
        false,
    );

    for (variable, assertion_set) in resolved_if_true_assertions {
        artifacts.if_true_assertions.entry(range).or_default().entry(variable).or_default().extend(assertion_set);
    }

    let resolved_if_false_assertions = resolve_invocation_assertion(
        context,
        block_context,
        artifacts,
        invocation,
        this_variable,
        &assertions.if_false,
        template_result,
        parameters,
        false,
    );

    for (variable, assertion_set) in resolved_if_false_assertions {
        artifacts.if_false_assertions.entry(range).or_default().entry(variable).or_default().extend(assertion_set);
    }

    record_null_context_assertions(
        context,
        block_context,
        artifacts,
        invocation,
        this_variable,
        &assertions.type_assertions,
        template_result,
        parameters,
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use mago_word::Word;

    use super::is_superglobal_index_key;
    use super::is_superglobal_name;
    use super::references_method_call_key;

    #[test]
    fn superglobal_index_keys_preserve_first_bracket_matching() {
        let roots: [&[u8]; 16] = [
            b"$_SESSION",
            b"$_GET",
            b"$_POST",
            b"$_COOKIE",
            b"$_SERVER",
            b"$_ENV",
            b"$_FILES",
            b"$_REQUEST",
            b"$GLOBALS",
            b"$_UNKNOWN",
            b"$global",
            b"$GLOBALSx",
            b"$_GET->property",
            b"$local",
            b"",
            b"\xff",
        ];
        let suffixes: [&[u8]; 7] = [b"", b"[0]", b"['key']", b"[0][1]", b"->property[0]", b"\0[0]", b"\xff[0]"];
        for root in roots {
            for suffix in suffixes {
                let key = [root, suffix].concat();
                let expected =
                    key.iter().position(|byte| *byte == b'[').is_some_and(|index| is_superglobal_name(&key[..index]));
                assert_eq!(is_superglobal_index_key(Word::new(&key)), expected, "{key:?}");
            }
        }
    }

    fn check_method_call_key(bytes: &[u8]) {
        assert_eq!(
            references_method_call_key(Word::new(bytes)),
            bytes.windows(2).any(|pair| pair == b"()"),
            "{bytes:?}"
        );
    }

    #[test]
    fn method_call_keys_match_byte_pair_search() {
        let alphabet = [b'(', b')', b'$', b'a', 0, 0xff];
        for length in 0..=4 {
            for mut value in 0..alphabet.len().pow(length) {
                let mut bytes = vec![0; length as usize];
                for byte in &mut bytes {
                    *byte = alphabet[value % alphabet.len()];
                    value /= alphabet.len();
                }
                check_method_call_key(&bytes);
            }
        }
    }

    #[test]
    fn method_call_keys_match_at_scan_boundaries() {
        for length in [15, 16, 17, 31, 32, 33, 63, 64, 65, 255, 256, 257] {
            let mut bytes = vec![b'('; length];
            check_method_call_key(&bytes);
            bytes.push(b')');
            check_method_call_key(&bytes);

            bytes.fill(0xff);
            check_method_call_key(&bytes);
            for index in 0..bytes.len() - 1 {
                bytes[index..index + 2].copy_from_slice(b"()");
                check_method_call_key(&bytes);
                bytes[index..index + 2].fill(0xff);
            }
        }
    }
}
