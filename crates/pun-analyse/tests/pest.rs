//! pestphp/pest-plugin-phpstan's `TestClosureThisTypeExtension`, `TestCaseDynamicPropertyTypeExtension` and
//! `ProtectedMethodCallIgnoreExtension`, with `universalObjectCratesClasses: [PHPUnit\Framework\TestCase]`.
//! tests/fixtures/pest binds `Tests\TestCase` to `tests/Feature` in tests/Pest.php and leaves `tests/Unit` unbound.
//! expected-phpstan.json is PHPStan 2.2.14's output at level 9 with pest-plugin-phpstan 1.1's extension.neon, on the
//! fixture's tests with the real pestphp/pest 4.1 and phpunit/phpunit 12; Pun reads their declarations from the
//! trimmed vendor/pest.php.

use std::path::Path;

use pun_analyse::{Project, run};
use pun_phpstan::{Array, Key, Value as Parameter};
use serde_json::Value;

type Found = Vec<(String, i64, String, String)>;

fn analyse(extension: bool) -> Found {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pest");
    let path = |p: &str| Parameter::str(root.join(p).to_string_lossy().into_owned());
    let mut parameters = Array::new();
    parameters.set(Key::from("level"), Parameter::Int(9));
    parameters.set(
        Key::from("fileExtensions"),
        Parameter::Array(Array::list([Parameter::str("php")])),
    );
    parameters.set(
        Key::from("paths"),
        Parameter::Array(Array::list([path("tests/Feature"), path("tests/Unit")])),
    );
    parameters.set(
        Key::from("scanFiles"),
        Parameter::Array(Array::list([
            path("vendor/pest.php"),
            path("tests/TestCase.php"),
        ])),
    );
    if extension {
        let mut pest = Array::new();
        pest.set(Key::from("closureThis"), Parameter::Bool(true));
        parameters.set(Key::from("pest"), Parameter::Array(pest));
    }
    let project = Project::from_config(&root, &parameters);
    let prefix = format!("{}/", root.display());
    let mut got: Found = run(&project)
        .unwrap()
        .into_iter()
        .map(|e| {
            let file = e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_string();
            (file, e.line, e.identifier, e.message)
        })
        .collect();
    got.sort();
    got
}

#[test]
fn the_test_case_bound_in_pest_php_types_this_and_what_before_each_assigns() {
    let oracle: Value =
        serde_json::from_str(include_str!("fixtures/pest/expected-phpstan.json")).unwrap();
    let mut want: Found = (oracle["errors"].as_array().unwrap().iter())
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
    assert_eq!(analyse(true), want);
}

#[test]
fn without_the_extension_this_is_pests_test_call() {
    let got = analyse(false);
    assert!(
        got.iter().any(
            |(file, line, _, message)| file == "tests/Feature/BoundTest.php"
                && *line == 11
                && message == "Dumped type: Pest\\PendingCalls\\TestCall"
        ),
        "{got:#?}"
    );
}
