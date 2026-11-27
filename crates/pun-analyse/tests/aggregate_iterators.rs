use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan's ObjectType::getIterableKeyType() and getIterableValueType(): an IteratorAggregate's own getIterator()
// comes before its parent's Traversable template, so a DatePeriod whose getIterator() returns a bare Generator, as
// Carbon\CarbonPeriod's does, iterates `mixed` (#138), while one without its own getIterator() keeps
// DateTimeInterface: PHPStan 2.3.5's errors on iterate.php at level 5
#[test]
fn an_aggregate_iterates_what_its_own_get_iterator_returns() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/aggregate-iterators");
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
    let mut project = Project::new(vec![root.join("iterate.php")], Vec::new());
    project.level = 5;
    let got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| (e.line, e.identifier, e.message))
        .collect();
    assert_eq!(got, want);
}
