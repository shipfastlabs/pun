use std::path::Path;

use pun_analyse::{Project, analyse, reports, run};
use pun_phpstan::config::{Options, resolve};
use serde_json::Value;

type Key = (String, i64, String, String, Option<String>);

fn check(neon: &str) {
    check_in("statements", neon);
}

fn check_in(fixtures: &str, neon: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixtures);
    let dir = root.to_string_lossy().into_owned();
    let options = Options {
        config: Some(format!("{dir}/{neon}.neon")),
        ..Options::for_project(&dir)
    };
    let config = resolve(&options).unwrap_or_else(|e| panic!("{e}"));
    let prefix = format!("{dir}/");
    let mut got: Vec<Key> = analyse(&root, &config)
        .unwrap()
        .into_iter()
        .map(|e| {
            let file = e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_string();
            (file, e.line, e.identifier, e.message, e.tip)
        })
        .collect();
    got.sort();
    let expected: Value =
        serde_json::from_slice(&std::fs::read(root.join(format!("expected-{neon}.json"))).unwrap())
            .unwrap();
    let mut want: Vec<Key> = expected["errors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| reports(e["identifier"].as_str().unwrap_or("")))
        .map(|e| {
            (
                e["file"].as_str().unwrap().to_string(),
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_string(),
                e["message"].as_str().unwrap().to_string(),
                e["tip"].as_str().map(str::to_string),
            )
        })
        .collect();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn php_8_5() {
    check("phpstan");
}

#[test]
fn builtins_follow_php_version() {
    check_in("builtins", "phpstan");
    check_in("builtins", "phpstan-php84");
}

#[test]
fn php_7_4() {
    check("phpstan-php74");
}

#[test]
fn void_cast_before_php_8_5() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/rules/data/Cast/data/void-cast-support.php");
    let mut project = Project::new(vec![file], Vec::new());
    project.php_version = pun_analyse::PhpVersion::from_id(80400);
    let got: Vec<_> = run(&project)
        .unwrap()
        .into_iter()
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(
        got,
        [(
            10,
            "cast.voidNotSupported".to_string(),
            "The (void) cast is supported only on PHP 8.5 and later.".to_string()
        )]
    );
}

#[test]
fn declarations_php_7_4() {
    check_in("declarations", "phpstan-70400");
}

#[test]
fn type_aliases() {
    check_in("type_aliases", "phpstan");
}

#[test]
fn declarations_php_8_0() {
    check_in("declarations", "phpstan-80000");
}

#[test]
fn declarations_php_8_1() {
    check_in("declarations", "phpstan-80100");
}

#[test]
fn declarations_php_8_2() {
    check_in("declarations", "phpstan-80200");
}

#[test]
fn declarations_php_8_5() {
    check_in("declarations", "phpstan-80500");
}

#[test]
fn declarations_readonly_php_8_0() {
    check_in("declarations", "phpstan-80000-readonly");
}

#[test]
fn anonymous_classes() {
    check_in("anonymous", "phpstan");
}

#[test]
fn property_access() {
    check_in("properties", "phpstan");
}

#[test]
fn property_access_php_8_1() {
    check_in("properties", "phpstan-81");
}

#[test]
fn static_property_expression_names_original() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "tests/fixtures/rules/data/Properties/data/properties-from-array-into-static-object.php",
    );
    let got: Vec<_> = run(&Project::new(vec![file], Vec::new()))
        .unwrap()
        .into_iter()
        .map(|e| (e.line, e.identifier, e.message, e.tip))
        .collect();
    assert_eq!(
        got,
        [(
            29,
            "staticProperty.notFound".to_string(),
            "Access to an undefined static property PropertiesFromArrayIntoStaticObject\\Foo::$noop."
                .to_string(),
            None,
        )]
    );
}

#[test]
fn static_property_expression_names() {
    check_in("static-property-names", "phpstan");
    check_in("static-property-names", "phpstan-level2");
}

#[test]
fn static_property_expression_name_boundaries() {
    check_in("static-property-names", "phpstan-boundaries");
}

#[test]
fn static_property_operand_reads() {
    check_in("static-property-names", "phpstan-operands");
    check_in("static-property-names", "phpstan-operands-level8");
    check_in("static-property-names", "phpstan-operands-level10");
}

/// A failed instance property read is ErrorType, not null, so a guard on it is neither always true nor followed by
/// unreachable code. A real `null` property keeps both findings; `__get` with a `@property-read` reads the tag.
#[test]
fn instance_property_failed_reads() {
    check_in("instance-property-reads", "phpstan");
}

#[test]
fn static_property_increment_writes() {
    check_in("static-property-names", "phpstan-writes");
    check_in("static-property-names", "phpstan-writes-level8");
    check_in("static-property-names", "phpstan-writes-level10");
}

#[test]
fn hierarchy() {
    check_in("hierarchy", "phpstan");
}

#[test]
fn through_static() {
    check_in("through-static", "phpstan");
}

#[test]
fn internal_methods() {
    check_in("internal", "phpstan");
}

#[test]
fn internal_functions() {
    check_in("restricted_functions", "phpstan");
}
