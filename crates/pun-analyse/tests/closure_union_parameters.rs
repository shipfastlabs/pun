use std::path::Path;

use pun_analyse::{analyse, reports};
use pun_phpstan::config::{Options, resolve};
use serde_json::Value;

// A closure or callable parameter typed as a union of one generic class's objects accepts every member, and still
// rejects another (#128): PHPStan 2.3.5's errors on fruit.php at level max
#[test]
fn a_closure_parameter_union_of_generics_accepts_each_member() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/closure-union-parameters");
    let root = dir.to_string_lossy().into_owned();
    let config = resolve(&Options {
        config: Some(format!("{root}/phpstan.neon")),
        ..Options::for_project(&root)
    })
    .unwrap();
    let expected: Value =
        serde_json::from_slice(&std::fs::read(dir.join("expected-phpstan.json")).unwrap()).unwrap();
    let mut want: Vec<_> = (expected["errors"].as_array().unwrap().iter())
        .filter(|e| reports(e["identifier"].as_str().unwrap()))
        .map(|e| {
            (
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_owned(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut got: Vec<_> = (analyse(&dir, &config).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    want.sort();
    got.sort();
    assert_eq!(got, want);
}
