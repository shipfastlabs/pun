use std::fmt::Write as _;
use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

fn errors(project: &Project) -> Vec<(i64, String, String, Option<String>)> {
    run(project)
        .unwrap()
        .into_iter()
        .map(|e| (e.line, e.identifier, e.message, e.tip))
        .collect()
}

// Literals above ConstantArrayTypeBuilder::ARRAY_COUNT_LIMIT are generalized
// oversized arrays, in their descriptions and in what they are accepted by.
#[test]
fn oversized_literals_match_phpstan() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/oversized-arrays");
    let expected: Value =
        serde_json::from_slice(&std::fs::read(root.join("expected-phpstan.json")).unwrap())
            .unwrap();
    let want: Vec<_> = expected["files"]["oversized.php"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_string(),
                e["message"].as_str().unwrap().to_string(),
                e["tip"].as_str().map(str::to_string),
            )
        })
        .collect();
    let mut project = Project::new(vec![root.join("oversized.php")], Vec::new());
    project.level = 9;
    assert_eq!(errors(&project), want);
}

fn check_rows(rows: usize) -> std::time::Duration {
    let mut source = String::from(
        "<?php\n\nfinal class Data\n{\n    /** @var array<string, list<array<int, int|string|null>>> */\n    public const array TABLES = ['rows' => [\n",
    );
    for i in 0..rows {
        writeln!(source, "['id-{i:06}', 'a', {i}, null],").unwrap();
    }
    source.push_str("]];\n\n    /** @return array<string, list<array<int, int|string|null>>> */\n    public static function tables(): array\n    {\n        return [...self::TABLES];\n    }\n}\n");
    let file = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("oversized-{rows}-rows.php"));
    std::fs::write(&file, source).unwrap();
    let mut project = Project::new(vec![file], Vec::new());
    project.level = 9;
    let started = std::time::Instant::now();
    assert_eq!(errors(&project), Vec::new());
    started.elapsed()
}

// Issue #61: the check of a large literal against its PHPDoc type grew with the
// cube of the row count (4,000 rows took about 38 s). Compare two sizes instead
// of a wall-clock bound, which depends on the machine and the opt level: four
// times the rows take about 2-4 times as long now, and up to 64 times as long
// with the cubic check.
#[test]
fn large_literal_checked_against_phpdoc_stays_linear() {
    let small = check_rows(1000);
    let large = check_rows(4000);
    assert!(
        large < small * 10,
        "1000 rows: {small:?}, 4000 rows: {large:?}"
    );
}
