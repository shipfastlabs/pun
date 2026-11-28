use std::path::Path;

use pun_analyse::{Project, run};
use serde_json::Value;

// PHPStan types `getTimezone()` of DateTime, DateTimeImmutable and DateTimeInterface as `DateTimeZone`, not the native
// `DateTimeZone|false` (#180): PHPStan 2.3.5's errors on timezone.php at level 7
#[test]
fn date_time_get_timezone_is_never_false() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/date-time-zones");
    let oracle: Value =
        serde_json::from_slice(&std::fs::read(root.join("expected-phpstan.json")).unwrap())
            .unwrap();
    let mut want: Vec<_> = (oracle["errors"].as_array().unwrap().iter())
        .map(|e| {
            (
                e["file"].as_str().unwrap().to_owned(),
                e["line"].as_i64().unwrap(),
                e["identifier"].as_str().unwrap().to_owned(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let mut project = Project::new(vec![root.join("timezone.php")], Vec::new());
    project.level = 7;
    let prefix = format!("{}/", root.display());
    let mut got: Vec<_> = (run(&project).unwrap().into_iter())
        .map(|e| {
            let file = e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_owned();
            (file, e.line, e.identifier, e.message)
        })
        .collect();
    want.sort();
    got.sort();
    assert_eq!(got, want);
}
