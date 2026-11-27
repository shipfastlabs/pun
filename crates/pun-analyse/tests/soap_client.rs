use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's SoapClientMethodsClassReflectionExtension: any method exists on `SoapClient` and its subclasses and
// returns `mixed` (#141), while a user class's `__call()` adds none: PHPStan 2.3.5's errors on calls.php at level 10,
// on a PHP without ext-soap
#[test]
fn every_method_exists_on_a_soap_client() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/soap-client");
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
    let mut project = Project::new(vec![root.join("calls.php")], Vec::new());
    project.level = 10;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
