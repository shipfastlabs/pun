use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// After `if (empty($item?->names)) return;` the scope goes on with `$item` an Item and every other variable
// defined (#140): the isset half of empty() must read `$item`, not `$item?`, as the path's root. PHPStan 2.3.5's
// errors on early_return.php at level 3
#[test]
fn a_nullsafe_empty_keeps_the_scope_after_an_early_return() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nullsafe-empty");
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
    let mut project = Project::new(vec![root.join("early_return.php")], Vec::new());
    project.level = 3;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
