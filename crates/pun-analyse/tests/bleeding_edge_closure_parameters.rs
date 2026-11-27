use std::path::Path;

use pun_analyse::{analyse, reports};
use pun_phpstan::config::{Options, resolve};
use serde_json::Value;

// With bleeding edge, a closure written where nothing types it takes its parameter types from its invocations: a
// default joins them (`?Box $box = null` stays nullable), an argument read from an offset is bound, not possibly
// undefined, and a closure sent to a built-in function takes PHPStan's parameter types (#123): PHPStan 2.3.5's
// errors on shelf.php at level max with bleedingEdge.neon
#[test]
fn bleeding_edge_closure_parameters_keep_their_declared_defaults_and_callable_targets() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/bleeding-edge-closure-parameters");
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
