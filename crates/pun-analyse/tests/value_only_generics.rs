use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// `Items<Item>` on an iterable class with two templates is `iterable<Item>&Items`, as PHPStan's TypeNodeResolver
// resolves it, so it is accepted as `Items<int, Item>` (#157): PHPStan 2.3.5's errors on items.php at level 8
#[test]
fn one_argument_on_a_two_template_iterable_is_its_value_type() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/value-only-generics");
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
    let mut project = Project::new(vec![root.join("items.php")], Vec::new());
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
