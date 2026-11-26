//! phpat/phpat 0.12.5, ported natively. tests/fixtures/phpat is a project whose arch/ classes are phpat tests that use
//! every selector and assertion; expected-phpstan.json is PHPStan 2.3.5's output on src/ at level 0 with
//! `includes: [vendor/phpat/phpat/extension.neon]`, the two test classes registered as services tagged `phpat.test`,
//! and Composer autoloading `App\` from src/ and `Arch\` from arch/ (only the `phpat.*` errors).

use std::path::Path;

use pun_analyse::{Project, run};
use pun_phpstan::{Array, Key, Value as Parameter};
use serde_json::Value;

type Found = Vec<(String, i64, String, String, Option<String>, bool)>;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phpat")
}

fn project(tests: &[&str], options: &[(&str, bool)]) -> Project {
    project_in(&root(), tests, options)
}

fn project_in(root: &Path, tests: &[&str], options: &[(&str, bool)]) -> Project {
    let mut phpat = Array::new();
    phpat.set(
        Key::from("tests"),
        Parameter::Array(Array::list(tests.iter().map(|t| Parameter::str(*t)))),
    );
    for (k, v) in options {
        phpat.set(Key::from(*k), Parameter::Bool(*v));
    }
    let mut parameters = Array::new();
    parameters.set(
        Key::from("fileExtensions"),
        Parameter::Array(Array::list([Parameter::str("php")])),
    );
    parameters.set(
        Key::from("paths"),
        Parameter::Array(Array::list([Parameter::str(
            root.join("src").to_string_lossy().into_owned(),
        )])),
    );
    parameters.set(
        Key::from("scanDirectories"),
        Parameter::Array(Array::list([Parameter::str(
            root.join("arch").to_string_lossy().into_owned(),
        )])),
    );
    parameters.set(Key::from("phpat"), Parameter::Array(phpat));
    Project::from_config(root, &parameters)
}

fn analyse(project: Project) -> Found {
    analyse_in(&root(), project)
}

/// The `phpat.*` errors, with file paths relative to `root`.
fn analyse_in(root: &Path, mut project: Project) -> Found {
    project.load_phpat().unwrap();
    let prefix = format!("{}/", root.display());
    let mut got: Found = run(&project)
        .unwrap()
        .into_iter()
        .filter(|e| e.identifier.starts_with("phpat."))
        .map(|e| {
            let file = e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_string();
            (
                file,
                e.line,
                e.identifier,
                e.message,
                e.tip,
                e.can_be_ignored,
            )
        })
        .collect();
    got.sort();
    got
}

#[test]
fn every_selector_and_assertion() {
    let got = analyse(project(&["Arch\\LayerTest", "Arch\\DeclarationTest"], &[]));
    assert_eq!(
        got,
        oracle(include_str!("fixtures/phpat/expected-phpstan.json"))
    );
}

/// tests/fixtures/phpat-cases: regex selectors, traits a parent or another trait uses, an error in a trait's code
/// (reported in the class's context), `self` and `parent` types, a DNF type, a hooked property, `@method` and
/// `@property` tags, and the code of a function declared in a method. expected-phpstan.json is the output of phpat
/// 0.12.5 on PHPStan 2.3.3, set up as for tests/fixtures/phpat with `Arch\\CasesTest` as the one test class. It shows
/// that phpat doesn't count a first-class callable `Dep::make(...)` or an anonymous class's code.
#[test]
fn more_cases() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phpat-cases");
    let got = analyse_in(&root, project_in(&root, &["Arch\\CasesTest"], &[]));
    assert_eq!(
        got,
        oracle(include_str!("fixtures/phpat-cases/expected-phpstan.json"))
    );
}

fn oracle(json: &str) -> Found {
    let oracle: Value = serde_json::from_str(json).unwrap();
    let mut want: Found = (oracle.as_array().unwrap().iter())
        .map(|e| {
            (
                e["file"].as_str().unwrap().to_string(),
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_string(),
                e["message"].as_str().unwrap().to_string(),
                e["tip"].as_str().map(str::to_string),
                e["ignorable"].as_bool().unwrap(),
            )
        })
        .collect();
    want.sort();
    want
}

