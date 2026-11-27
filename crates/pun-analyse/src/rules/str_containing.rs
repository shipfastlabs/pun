//! PHPStan's `StrContainingTypeSpecifyingExtension`: when `str_contains()`, `str_starts_with()`, `strpos()` and their
//! kin find a non-empty needle in a string haystack, the haystack is a `non-empty-string`, or a `non-falsy-string` when
//! the needle is one.

use mago_analyzer::plugin::context::{InvocationInfo, ProviderContext};
use mago_analyzer::plugin::provider::assertion::{FunctionAssertionProvider, InvocationAssertions};
use mago_analyzer::plugin::provider::function::FunctionTarget;
use mago_analyzer::plugin::provider::{Provider, ProviderMeta};
use mago_codex::assertion::Assertion;
use mago_codex::ttype::atomic::TAtomic;
use mago_codex::ttype::atomic::scalar::TScalar;
use mago_codex::ttype::atomic::scalar::string::{TString, TStringCasing};

static META: ProviderMeta = ProviderMeta::new(
    "pun-str-containing",
    "pun",
    "PHPStan's haystack narrowing of substring searches",
);

/// STR_CONTAINING_FUNCTIONS: the haystack is the first argument and the needle the second, except for `fnmatch()`.
const NAMES: &[&[u8]] = &[
    b"fnmatch",
    b"str_contains",
    b"str_starts_with",
    b"str_ends_with",
    b"strpos",
    b"strrpos",
    b"stripos",
    b"strripos",
    b"strstr",
    b"mb_strpos",
    b"mb_strrpos",
    b"mb_stripos",
    b"mb_strripos",
    b"mb_strstr",
];

pub(super) struct StrContaining;

impl Provider for StrContaining {
    fn meta() -> &'static ProviderMeta {
        &META
    }
}

fn string(atomic: &TAtomic) -> Option<&TString> {
    match atomic {
        TAtomic::Scalar(TScalar::String(string)) => Some(string),
        _ => None,
    }
}

impl FunctionAssertionProvider for StrContaining {
    fn targets() -> FunctionTarget {
        FunctionTarget::ExactMultiple(NAMES)
    }

    fn get_assertions(
        &self,
        context: &ProviderContext<'_, '_, '_>,
        invocation: &InvocationInfo<'_, '_, '_>,
    ) -> Option<InvocationAssertions> {
        let (haystack_parameter, needle_position) = if invocation
            .function_name()
            .trim_start_matches('\\')
            .eq_ignore_ascii_case("fnmatch")
        {
            ("$filename", 0)
        } else {
            ("$haystack", 1)
        };
        if invocation.argument_count() < 2 {
            return None;
        }
        let haystack =
            context.get_expression_type(invocation.get_argument(1 - needle_position, &[])?)?;
        let needle = context.get_expression_type(invocation.get_argument(needle_position, &[])?)?;
        if !haystack.is_string() {
            return None;
        }
        let needles = needle
            .types
            .iter()
            .map(string)
            .collect::<Option<Vec<_>>>()?;
        if needles.is_empty() || !needles.iter().all(|needle| needle.is_non_empty()) {
            return None;
        }
        let truthy = needles.iter().all(|needle| needle.is_truthy());
        let asserted =
            TString::general_with_props(false, truthy, true, false, TStringCasing::Unspecified);
        let mut assertions = InvocationAssertions::new();
        assertions.add_if_true(
            mago_word::word(haystack_parameter),
            vec![Assertion::IsType(TAtomic::Scalar(TScalar::String(
                asserted,
            )))],
        );
        Some(assertions)
    }
}
