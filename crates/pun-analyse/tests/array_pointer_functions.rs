use std::path::Path;

use pun_analyse::{analyse, reports};
use pun_phpstan::config::{Options, resolve};
use serde_json::Value;

// `end()`, `reset()`, `next()` and `prev()` move the internal pointer and leave the array's type alone (#126):
// PHPStan 2.3.5's errors on pointers.php at level max
#[test]
fn array_pointer_functions_keep_the_array_type() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/array-pointer-functions");
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
