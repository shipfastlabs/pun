//! PHPStan's `ArrayMergeFunctionDynamicReturnTypeExtension` for arrays that are not constant: merging arrays whose
//! keys are all integers renumbers them, so the result is a list, non-empty when an array that is always merged is
//! non-empty. Constant arrays and string keys keep the stub's return type.

use mago_analyzer::plugin::context::{InvocationInfo, ProviderContext};
use mago_analyzer::plugin::provider::function::{FunctionReturnTypeProvider, FunctionTarget};
use mago_analyzer::plugin::provider::{Provider, ProviderMeta};
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::array::TArray;
use mago_codex::ttype::atomic::array::list::TList;
use mago_codex::ttype::combiner::CombinerOptions;
use mago_codex::ttype::union::TUnion;
use mago_codex::ttype::{combine_union_types, get_array_parameters};
use std::sync::Arc;

static META: ProviderMeta = ProviderMeta::new(
    "pun-array-merge",
    "pun",
    "PHPStan's array_merge() return type",
);

pub(super) struct ArrayMerge;

impl Provider for ArrayMerge {
    fn meta() -> &'static ProviderMeta {
        &META
    }
}

fn is_constant(array: &TArray) -> bool {
    match array {
        TArray::Keyed(keyed) => keyed.parameters.is_none(),
        TArray::List(list) => list.known_elements.is_some(),
    }
}

fn is_empty(array: &TArray) -> bool {
    matches!(array, TArray::Keyed(keyed) if keyed.known_items.is_none() && keyed.parameters.is_none())
}

/// The arrays of a union, `None` unless every member is an array that is not constant (the empty array merges
/// nothing, so it is left out).
fn arrays(ty: &TUnion) -> Option<Vec<&TArray>> {
    ty.types
        .iter()
        .filter(|atomic| !matches!(atomic, TAtomic::Array(array) if is_empty(array)))
        .map(|atomic| match atomic {
            TAtomic::Array(array) if !is_constant(array) => Some(array),
            _ => None,
        })
        .collect()
}

impl FunctionReturnTypeProvider for ArrayMerge {
    fn targets() -> FunctionTarget {
        FunctionTarget::Exact(b"array_merge")
    }

    fn get_return_type(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<TUnion> {
        let codebase = context.codebase();
        let combine = |a: Option<TUnion>, b: &TUnion| match a {
            Some(a) => combine_union_types(&a, b, codebase, CombinerOptions::default()),
            None => b.clone(),
        };
        // Each merged argument's type, and whether it is always merged
        let mut merged: Vec<(TUnion, bool)> = Vec::new();
        for argument in invocation.arguments() {
            let ty = context.get_expression_type(argument.value()?)?;
            if !argument.is_unpacked() {
                merged.push((ty.clone(), true));
                continue;
            }
            if let [TAtomic::Array(TArray::List(list))] = ty.types.as_ref()
                && list.element_type.is_never()
                && let Some(elements) = list.known_elements.as_deref()
            {
                merged.extend(
                    elements
                        .values()
                        .map(|(optional, value)| (value.clone(), !optional)),
                );
                continue;
            }
            let mut value = None;
            let mut non_empty = true;
            for array in arrays(ty)? {
                non_empty &= array.is_non_empty();
                value = Some(combine(value, &get_array_parameters(array, codebase).1));
            }
            merged.push((value?, non_empty));
        }
        if merged.is_empty() {
            return None;
        }
        let (mut values, mut non_empty) = (None, false);
        for (ty, always) in &merged {
            for array in arrays(ty)? {
                let (key, value) = get_array_parameters(array, codebase);
                if !key
                    .types
                    .iter()
                    .all(|k| matches!(k, TAtomic::Scalar(s) if s.is_int()))
                {
                    return None;
                }
                values = Some(combine(values, &value));
            }
            non_empty |= *always
                && ty
                    .types
                    .iter()
                    .all(|a| matches!(a, TAtomic::Array(array) if array.is_non_empty()));
        }
        let mut list = TList::new(Arc::new(values?));
        list.non_empty = non_empty;
        Some(TUnion::from_atomic(TAtomic::Array(TArray::List(list))))
    }
}