/// A project of its own whose phpat test class `Arch\\T` has a `test()` method with `body`, analysing the fixture's
/// src/, and the guard that removes its temp dir.
struct Temp {
    dir: std::path::PathBuf,
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp(body: &str) -> (Temp, Project) {
    let class = format!(
        "<?php\n\nnamespace Arch;\n\nuse App\\Domain\\Handler;\nuse App\\Domain\\Invoice;\nuse PHPat\\Selector\\Selector;\nuse PHPat\\Test\\PHPat;\n\nfinal class T\n{{\n    public function test()\n    {{\n        {body}\n    }}\n}}\n"
    );
    temp_files(&[("arch/T.php", &class)])
}

/// A project of its own with `files` (paths relative to its root; the arch/ ones are scanned), whose phpat test
/// class is `Arch\\T`, analysing the fixture's src/.
fn temp_files(files: &[(&str, &str)]) -> (Temp, Project) {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pun-phpat-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(dir.join("arch")).unwrap();
    for (name, contents) in files {
        std::fs::write(dir.join(name), contents).unwrap();
    }
    let mut parameters = Array::new();
    parameters.set(
        Key::from("fileExtensions"),
        Parameter::Array(Array::list([Parameter::str("php")])),
    );
    parameters.set(
        Key::from("paths"),
        Parameter::Array(Array::list([Parameter::str(
            root().join("src").to_string_lossy().into_owned(),
        )])),
    );
    parameters.set(
        Key::from("scanDirectories"),
        Parameter::Array(Array::list([Parameter::str(
            dir.join("arch").to_string_lossy().into_owned(),
        )])),
    );
    let mut phpat = Array::new();
    phpat.set(
        Key::from("tests"),
        Parameter::Array(Array::list([Parameter::str("Arch\\T")])),
    );
    parameters.set(Key::from("phpat"), Parameter::Array(phpat));
    let mut project = Project::from_config(&dir, &parameters);
    project.scanned = (files.iter())
        .filter(|(name, _)| name.starts_with("arch/"))
        .map(|(name, _)| dir.join(name))
        .collect();
    (Temp { dir }, project)
}

/// The error when Pun can't read the test `body`.
fn load_error(body: &str) -> String {
    let (t, mut project) = temp(body);
    let e = project.load_phpat().unwrap_err();
    e.replace(&format!("{}/", t.dir.display()), "")
}

/// A value only PHP can compute, a method phpat doesn't have, or a rule phpat's validator rejects stops the run with
/// an error that names the code; nothing is skipped.
#[test]
fn unreadable_tests_are_errors() {
    assert_eq!(
        load_error(
            "return PHPat::rule()->classes(Selector::inNamespace(getenv('NS')))->shouldBeFinal();"
        ),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `getenv('NS')` (line 14)"
    );
    assert_eq!(
        load_error("return PHPat::rule()->classes(Selector::all())->shouldBeFancy();"),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `PHPat::rule()->classes(Selector::all())->shouldBeFancy()` (line 14): phpat 0.12.5 has no shouldbefancy() at this point of the rule builder"
    );
    assert_eq!(
        load_error("return PHPat::rule()->classes(Selector::isFancy())->shouldBeFinal();"),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `Selector::isFancy()` (line 14): phpat 0.12.5 has no selector isfancy()"
    );
    assert_eq!(
        load_error(
            "return PHPat::rule()->classes(Selector::appliesAttribute('A', false, ['x' => 1]))->shouldBeFinal();"
        ),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `Selector::appliesAttribute('A', false, ['x' => 1])` (line 14): Pun does not support appliesAttribute()'s $arguments yet"
    );
    assert_eq!(
        load_error("while (true) {}\n        return PHPat::rule();"),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `while (true) {}` (line 14)"
    );
    // RuleValidator's messages
    assert_eq!(
        load_error("return PHPat::rule()->classes(Selector::all())->shouldNotDependOn();"),
        "The PHPat rule Arch\\T:test has no targets"
    );
    assert_eq!(
        load_error("return PHPat::rule()->classes()->shouldBeFinal();"),
        "The PHPat rule Arch\\T:test has no subjects"
    );
    assert_eq!(
        load_error("return PHPat::rule()->classes(Selector::all());"),
        "The PHPat rule Arch\\T:test has no assertion"
    );
}

/// Without `parameters.phpat`, phpat is off: loading reads nothing and fails on nothing.
#[test]
fn off_without_parameters() {
    let mut project = Project::from_config(&root(), &Array::new());
    assert_eq!(project.load_phpat(), Ok(()));
}

#[test]
fn missing_test_class_is_an_error() {
    let mut project = project(&["Arch\\Missing"], &[]);
    assert_eq!(
        project.load_phpat().unwrap_err(),
        "phpat test class Arch\\Missing was not found: Pun looks for it in Composer's autoload (autoload-dev included) and in the analysed and scanned files"
    ); // Composer's autoload maps Arch\\T to arch/T.php, which declares another class.
    let (t, mut project) = temp_files(&[
        (
            "composer.json",
            r#"{"autoload": {"psr-4": {"Arch\\": "arch/"}}}"#,
        ),
        (
            "arch/T.php",
            "<?php\n\nnamespace Arch;\n\nfinal class U\n{\n}\n",
        ),
    ]);
    assert_eq!(
        (project.load_phpat().unwrap_err()).replace(&format!("{}/", t.dir.display()), ""),
        "arch/T.php: phpat test class Arch\\T is not declared there"
    );
}

/// `show_rule_names` prefixes each message with the rule name, and `ignore_built_in_classes` drops PHP's own classes
/// from the names a rule checks: PHPStan 2.3.5 with phpat on arch/OptionsTest.php, `show_rule_names: true`, reports
/// the line below with `ignore_built_in_classes: false` and nothing with `true`. `Stringable` is not reported:
/// `RuntimeException` brings it, so it is no direct interface of `Failure`.
#[test]
fn options() {
    let run = |ignore: bool| {
        analyse(project(
            &["Arch\\OptionsTest"],
            &[
                ("show_rule_names", true),
                ("ignore_built_in_classes", ignore),
            ],
        ))
    };
    assert_eq!(
        run(false),
        [(
            "src/Domain/Failure.php".to_string(),
            5,
            "phpat.testFailureDependencies".to_string(),
            "testFailureDependencies: App\\Domain\\Failure should not depend on RuntimeException"
                .to_string(),
            None,
            true
        )]
    );
    assert_eq!(run(true), []);
}

/// Local array variables, `array_map()` with a Selector factory, the spread of its result, and a `foreach` that
/// appends selectors are read as PHP evaluates them. Each body below makes the subjects `Selector::classname()` of
/// Invoice, Handler and App\Domain\Customer with `shouldBeFinal()`: Handler is final, so phpat reports the other
/// two, at their `class` lines.
#[test]
fn local_arrays_and_array_map() {
    let rule = "PHPat::rule()->classes(...$s)->shouldBeFinal()";
    let classes = "[Invoice::class, Handler::class, 'App\\Domain\\Customer']";
    let want: Found = [
        ("src/Domain/Customer.php", 14, "Customer"),
        ("src/Domain/Invoice.php", 5, "Invoice"),
    ]
    .map(|(file, line, class)| {
        (
            file.to_string(),
            line,
            "phpat.test".to_string(),
            format!("App\\Domain\\{class} should be final"),
            None,
            true,
        )
    })
    .into();
    for body in [
        format!("$c = {classes};\n$s = array_map(Selector::classname(...), $c);\nreturn {rule};"),
        format!(
            "return PHPat::rule()->classes(...array_map(Selector::classname(...), {classes}))->shouldBeFinal();"
        ),
        format!(
            "$s = array_map('PHPat\\Selector\\Selector::classname', {classes});\nreturn {rule};"
        ),
        format!("$s = array_map(fn ($c) => Selector::classname($c), {classes});\nreturn {rule};"),
        format!(
            "$s = array_map(static fn (string $c): \\PHPat\\Selector\\SelectorInterface => Selector::classname($c), {classes});\nreturn {rule};"
        ),
        format!(
            "$s = array_map(function ($c) {{ return Selector::classname($c); }}, {classes});\nreturn {rule};"
        ),
        format!(
            "$s = [];\nforeach ({classes} as $c) {{\n$s[] = Selector::classname($c);\n}}\nreturn {rule};"
        ),
        format!("foreach ({classes} as $i => $c) $s[] = Selector::classname($c);\nreturn {rule};"),
        // String keys, an empty statement, and an argument past phpat's parameters, which PHP ignores
        format!(
            "foreach (['i' => Invoice::class, 'h' => Handler::class, 'c' => 'App\\Domain\\Customer'] as $k => $c) {{\n;\n$s[] = Selector::classname($c, false, 'extra');\n}}\nreturn {rule};"
        ),
        // The arrow function's parameter shadows `$c`, which keeps its value after.
        format!(
            ";\n$c = 'App\\Domain\\Customer';\n$s = array_map(fn ($c) => Selector::classname($c), [Invoice::class, Handler::class]);\n$s[] = Selector::classname($c);\nreturn {rule};"
        ),
        // `array()`, int and numeric-string keys, a repeated key whose last value wins, and `.`
        format!(
            "$s = array(0 => Selector::classname('App\\Nope'), '1' => Selector::classname(Handler::class), 0 => Selector::classname(Invoice::class), Selector::classname('App\\\\' . 'Domain\\\\' . 'Customer'));\nreturn {rule};"
        ),
    ] {
        let (_t, project) = temp(&body);
        assert_eq!(analyse(project), want, "{body}");
    }
    // A `return` after `yield` ends the generator; the yielded rule is `test:0`, so its identifier is `phpat.test0`.
    let (_t, project) = temp(&format!(
        "$s = array_map(Selector::classname(...), {classes});\nyield {rule};\nreturn;"
    ));
    let renamed: Found = (want.iter().cloned())
        .map(|mut e| {
            e.2 = "phpat.test0".to_string();
            e
        })
        .collect();
    assert_eq!(analyse(project), renamed);
}

/// A variable whose value depends on what Pun can't evaluate, or on control flow, stays an error naming the code.
#[test]
fn dynamic_selector_arrays_are_errors() {
    assert_eq!(
        load_error(
            "$c = $this->clients();\n        return PHPat::rule()->classes(...array_map(Selector::classname(...), $c))->shouldBeFinal();"
        ),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `$this` (line 14)"
    );
    assert_eq!(
        load_error(
            "$c = [Invoice::class];\n        if (PHP_OS === 'Linux') { $c = [Handler::class]; }\n        return PHPat::rule()->classes(...array_map(Selector::classname(...), $c))->shouldBeFinal();"
        ),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `if (PHP_OS === 'Linux') { $c = [Handler::class]; }` (line 15)"
    );
    assert_eq!(
        load_error(
            "return PHPat::rule()->classes(...array_map(Selector::classname(...), [$this->c]))->shouldBeFinal();"
        ),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `$this->c` (line 14)"
    );
    assert_eq!(
        load_error(
            "return PHPat::rule()->classes(...array_map(fn ($c) => strtoupper($c), [Invoice::class]))->shouldBeFinal();"
        ),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `strtoupper($c)` (line 14)"
    );
    assert_eq!(
        load_error(
            "foreach ([Invoice::class] as $c) { $s[] = Selector::classname($c); break; }\n        return PHPat::rule()->classes(...$s)->shouldBeFinal();"
        ),
        "arch/T.php:12: phpat test Arch\\T::test(): Pun reads phpat tests without running PHP and can't read `break;` (line 14)"
    );
}

/// `TestParser` lists test methods with `ReflectionClass::getMethods()`, which includes the public methods of the
/// traits a class uses and of the traits those use, and a class's own method wins over a trait's of the same name.
/// `T::test()` makes Invoice final and the traits' tests make Customer and Invoice final; `A::test()`, which `T`
/// overrides, would report Customer under `phpat.test`. Invoice and Customer aren't final, so phpat reports each, with
/// the identifier of the test that checked it. `Base` declares `test()` abstract, which doesn't hide `T`'s.
#[test]
fn test_methods_of_used_traits() {
    let header = "<?php\n\nnamespace Arch;\n\nuse App\\Domain\\Invoice;\nuse PHPat\\Selector\\Selector;\nuse PHPat\\Test\\PHPat;\n\n";
    let rule = |class: &str| {
        format!("return PHPat::rule()->classes(Selector::classname({class}))->shouldBeFinal();")
    };
    let t = format!(
        "{header}final class T extends Base\n{{\n    use A, B;\n\n    public function test()\n    {{\n        {}\n    }}\n}}\n",
        rule("Invoice::class")
    );
    let a = format!(
        "{header}trait A\n{{\n    use B;\n\n    public function test()\n    {{\n        {customer}\n    }}\n\n    public function testFromTrait()\n    {{\n        {customer}\n    }}\n}}\n",
        customer = rule("'App\\Domain\\Customer'")
    );
    let b = format!(
        "{header}trait B\n{{\n    public function testNested()\n    {{\n        {}\n    }}\n}}\n",
        rule("Invoice::class")
    );
    let (_t, project) = temp_files(&[
        ("arch/T.php", &t),
        ("arch/A.php", &a),
        ("arch/B.php", &b),
        (
            "arch/Base.php",
            "<?php\n\nnamespace Arch;\n\nabstract class Base\n{\n    abstract public function test();\n}\n",
        ),
    ]);
    let want: Found = [
        ("src/Domain/Customer.php", 14, "testFromTrait", "Customer"),
        ("src/Domain/Invoice.php", 5, "test", "Invoice"),
        ("src/Domain/Invoice.php", 5, "testNested", "Invoice"),
    ]
    .map(|(file, line, test, class)| {
        (
            file.to_string(),
            line,
            format!("phpat.{test}"),
            format!("App\\Domain\\{class} should be final"),
            None,
            true,
        )
    })
    .into();
    assert_eq!(analyse(project), want);
}

/// A trait Pun can't find, or a `use` whose `insteadof` and `as` adaptations Pun doesn't apply yet, stops the run.
#[test]
fn unreadable_trait_uses_are_errors() {
    let class =
        |uses: &str| format!("<?php\n\nnamespace Arch;\n\nfinal class T\n{{\n    {uses}\n}}\n");
    let error = |files: &[(&str, &str)]| {
        let (t, mut project) = temp_files(files);
        let e = project.load_phpat().unwrap_err();
        e.replace(&format!("{}/", t.dir.display()), "")
    };
    assert_eq!(
        error(&[("arch/T.php", &class("use Missing;"))]),
        "trait Arch\\Missing of phpat test class Arch\\T was not found: Pun looks for it in Composer's autoload (autoload-dev included) and in the analysed and scanned files"
    );
    assert_eq!(
        error(&[
            ("arch/T.php", &class("use A { testA as testB; }")),
            ("arch/A.php", "<?php\n\nnamespace Arch;\n\ntrait A\n{\n}\n"),
        ]),
        "arch/T.php:7: phpat test class Arch\\T: Pun reads phpat tests without running PHP and can't read the trait adaptations in `use A { testA as testB; }` yet"
    );
}

/// Every construct the reader can't evaluate, every value of the wrong kind, and every argument phpat's methods
/// wouldn't accept is an error naming the code. Each body is the test method's, at line 14.
#[test]
fn reader_errors() {
    let at = "arch/T.php:12: phpat test Arch\\T::test(): ";
    let code = |c: &str| {
        format!("{at}Pun reads phpat tests without running PHP and can't read `{c}` (line 14)")
    };
    let returns = |kind: &str| {
        format!("{at}a test method returns a rule builder or an iterable of them, not {kind}")
    };
    let rule =
        |subject: &str| format!("return PHPat::rule()->classes({subject})->shouldBeFinal();");
    let map = |args: &str| rule(&format!("...array_map({args})"));
    let deep = format!("return {}1{};", "(".repeat(70), ")".repeat(70));
    let cases: Vec<(String, String)> = vec![
        ("return 1;".into(), returns("an int")),
        ("return 'a' . 1;".into(), returns("a string")),
        ("return true;".into(), returns("a bool")),
        ("return null;".into(), returns("null")),
        ("return [[]];".into(), returns("an array")),
        ("return Selector::all();".into(), returns("a selector")),
        ("return [1];".into(), returns("an int")),
        ("yield 1;".into(), returns("an int")),
        (
            "$x = 1;".into(),
            format!("{at}a test method returns a rule builder or an iterable of them"),
        ),
        ("strtoupper('x');".into(), code("strtoupper('x');")),
        ("return;".into(), code("return;")),
        ("$a['k'] = 1;".into(), code("$a['k']")),
        (
            "$x = 1;\n        $x[] = 2;".into(),
            code("$x[] = 2").replace("line 14", "line 15"),
        ),
        ("foreach ('x' as $c) {}".into(), code("'x'")),
        ("foreach ([1] as [$a]) {}".into(), code("[$a]")),
        (
            "foreach ([1] as $c) { strtoupper($c); }".into(),
            code("strtoupper($c);"),
        ),
        (
            map("Selector::classname(...)"),
            code("array_map(Selector::classname(...))"),
        ),
        (
            map("Selector::classname(...), ...[[1]]"),
            code("array_map(Selector::classname(...), ...[[1]])"),
        ),
        (
            map("Selector::classname(...), 'x'"),
            code("array_map(Selector::classname(...), 'x')"),
        ),
        (map("'Foo::bar', ['x']"), code("'Foo::bar'")),
        (map("'nope', ['x']"), code("'nope'")),
        (
            map("Selector::isFancy(...), ['x']"),
            code("Selector::isFancy(...)") + ": phpat 0.12.5 has no selector isfancy()",
        ),
        (map("fn ($a, $b) => $a, ['x']"), code("fn ($a, $b) => $a")),
        (map("$x::classname(...), ['x']"), code("$x::classname(...)")),
        (
            map("function ($c) { $d = $c; return Selector::classname($d); }, ['x']"),
            code("function ($c) { $d = $c; return Selector::classname($d); }"),
        ),
        (map("$f, ['x']"), code("$f")),
        ("return [true => PHPat::rule()];".into(), code("true")),
        (deep, code("(((((1)))))")),
        (rule("Selector::classname($x::class)"), code("$x::class")),
        (rule("Selector::classname(self::{'X'})"), code("self::{'X'}")),
        (rule("Selector::classname(Foo::X)"), code("Foo::X")),
        (rule("Selector::classname(self::MISSING)"), code("self::MISSING")),
        (rule("Selector::classname(\"\\u{zz}\")"), code("\"\\u{zz}\"")),
        (
            rule("Selector::classname(0xFFFFFFFFFFFFFFFF)"),
            code("0xFFFFFFFFFFFFFFFF"),
        ),
        (rule("Selector::classname('a' . true)"), code("'a' . true")),
        (rule("Selector::$m()"), code("Selector::$m()")),
        (rule("Foo::bar()"), code("Foo::bar()")),
        (rule("Selector::all()->foo()"), code("Selector::all()->foo()")),
        ("return [...[1]];".into(), code("...[1]")),
        (rule("Selector::allOf(...'x')"), code("...'x'")),
        (
            "return PHPat::rule()->$m();".into(),
            code("PHPat::rule()->$m()"),
        ),
        (
            rule("Selector::classname(Selector::all())"),
            code("Selector::classname(Selector::all())") + ": expected a string, got a selector",
        ),
        (
            rule("Selector::classname('A', 'x')"),
            code("Selector::classname('A', 'x')") + ": expected a bool, got a string",
        ),
        (
            rule("Selector::not(PHPat::rule())"),
            code("Selector::not(PHPat::rule())") + ": expected a selector, got a rule builder",
        ),
        (
            rule("Selector::classname(...['nope' => 'x'])"),
            code("Selector::classname(...['nope' => 'x'])") + ": unknown named argument $nope",
        ),
        (
            rule("Selector::all(1)"),
            code("Selector::all(1)") + ": all() takes no arguments",
        ),
        (
            rule("Selector::atLeastCountOf(Selector::all())"),
            code("Selector::atLeastCountOf(Selector::all())")
                + ": atleastcountof() takes an int count first",
        ),
        (
            rule("Selector::allOf(x: Selector::all())"),
            code("Selector::allOf(x: Selector::all())") + ": unknown named argument $x",
        ),
        (
            rule("Selector::classname('x', fqcn: 'y')"),
            code("Selector::classname('x', fqcn: 'y')") + ": argument $fqcn is given twice",
        ),
        (
            rule("Selector::classname()"),
            code("Selector::classname()") + ": argument $fqcn is missing",
        ),
        (
            "return PHPat::rule()->classes(Selector::all())->shouldBeFinal()->because(tip: 'x');"
                .into(),
            code("PHPat::rule()->classes(Selector::all())->shouldBeFinal()->because(tip: 'x')")
                + ": unknown named argument $tip",
        ),
        (
            "return PHPat::rule()->classes(Selector::all())->should(1);".into(),
            code("PHPat::rule()->classes(Selector::all())->should(1)")
                + ": should() takes no arguments",
        ),
        (
            "return PHPat::rule()->classes(Selector::all())->shouldBeFinal()->because('x')->fooBar();"
                .into(),
            code("PHPat::rule()->classes(Selector::all())->shouldBeFinal()->because('x')->fooBar()")
                + ": phpat 0.12.5 has no foobar() at this point of the rule builder",
        ),
        (
            "return PHPat::rule()->classes(Selector::all())->should()->beFancy();".into(),
            code("PHPat::rule()->classes(Selector::all())->should()->beFancy()")
                + ": phpat 0.12.5 has no befancy() at this point of the rule builder",
        ),
        // Code longer than 80 characters is cut to its first 77 and `...`.
        (
            "return PHPat::rule()->classes(Selector::all())->shouldBeFinal()->because('a long tip')->fooBar();"
                .into(),
            code("PHPat::rule()->classes(Selector::all())->shouldBeFinal()->because('a long tip...")
                + ": phpat 0.12.5 has no foobar() at this point of the rule builder",
        ),
    ];
    for (body, want) in cases {
        assert_eq!(load_error(&body), want, "{body}");
    }
}

/// A `should` relation names each target it misses by the selector's `getName()` (phpat 0.12.5's `Selector\*` and
/// `Selector\Modifier\*` classes). Invoice extends no class, so it misses every target.
#[test]
fn target_selector_names() {
    let a = "Selector::classname('A')";
    let b = "Selector::classname('B')";
    let targets: [(String, &str); 18] = [
        ("Selector::all()".into(), "-all classes-"),
        ("Selector::isException()".into(), "-all exceptions-"),
        ("Selector::isThrowable()".into(), "-all throwables-"),
        ("Selector::isError()".into(), "-all errors-"),
        ("Selector::isInterface()".into(), "-all interfaces-"),
        ("Selector::isAbstract()".into(), "-abstract classes-"),
        ("Selector::isFinal()".into(), "-all final classes-"),
        ("Selector::isTrait()".into(), "-all traits-"),
        ("Selector::isReadonly()".into(), "-all readonly classes-"),
        ("Selector::isEnum()".into(), "-all enums-"),
        ("Selector::isAttribute()".into(), "-attribute classes-"),
        ("Selector::isStandardClass()".into(), "-standard classes-"),
        (format!("Selector::allOf({a}, {b})"), "A and B"),
        (format!("Selector::anyOf({a}, {b})"), "A or B"),
        (format!("Selector::noneOf({a}, {b})"), "none of: A and B"),
        (
            format!("Selector::atLeastCountOf(1, {a}, {b})"),
            "at least 1 of A and B",
        ),
        (
            format!("Selector::atMostCountOf(1, {a}, {b})"),
            "at most 1 of A and B",
        ),
        (format!("Selector::oneOf({a}, {b})"), "one of: [ A and B ]"),
    ];
    let list = (targets.iter().map(|(t, _)| t.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let (_t, project) = temp(&format!(
        "return PHPat::rule()->classes(Selector::classname(Invoice::class))->shouldExtend()->classes({list});"
    ));
    let mut want: Found = (targets.iter())
        .map(|(_, name)| {
            (
                "src/Domain/Invoice.php".to_string(),
                5,
                "phpat.test".to_string(),
                format!("App\\Domain\\Invoice should extend {name}"),
                None,
                true,
            )
        })
        .collect();
    want.sort();
    assert_eq!(analyse(project), want);
}
