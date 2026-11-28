//! The ports of PHP's `filter_var(FILTER_VALIDATE_EMAIL)` and `parse_url` against PHP 8.5's own answers
//! (`fixtures/php-filters.json`, from `scripts/export-php-filters.php`).

use pun_packages::php::{UrlParts, is_email, is_email_unicode, parse_url};
use serde_json::Value;

const FIXTURE: &str = include_str!("fixtures/php-filters.json");

#[test]
fn emails_as_php_validates_them() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let wrong: Vec<String> = (fixture["email"].as_object().unwrap().iter())
        .filter(|(email, valid)| is_email(email) != valid.as_bool().unwrap())
        .map(|(email, valid)| format!("{email:?}: PHP says {valid}"))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn unicode_emails_as_php_validates_them() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let wrong: Vec<String> = (fixture["email_unicode"].as_object().unwrap().iter())
        .filter(|(email, valid)| is_email_unicode(email) != valid.as_bool().unwrap())
        .map(|(email, valid)| format!("{email:?}: PHP says {valid}"))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn urls_as_php_parses_them() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let wrong: Vec<String> = (fixture["url"].as_object().unwrap().iter())
        .filter_map(|(url, want)| {
            let want = want.as_object().map(|bits| UrlParts {
                scheme: bits["scheme"].as_str().map(String::from),
                host: bits["host"].as_str().map(String::from),
            });
            let got = parse_url(url);
            (got != want).then(|| format!("{url:?}: PHP gives {want:?}, the port {got:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
