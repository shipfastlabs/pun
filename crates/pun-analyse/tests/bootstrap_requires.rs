//! PHPStan runs its bootstrap files, so the functions of the files they require are known. tests/fixtures/
//! bootstrap-requires has a bootstrap file that requires `__DIR__ . '/libs/helpers.php'`, which requires
//! `'shout.php'` (a path PHP finds in the including file's directory); expected-phpstan.json is PHPStan 2.3.5's output
//! at level 7 with `bootstrapFiles: [bootstrap.php]`.

use std::path::Path;

use pun_analyse::{Project, run};
use pun_phpstan::{Array, Key, Value as Parameter};
use serde_json::Value;

#[test]
fn bootstrap_requires_define_functions() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bootstrap-requires");
    let path = |p: &str| Parameter::str(root.join(p).to_string_lossy().into_owned());
    let mut parameters = Array::new();
    parameters.set(Key::from("level"), Parameter::Int(7));
    parameters.set(
        Key::from("fileExtensions"),
        Parameter::Array(Array::list([Parameter::str("php")])),
    );
    parameters.set(
        Key::from("paths"),
        Parameter::Array(Array::list([path("src")])),
    );
    parameters.set(
        Key::from("bootstrapFiles"),
        Parameter::Array(Array::list([path("bootstrap.php")])),
    );
    let project = Project::from_config(&root, &parameters);
    let prefix = format!("{}/", root.display());
    let mut got: Vec<(String, i64, String, String)> = run(&project)
        .unwrap()
        .into_iter()
        .map(|e| {
            let file = e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_string();
            (file, e.line, e.identifier, e.message)
        })
        .collect();
    got.sort();
    let oracle: Value = serde_json::from_str(include_str!(
        "fixtures/bootstrap-requires/expected-phpstan.json"
    ))
    .unwrap();
    let mut want: Vec<(String, i64, String, String)> = (oracle.as_array().unwrap().iter())
        .map(|e| {
            (
                e["file"].as_str().unwrap().to_string(),
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_string(),
                e["message"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    want.sort();
    assert_eq!(got, want);
}
