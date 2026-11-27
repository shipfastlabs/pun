use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's ArrayMapFunctionReturnTypeExtension: mapping one array that is not a shape keeps its key type and its
// non-emptiness (#105), so `max()` of the result is not reported: PHPStan 2.3.5's errors on map.php at level 10
#[test]
fn mapping_a_non_empty_array_keeps_its_keys_and_non_emptiness() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/array-map-keys");
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
    let mut project = Project::new(vec![root.join("map.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
