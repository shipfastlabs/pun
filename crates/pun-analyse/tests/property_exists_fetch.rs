use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan remembers `property_exists($queued->job, 'afterCommit')` for the fetch `$queued->job` as it does for a
// variable (#107); without the check, or after `$queued->job` is reassigned, the property is still undefined:
// PHPStan 2.3.5's errors on exists.php at level 10
#[test]
fn property_exists_on_a_property_fetch_allows_the_property() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/property-exists-fetch");
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
    let mut project = Project::new(vec![root.join("exists.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
