use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's StrContainingTypeSpecifyingExtension: a substring search that finds a non-empty needle makes the haystack
// non-empty, or non-falsy for a non-falsy needle, and `$a ?? ''` as the haystack narrows `$a` (#110), while an
// unknown needle and a fallback that is itself non-falsy leave it: PHPStan 2.3.5's errors on search.php at level 10
#[test]
fn a_found_needle_makes_the_haystack_non_empty() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/str-containing");
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
    let mut project = Project::new(vec![root.join("search.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
