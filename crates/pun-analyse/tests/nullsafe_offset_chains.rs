use std::path::Path;

use pun_analyse::{analyse, reports};
use pun_phpstan::config::{Options, resolve};
use serde_json::Value;

// An offset in a nullsafe chain is null only where the chain short-circuited, so the next call may skip it; a
// nullable value still fails (#129): PHPStan 2.3.5's errors on mailbox.php at level max
#[test]
fn an_offset_in_a_nullsafe_chain_keeps_the_short_circuit() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/nullsafe-offset-chains");
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
