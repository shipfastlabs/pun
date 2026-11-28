use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// A child method narrowing the native return type (`array|\Stringable|null` to `array`) without a PHPDoc inherits the
// parent's `@return` narrowed to that type, as PHPStan's PhpClassReflectionExtension does (#156): PHPStan 2.3.5's errors
// on widgets.php at level 6
#[test]
fn a_narrowed_native_return_keeps_the_inherited_phpdoc() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inherited-narrowed-return");
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
    let mut project = Project::new(vec![root.join("widgets.php")], Vec::new());
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
