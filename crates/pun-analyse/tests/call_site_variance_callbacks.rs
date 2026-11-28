use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// A callback parameter of a method on `Items<covariant Animal>` takes `Animal`, not `covariant Animal`, so a closure typed
// `Animal` is accepted and one typed `stdClass` is not (#175): PHPStan 2.3.5's errors on callbacks.php at level 6
#[test]
fn a_call_site_variance_stays_off_a_member_type() {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/call-site-variance-callbacks");
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
    let mut project = Project::new(vec![root.join("callbacks.php")], Vec::new());
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
