use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// `??` on a property fetch whose receiver may be null keeps its right side, as IssetabilityResolution finds no property
// reflection on `Inner|null` (#145, #174), while a fetch of a set readonly property on a non-null receiver drops it:
// PHPStan 2.3.5's errors on coalesce.php at level 8, sorted
#[test]
fn a_coalesce_on_a_nullable_receiver_keeps_its_right_side() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coalesce-nullable-receiver");
    let oracle: Value =
        serde_json::from_slice(&std::fs::read(root.join("expected-phpstan.json")).unwrap())
            .unwrap();
    let mut want: Vec<_> = (oracle["errors"].as_array().unwrap().iter())
        .map(|e| {
            (
                e["file"].as_str().unwrap().to_owned(),
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_owned(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut project = Project::new(vec![root.join("coalesce.php")], Vec::new());
    project.level = 8;
    let prefix = format!("{}/", root.display());
    let mut got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| {
            let file = e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_owned();
            (file, e.line, e.identifier, e.message)
        })
        .collect();
    want.sort();
    got.sort();
    assert_eq!(got, want);
}
