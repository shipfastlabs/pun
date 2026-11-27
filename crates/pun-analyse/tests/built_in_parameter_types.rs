use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's PhpDocInheritanceResolver takes an untyped parameter's type from the built-in method it overrides, whose
// stub documents it (`SessionHandlerInterface::read($id)` makes `$id` a string, #100), but not from a user interface:
// PHPStan 2.3.5's errors on sessions.php at level 10
#[test]
fn untyped_parameters_take_the_built_in_parent_types() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/built-in-parameter-types");
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
    let mut project = Project::new(vec![root.join("sessions.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
