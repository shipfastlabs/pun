use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// StrContainingTypeSpecifyingExtension narrows the haystack only in a true context: after a false `str_starts_with()`
// it stays a `string`, so a later literal comparison is not `*NEVER*` (#147), and a stored `str_contains()` result is not
// implied by a stored `str_starts_with()` on the same string (#188): PHPStan 2.3.5's errors on branches.php at level 10
#[test]
fn a_missing_needle_leaves_the_haystack_a_string() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/str-containing-false");
    let oracle: Value =
        serde_json::from_slice(&std::fs::read(root.join("expected-phpstan.json")).unwrap())
            .unwrap();
    let want: Vec<_> = (oracle["errors"].as_array().unwrap().iter())
        .map(|e| {
            (
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_owned(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut project = Project::new(vec![root.join("branches.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
