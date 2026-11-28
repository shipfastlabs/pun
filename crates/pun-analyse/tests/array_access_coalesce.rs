use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// `$rows[$year]['participants'] ?? 0` on an ArrayAccess object is no always-existing offset: PHPStan's IssetCheck finds
// the object's offset only maybe there, so an earlier offset may be null (#171): PHPStan 2.3.5's errors on coalesce.php
// at level 6
#[test]
fn an_offset_of_an_array_access_object_may_be_null() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/array-access-coalesce");
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
    project.level = 6;
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
