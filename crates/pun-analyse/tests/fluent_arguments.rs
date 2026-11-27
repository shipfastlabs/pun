use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's ArgumentsHandler: a method declared to return `static` or `$this` of its class, and not `@phpstan-impure`,
// leaves its object arguments' property narrowings in place (#109), while a `void` method and an impure one clear
// them: PHPStan 2.3.5's errors on fluent.php at level 10
#[test]
fn a_fluent_method_keeps_its_arguments_narrowed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fluent-arguments");
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
    let mut project = Project::new(vec![root.join("fluent.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
