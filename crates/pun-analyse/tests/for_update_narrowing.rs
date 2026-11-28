use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// ForHandler walks a for loop's update expressions on the back edge of a body entered through the last condition's
// truthy scope, so `$failure = $failure->getPrevious()` sees the `Throwable` that `$failure instanceof \Throwable`
// narrowed (#104). A body that reassigns the variable, or a condition on another variable, narrows nothing there:
// PHPStan 2.3.5's errors on loops.php at level max
#[test]
fn for_update_expressions_see_the_last_condition_truthy_scope() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/for-update-narrowing");
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
    let mut project = Project::new(vec![root.join("loops.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
