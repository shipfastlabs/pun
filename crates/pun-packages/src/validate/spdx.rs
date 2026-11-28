//! `SpdxLicenses::validate` (composer/spdx-licenses, as Composer 2.11.2 bundles it) for one license string: a known
//! identifier, or an SPDX expression of them (`OR`, `AND`, `WITH` an exception, `+`, `LicenseRef-`, parentheses).

use pcre2::bytes::{Regex, RegexBuilder};
use serde_json::Value;
use std::sync::LazyLock;

/// The identifiers of `assets/spdx.json`, lower-cased as `SpdxLicenses` keys them.
struct Identifiers {
    licenses: Vec<String>,
    known: std::collections::HashSet<String>,
    exceptions: Vec<String>,
}

static IDENTIFIERS: LazyLock<Identifiers> = LazyLock::new(|| {
    let data: Value =
        serde_json::from_str(include_str!("../../assets/spdx.json")).expect("valid spdx.json");
    let licenses: Vec<String> = (data["licenses"].as_object().expect("licenses").keys())
        .map(|id| id.to_ascii_lowercase())
        .collect();
    Identifiers {
        known: licenses.iter().cloned().collect(),
        licenses,
        exceptions: (data["exceptions"].as_array().expect("exceptions").iter())
            .filter_map(Value::as_str)
            .map(str::to_ascii_lowercase)
            .collect(),
    }
});

/// `getLicensesExpression` / `getExceptionsExpression`: the identifiers quoted, sorted descending (`rsort`) and joined.
fn alternatives(ids: &[String]) -> String {
    let mut quoted: Vec<String> = ids.iter().map(|id| crate::php::preg_quote(id)).collect();
    quoted.sort_by(|a, b| b.cmp(a));
    quoted.join("|")
}

static EXPRESSION: LazyLock<Regex> = LazyLock::new(|| {
    let licenses = alternatives(&IDENTIFIERS.licenses);
    let exceptions = alternatives(&IDENTIFIERS.exceptions);
    let pattern = format!(
        r"
(?(DEFINE)
    (?<idstring>[\pL\pN.-]{{1,}})
    (?<licenseid>{licenses})
    (?<licenseexceptionid>{exceptions})
    (?<licenseref>(?:DocumentRef-(?&idstring):)?LicenseRef-(?&idstring))
    (?<simple_expression>(?&licenseid)\+? | (?&licenseid) | (?&licenseref))
    (?<compound_head>
        (?&simple_expression) ( \s+ WITH \s+ (?&licenseexceptionid))?
            | \( \s* (?&compound_expression) \s* \)
    )
    (?<compound_expression>
        (?&compound_head) (?: \s+ (?:AND|OR) \s+ (?&compound_expression))?
    )
    (?<license_expression>(?&compound_expression) | (?&simple_expression))
)
^(NONE | NOASSERTION | (?&license_expression))$
"
    );
    RegexBuilder::new()
        .extended(true)
        .caseless(true)
        .build(&pattern)
        .expect("Composer's SPDX expression")
});

/// `SpdxLicenses::validate` of a string.
pub(super) fn is_valid(license: &str) -> bool {
    IDENTIFIERS.known.contains(&license.to_ascii_lowercase())
        || EXPRESSION.is_match(license.as_bytes()).unwrap_or(false)
}
