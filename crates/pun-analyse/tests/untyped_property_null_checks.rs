use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// A property without a native type, promoted with a `@param` type or documented with `@var`, compared with `null` is no
// constant condition, as PHPStan's RicherScopeGetTypeHelper says (#170): PHPStan 2.3.5's errors on untyped-promoted.php
// at level 6
#[test]
fn a_property_without_native_type_may_be_null() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/untyped-promoted-property");
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
    let mut project = Project::new(vec![root.join("untyped-promoted.php")], Vec::new());
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
