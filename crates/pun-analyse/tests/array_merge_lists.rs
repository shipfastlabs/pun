use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's ArrayMergeFunctionDynamicReturnTypeExtension: merging arrays whose keys are all integers renumbers them,
// so the result is a list, non-empty when an array always merged is (#103), while a string-keyed input keeps its
// keys: PHPStan 2.3.5's errors on merge.php at level 10
#[test]
fn merged_lists_and_int_keyed_arrays_are_lists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/array-merge-lists");
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
    let mut project = Project::new(vec![root.join("merge.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
