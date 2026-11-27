//! PHPStan's `InArrayFunctionTypeSpecifyingExtension` for a strict `in_array()` over arrays that are not shapes: in
//! the falsey context of one finite needle the haystack holds no value identical to it, so the needle is removed from
//! the haystack's value type (`! in_array(false, $parts, true)` makes `list<GdImage|false>` a `list<GdImage>`), and
//! in the truthy context the haystack is non-empty.

use std::sync::Arc;

use mago_analyzer::plugin::context::{InvocationInfo, ProviderContext};
use mago_analyzer::plugin::provider::assertion::{FunctionAssertionProvider, InvocationAssertions};
use mago_analyzer::plugin::provider::function::FunctionTarget;
use mago_analyzer::plugin::provider::{Provider, ProviderMeta};
use mago_codex::assertion::Assertion;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::array::TArray;
use mago_codex::ttype::union::TUnion;

static META: ProviderMeta = ProviderMeta::new(
    "pun-in-array",
    "pun",
    "PHPStan's in_array() haystack narrowing",
);

pub(super) struct InArray;

impl Provider for InArray {
    fn meta() -> &'static ProviderMeta {
        &META
    }
}

/// A value that is the only value of its type, so `!==` to it removes it.
fn is_finite(atomic: &TAtomic) -> bool {
    atomic.is_null() || atomic.is_true() || atomic.is_false() || atomic.is_literal()
}

fn without(value: &TUnion, needle: &TAtomic) -> Option<TUnion> {
    if !value.types.contains(needle) || value.types.len() < 2 {
        return None;
    }
    let mut value = value.clone();
    value.remove_type(needle);
    Some(value)
}

/// The haystack array with `needle` removed from its value type, `None` when it has no such value or is a shape.
fn narrowed(array: &TArray, needle: &TAtomic) -> Option<TAtomic> {
    match array {
        TArray::List(list) if list.known_elements.is_none() => {
            let mut list = list.clone();
            list.element_type = Arc::new(without(&list.element_type, needle)?);
            Some(TAtomic::Array(TArray::List(list)))
        }
        TArray::Keyed(keyed) if keyed.known_items.is_none() => {
            let mut keyed = keyed.clone();
            let (key, value) = keyed.parameters.clone()?;
            keyed.parameters = Some((key, Arc::new(without(&value, needle)?)));
            Some(TAtomic::Array(TArray::Keyed(keyed)))
        }
        _ => None,
    }
}

/// The haystack array made non-empty, `None` when it is a shape.
fn non_empty(array: &TArray) -> Option<TAtomic> {
    match array {
        TArray::List(list) if list.known_elements.is_none() => {
            let mut list = list.clone();
            list.non_empty = true;
            Some(TAtomic::Array(TArray::List(list)))
        }
        TArray::Keyed(keyed) if keyed.known_items.is_none() && keyed.parameters.is_some() => Some(
            TAtomic::Array(TArray::Keyed(keyed.clone().with_non_empty(true))),
        ),
        _ => None,
    }
}

impl FunctionAssertionProvider for InArray {
    fn targets() -> FunctionTarget {
        FunctionTarget::Exact(b"in_array")
    }

    fn get_assertions(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<InvocationAssertions> {
        let strict = context.get_expression_type(invocation.get_argument(2, &[b"strict"])?)?;
        if !strict.is_true() {
            return None;
        }
        let needle = context.get_expression_type(invocation.get_argument(0, &[b"needle"])?)?;
        let haystack = context.get_expression_type(invocation.get_argument(1, &[b"haystack"])?)?;
        // one array: the assertions of a conjunction intersect, so a union haystack cannot be narrowed per array
        let haystack = haystack.get_single_array()?;
        let mut assertions = InvocationAssertions::new();
        if let Some(non_empty) = non_empty(haystack) {
            assertions.add_if_true(
                mago_word::word("$haystack"),
                vec![Assertion::IsType(non_empty)],
            );
        }
        if let [needle] = needle.types.as_ref()
            && is_finite(needle)
            && let Some(narrowed) = narrowed(haystack, needle)
        {
            assertions.add_if_false(
                mago_word::word("$haystack"),
                vec![Assertion::IsType(narrowed)],
            );
        }
        if assertions.is_empty() {
            return None;
        }
        Some(assertions)
    }
}
