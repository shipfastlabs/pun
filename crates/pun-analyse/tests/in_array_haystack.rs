use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's InArrayFunctionTypeSpecifyingExtension: when a strict `in_array()` of one finite needle is false, the
// needle is removed from the haystack's value type (#101), while a loose call and a true one leave it: PHPStan
// 2.3.5's errors on narrow.php at level 10
#[test]
fn a_false_strict_in_array_removes_the_needle_from_the_haystack() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/in-array-haystack");
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
    let mut project = Project::new(vec![root.join("narrow.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
