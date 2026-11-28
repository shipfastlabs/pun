//! The result cache of `pun analyse`: a later run analyses again only the files whose contents changed or that read
//! something that changed, and its output is the output of a run without the cache (`--no-cache`), which is the
//! oracle here. `PUN_CACHE_STATS` makes a run print how many files it analysed and reused. Each test's cache directory
//! is its own (`common::clean_env`).

mod common;

use common::{PUN, clean_env, project};
use std::path::Path;
use std::process::Command;

fn pun(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (String, String, Option<i32>) {
    let out = clean_env(&mut Command::new(PUN), &["PATH", "HOME"])
        .current_dir(dir)
        .env("PUN_AGENT", "0")
        .env("NO_COLOR", "1")
        .env("PUN_CACHE_STATS", "1")
        .envs(env.iter().copied())
        .args(args)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code(),
    )
}

/// What the run reported about the cache: (unchanged, analysed, reused), or none when it did not use it.
fn stats(stderr: &str) -> Option<(bool, usize, usize)> {
    let line = stderr.lines().find(|l| l.starts_with("analyse cache: "))?;
    let mut parts = line.trim_start_matches("analyse cache: ").split(' ');
    let mut next = || parts.next()?.split_once('=').map(|(_, v)| v.to_string());
    Some((
        next()? == "true",
        next()?.parse().ok()?,
        next()?.parse().ok()?,
    ))
}

/// A cached run and a run without the cache, which must print the same and exit alike; the cached run's stats.
fn analyse(dir: &Path) -> (String, Option<(bool, usize, usize)>) {
    let (out, err, code) = pun(dir, &["analyse", "--format=json"], &[]);
    let (want, _, want_code) = pun(dir, &["analyse", "--format=json", "--no-cache"], &[]);
    assert_eq!((&out, code), (&want, want_code), "{err}");
    (out, stats(&err))
}

const CONFIG: &str = "[analyse]\npreset = \"phpstan/level_5\"\npaths = [\"src\"]\n";

const CALLER: &str =
    "<?php\n\nnamespace App;\n\nfunction caller(B $b): int\n{\n    return $b->size();\n}\n";
const CALLEE: &str = "<?php\n\nnamespace App;\n\nclass B\n{\n    public function size(): int\n    {\n        return 1;\n    }\n}\n";
const OTHER: &str = "<?php\n\nnamespace App;\n\nfunction other(): int\n{\n    return 2;\n}\n";

fn sample(name: &str) -> std::path::PathBuf {
    project(
        "analyse_cache",
        name,
        &[
            ("pun.toml", CONFIG),
            ("src/caller.php", CALLER),
            ("src/B.php", CALLEE),
            ("src/other.php", OTHER),
        ],
    )
}

#[test]
fn an_unchanged_project_reuses_the_whole_analysis() {
    let d = sample("unchanged");
    let (first, stats1) = analyse(&d);
    assert_eq!(stats1, Some((false, 3, 0)));
    let (second, stats2) = analyse(&d);
    assert_eq!(stats2, Some((true, 0, 3)));
    assert_eq!(first, second);
}

/// A body is no other file's business: only the edited file is analysed again, and its new error shows.
#[test]
fn editing_a_method_body_analyses_only_that_file() {
    let d = sample("body");
    analyse(&d);
    std::fs::write(
        d.join("src/B.php"),
        CALLEE.replace("return 1;", "return 'one';"),
    )
    .unwrap();
    let (out, stats) = analyse(&d);
    assert_eq!(stats, Some((false, 1, 2)));
    assert!(
        out.contains("should return int but returns string"),
        "{out}"
    );
}

/// The caller reads `B::size()`'s return type: changing it analyses the caller again, which now returns a string.
#[test]
fn changing_a_signature_analyses_the_files_that_read_it() {
    let d = sample("signature");
    analyse(&d);
    std::fs::write(
        d.join("src/B.php"),
        CALLEE
            .replace("size(): int", "size(): string")
            .replace("return 1;", "return 'one';"),
    )
    .unwrap();
    let (out, stats) = analyse(&d);
    assert_eq!(stats, Some((false, 2, 1)));
    assert!(
        out.contains("caller() should return int but returns string"),
        "{out}"
    );
}

/// A class nobody declared is looked up and missed; declaring it in a new file makes the error go.
#[test]
fn declaring_a_missing_class_analyses_its_users_again() {
    let d = sample("missing");
    let user =
        "<?php\n\nnamespace App;\n\nfunction make(): object\n{\n    return new Later();\n}\n";
    std::fs::write(d.join("src/user.php"), user).unwrap();
    let (out, _) = analyse(&d);
    assert!(out.contains("App\\\\Later not found"), "{out}");
    std::fs::write(
        d.join("src/Later.php"),
        "<?php\n\nnamespace App;\n\nclass Later\n{\n}\n",
    )
    .unwrap();
    let (out, stats) = analyse(&d);
    assert!(!out.contains("Later not found"), "{out}");
    assert_eq!(stats.map(|(unchanged, ..)| unchanged), Some(false));
}

/// Deleting the declaration a caller reads brings the caller's error back.
#[test]
fn deleting_a_declaration_analyses_its_users_again() {
    let d = sample("delete");
    analyse(&d);
    std::fs::remove_file(d.join("src/B.php")).unwrap();
    let (out, _) = analyse(&d);
    assert!(out.contains("unknown class App\\\\B"), "{out}");
}

/// A match over an enum is exhaustive until a case is added in the enum's file.
#[test]
fn adding_an_enum_case_analyses_the_matches_over_it() {
    let d = project(
        "analyse_cache",
        "enum",
        &[
            ("pun.toml", CONFIG),
            (
                "src/Suit.php",
                "<?php\n\nnamespace App;\n\nenum Suit\n{\n    case Hearts;\n    case Spades;\n}\n",
            ),
            (
                "src/name.php",
                "<?php\n\nnamespace App;\n\nfunction name(Suit $s): string\n{\n    return match ($s) {\n        Suit::Hearts => 'h',\n        Suit::Spades => 's',\n    };\n}\n",
            ),
        ],
    );
    let (out, _) = analyse(&d);
    assert!(!out.contains("unhandled"), "{out}");
    let suit = std::fs::read_to_string(d.join("src/Suit.php")).unwrap();
    std::fs::write(
        d.join("src/Suit.php"),
        suit.replace("case Spades;", "case Spades;\n    case Clubs;"),
    )
    .unwrap();
    let (out, stats) = analyse(&d);
    assert!(out.contains("Clubs"), "{out}");
    assert_eq!(stats, Some((false, 2, 0)));
}

/// The config is part of the key: another level is another analysis.
#[test]
fn another_config_analyses_every_file() {
    let d = sample("config");
    analyse(&d);
    std::fs::write(d.join("pun.toml"), CONFIG.replace("level_5", "level_6")).unwrap();
    let (_, stats) = analyse(&d);
    assert_eq!(stats, Some((false, 3, 0)));
}

#[test]
fn no_cache_reads_and_writes_nothing() {
    let d = sample("off");
    for env in [&[][..], &[("PUN_NO_CACHE", "1")][..]] {
        let args: &[&str] = if env.is_empty() {
            &["analyse", "--no-cache"]
        } else {
            &["analyse"]
        };
        let (_, err, _) = pun(&d, args, env);
        assert_eq!(stats(&err), None, "{err}");
    }
    let (_, err, _) = pun(&d, &["analyse"], &[]);
    assert_eq!(stats(&err), Some((false, 3, 0)));
}

/// A cache file that is not a record (truncated, from another version) is an empty cache.
#[test]
fn an_unreadable_cache_file_is_an_empty_cache() {
    let d = sample("corrupt");
    analyse(&d);
    let dir = common::test_cache_dir().join("analyse");
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        let bytes = std::fs::read(entry.path()).unwrap();
        std::fs::write(entry.path(), &bytes[..bytes.len() / 2]).unwrap();
    }
    let (_, stats) = analyse(&d);
    assert_eq!(stats, Some((false, 3, 0)));
}

const WITH_SCAN: &str =
    "[analyse]\npreset = \"phpstan/level_5\"\npaths = [\"src\"]\nscan = [\"lib\"]\n";

/// A scanned file (outside `paths`, read for its symbols) is a source too: editing it reaches the files that use it,
/// on the fast path as well.
#[test]
fn editing_a_scanned_file_analyses_its_users_again() {
    let d = project(
        "analyse_cache",
        "scanned",
        &[
            ("pun.toml", WITH_SCAN),
            (
                "lib/Box.php",
                "<?php\n\nnamespace Lib;\n\nclass Box\n{\n    public function size(): int\n    {\n        return 1;\n    }\n}\n",
            ),
            (
                "src/use.php",
                "<?php\n\nnamespace App;\n\nfunction f(\\Lib\\Box $b): int\n{\n    return $b->size();\n}\n",
            ),
        ],
    );
    analyse(&d);
    let path = d.join("lib/Box.php");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source
            .replace("size(): int", "size(): string")
            .replace("return 1;", "return 'x';"),
    )
    .unwrap();
    let (out, _) = analyse(&d);
    assert!(
        out.contains("f() should return int but returns string"),
        "{out}"
    );
}

/// A `require` of a file that is missing reports it; creating the file makes the error go, on the fast path too.
#[test]
fn creating_a_required_file_analyses_the_requirer_again() {
    let d = project("analyse_cache", "require", &[("pun.toml", CONFIG)]);
    let missing = d.join("lib/missing.php");
    let requirer = format!("<?php\n\nrequire '{}';\n", missing.display());
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(d.join("src/inc.php"), requirer).unwrap();
    let (out, _) = analyse(&d);
    assert!(out.contains("require.fileNotFound"), "{out}");
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::write(&missing, "<?php\n").unwrap();
    let (out, _) = analyse(&d);
    assert!(!out.contains("require.fileNotFound"), "{out}");
}

/// A file reused from the cache keeps the paths it read in the new record: creating the file it requires, after a
/// run that reused it, still analyses it again.
#[test]
fn a_reused_file_keeps_the_paths_it_read() {
    let d = project("analyse_cache", "require_reused", &[("pun.toml", CONFIG)]);
    let missing = d.join("lib/missing.php");
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(
        d.join("src/inc.php"),
        format!("<?php\n\nrequire '{}';\n", missing.display()),
    )
    .unwrap();
    std::fs::write(d.join("src/other.php"), OTHER).unwrap();
    let (out, _) = analyse(&d);
    assert!(out.contains("require.fileNotFound"), "{out}");
    std::fs::write(d.join("src/other.php"), OTHER.replace("2;", "3;")).unwrap();
    let (out, stats) = analyse(&d);
    assert!(out.contains("require.fileNotFound"), "{out}");
    assert_eq!(stats, Some((false, 1, 1)));
    std::fs::create_dir_all(d.join("lib")).unwrap();
    std::fs::write(&missing, "<?php\n").unwrap();
    let (out, _) = analyse(&d);
    assert!(!out.contains("require.fileNotFound"), "{out}");
}

/// A file that read a class's descendants (`method_exists()` narrows to them) is checked against them: reused while
/// another file changes, analysed again once a new implementation appears.
#[test]
fn descendants_a_file_read_are_checked_again() {
    let d = project(
        "analyse_cache",
        "descendants",
        &[
            ("pun.toml", CONFIG),
            (
                "src/B.php",
                "<?php\n\nnamespace App;\n\ninterface Shape\n{\n}\n\nfunction g(Shape $s): int\n{\n    if (method_exists($s, 'area')) {\n        return $s->area();\n    }\n    return 0;\n}\n",
            ),
            (
                "src/C.php",
                "<?php\n\nnamespace App;\n\nclass Other\n{\n}\n",
            ),
            ("src/D.php", OTHER),
        ],
    );
    analyse(&d);
    std::fs::write(d.join("src/D.php"), OTHER.replace("2;", "3;")).unwrap();
    let (_, stats) = analyse(&d);
    assert_eq!(stats, Some((false, 1, 2)));
    std::fs::write(
        d.join("src/C.php"),
        "<?php\n\nnamespace App;\n\nfinal class Circle implements Shape\n{\n    public function area(): string\n    {\n        return 'x';\n    }\n}\n",
    )
    .unwrap();
    let (_, stats) = analyse(&d);
    assert_eq!(stats, Some((false, 2, 1)));
}

/// A config file that is a symbolic link counts with what it points to: pointing it at another file is seen.
#[test]
fn a_symlinked_config_file_pointed_elsewhere_is_seen() {
    let d = laravel(
        "config_symlink",
        &[
            ("ext/scalar.php", "<?php return ['value' => 1];\n"),
            ("ext/array.php", "<?php return ['value' => []];\n"),
            (
                "src/Use.php",
                "<?php\nnamespace App;\nuse Illuminate\\Support\\Facades\\Config;\nfunction f(): void { Config::collection('settings.value'); }\n",
            ),
        ],
    );
    std::fs::create_dir_all(d.join("config")).unwrap();
    std::os::unix::fs::symlink(d.join("ext/scalar.php"), d.join("config/settings.php")).unwrap();
    let (out, _) = analyse(&d);
    assert!(out.contains("is not an array"), "{out}");
    std::fs::remove_file(d.join("config/settings.php")).unwrap();
    std::os::unix::fs::symlink(d.join("ext/array.php"), d.join("config/settings.php")).unwrap();
    let (out, _) = analyse(&d);
    assert!(!out.contains("is not an array"), "{out}");
}

/// A file Composer autoloads (`autoload.files`) is a source the project setup also asks about: editing its body
/// analyses its callers again and the setup does not keep a stale answer.
#[test]
fn an_autoloaded_file_edited_is_analysed_again() {
    let helper = "<?php\n\nfunction helper(): int\n{\n    return 1;\n}\n";
    let d = project(
        "analyse_cache",
        "autoload_files",
        &[
            ("pun.toml", CONFIG),
            (
                "composer.json",
                "{\"autoload\":{\"files\":[\"src/helpers.php\"]}}",
            ),
            ("src/helpers.php", helper),
            (
                "src/use.php",
                "<?php\n\nfunction caller(): string\n{\n    return helper();\n}\n",
            ),
        ],
    );
    let (out, _) = analyse(&d);
    assert!(
        out.contains("caller() should return string but returns int"),
        "{out}"
    );
    std::fs::write(
        d.join("src/helpers.php"),
        helper
            .replace("(): int", "(): string")
            .replace("1;", "'a';"),
    )
    .unwrap();
    let (out, _) = analyse(&d);
    assert!(!out.contains("caller()"), "{out}");
}

/// A file rewritten with the same size and its old modification time put back still counts as changed: the stat
/// holds the change time and the inode.
#[test]
fn a_rewrite_that_keeps_size_and_modification_time_is_seen() {
    let d = sample("restored_mtime");
    let path = d.join("src/B.php");
    // old enough that the run recording it does not take it as changed during the run
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(5);
    let set_old = || {
        (std::fs::File::options().write(true).open(&path))
            .and_then(|f| f.set_modified(old))
            .unwrap();
    };
    set_old();
    analyse(&d);
    let changed = CALLEE.replace("return 1;", "return'';");
    assert_eq!(changed.len(), CALLEE.len());
    std::fs::write(&path, changed).unwrap();
    set_old();
    let (out, _) = analyse(&d);
    assert!(
        out.contains("should return int but returns string"),
        "{out}"
    );
}

/// An array constant's key order decides whether it is a list: reordering the keys reaches its users.
#[test]
fn reordering_a_constant_array_analyses_its_users_again() {
    let d = project(
        "analyse_cache",
        "key_order",
        &[
            ("pun.toml", CONFIG),
            (
                "src/C.php",
                "<?php\n\nnamespace App;\n\nclass C\n{\n    const ITEMS = [0 => 1, 1 => 2];\n}\n",
            ),
            (
                "src/use.php",
                "<?php\n\nnamespace App;\n\n/** @return list<int> */\nfunction f(): array\n{\n    return C::ITEMS;\n}\n",
            ),
        ],
    );
    let (out, _) = analyse(&d);
    assert!(!out.contains("should return list"), "{out}");
    std::fs::write(
        d.join("src/C.php"),
        "<?php\n\nnamespace App;\n\nclass C\n{\n    const ITEMS = [1 => 2, 0 => 1];\n}\n",
    )
    .unwrap();
    let (out, _) = analyse(&d);
    assert!(out.contains("should return list<int>"), "{out}");
}

/// PHP names may hold bytes that are not UTF-8: declaring such a missing function reaches its caller.
#[test]
fn a_name_that_is_not_utf8_is_looked_up_as_written() {
    let d = project("analyse_cache", "bytes", &[("pun.toml", CONFIG)]);
    std::fs::create_dir_all(d.join("src")).unwrap();
    let mut caller = b"<?php\n\nfunction h(): int\n{\n    return f".to_vec();
    caller.extend_from_slice(b"\xff();\n}\n");
    std::fs::write(d.join("src/use.php"), caller).unwrap();
    std::fs::write(d.join("src/decl.php"), "<?php\n").unwrap();
    let (out, _) = analyse(&d);
    assert!(out.contains("not found"), "{out}");
    let mut decl = b"<?php\n\nfunction f".to_vec();
    decl.extend_from_slice(b"\xff(): int\n{\n    return 1;\n}\n");
    std::fs::write(d.join("src/decl.php"), decl).unwrap();
    let (out, _) = analyse(&d);
    assert!(!out.contains("not found"), "{out}");
}

/// Two traits with the same eight methods, so a class using both has many adaptations to hash.
fn traits() -> String {
    use std::fmt::Write as _;
    let mut methods = String::new();
    for i in 1..=8 {
        let _ = write!(
            methods,
            "    public function m{i}(): string\n    {{\n        return '{i}';\n    }}\n\n"
        );
    }
    format!("<?php\n\nnamespace App;\n\ntrait A\n{{\n{methods}}}\n\ntrait B\n{{\n{methods}}}\n")
}

/// A class that resolves every clash with `insteadof` and re-exposes each method `as protected`.
fn adapted() -> String {
    use std::fmt::Write as _;
    let mut rules = String::new();
    for i in 1..=8 {
        let _ = write!(
            rules,
            "        A::m{i} insteadof B;\n        B::m{i} as protected b{i};\n"
        );
    }
    format!("<?php\n\nnamespace App;\n\nclass K\n{{\n    use A, B {{\n{rules}    }}\n}}\n")
}

/// A class's fingerprint is the same in every process, trait adaptations (`insteadof`, `as protected`) included: an
/// edit of an unrelated file analyses only that file.
#[test]
fn fingerprints_hold_across_processes() {
    let d = project(
        "analyse_cache",
        "adaptations",
        &[
            ("pun.toml", CONFIG),
            ("src/T.php", &traits()),
            ("src/K.php", &adapted()),
            (
                "src/use.php",
                "<?php\n\nnamespace App;\n\nfunction g(K $k): string\n{\n    return $k->m1();\n}\n",
            ),
            ("src/other.php", OTHER),
        ],
    );
    analyse(&d);
    std::fs::write(
        d.join("src/other.php"),
        OTHER.replace("return 2;", "return 3;"),
    )
    .unwrap();
    let (_, stats) = analyse(&d);
    assert_eq!(stats, Some((false, 1, 3)));
}

const LARAVEL: &str = "[extensions]\nlaravel = \"on\"\n[analyse]\npaths = [\"src\"]\nscan = [\"stubs\"]\n[analyse.rules]\n\"laravel/config-collection\" = \"on\"\n";
const FACADE: &str = "<?php\n\nnamespace Illuminate\\Support\\Facades;\n\nclass Config\n{\n    public static function collection(mixed $key, mixed $default = null): mixed\n    {\n        return null;\n    }\n}\n";

/// A Laravel project with Larastan's config checks: `config/` files and a `Config::collection()` user.
fn laravel(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let mut all = vec![
        ("pun.toml", LARAVEL),
        ("composer.json", "{\"require\":{\"php\":\"^8.3\"}}"),
        (
            "composer.lock",
            "{\"packages\":[{\"name\":\"laravel/framework\",\"version\":\"v12.10.0\"}]}",
        ),
        ("stubs/ConfigFacade.php", FACADE),
    ];
    all.extend_from_slice(files);
    project("analyse_cache", name, &all)
}

/// A cached run and a run without the cache on one thread, so the order files are checked in is fixed.
fn analyse_one_thread(dir: &Path) -> String {
    let args = ["analyse", "--format=json", "--max-processes=1"];
    let (out, err, code) = pun(dir, &args, &[]);
    let (want, _, want_code) = pun(dir, &[&args[..], &["--no-cache"]].concat(), &[]);
    assert_eq!((&out, code), (&want, want_code), "{err}");
    out
}

/// A config entry that turns from a file into a directory holding the config file is seen: the listing records each
/// entry's kind, not only its name.
#[test]
fn a_config_entry_that_becomes_a_directory_is_seen() {
    let d = laravel(
        "config_kind",
        &[
            ("config/app.php", "<?php return ['x' => 1];\n"),
            ("config/extra", "not php\n"),
            (
                "src/Use.php",
                "<?php\nnamespace App;\nuse Illuminate\\Support\\Facades\\Config;\nfunction f(): void { Config::collection('settings.value'); }\n",
            ),
        ],
    );
    analyse(&d);
    std::fs::remove_file(d.join("config/extra")).unwrap();
    std::fs::create_dir_all(d.join("config/extra")).unwrap();
    std::fs::write(
        d.join("config/extra/settings.php"),
        "<?php return ['value' => []];\n",
    )
    .unwrap();
    let (out, _) = analyse(&d);
    assert!(!out.contains("is not an array"), "{out}");
}

/// A required path that turns into a FIFO is no longer a file: the kind answer tells a regular file from anything else.
#[test]
fn a_required_file_replaced_by_a_fifo_is_seen() {
    let d = project("analyse_cache", "fifo", &[("pun.toml", CONFIG)]);
    let target = d.join("ext/target.php");
    std::fs::create_dir_all(d.join("ext")).unwrap();
    std::fs::write(&target, "<?php\n").unwrap();
    std::fs::create_dir_all(d.join("src")).unwrap();
    std::fs::write(
        d.join("src/A.php"),
        format!("<?php\nrequire '{}';\n", target.display()),
    )
    .unwrap();
    let (out, _) = analyse(&d);
    assert!(!out.contains("require.fileNotFound"), "{out}");
    std::fs::remove_file(&target).unwrap();
    let made = Command::new("mkfifo").arg(&target).status().unwrap();
    assert!(made.success());
    let (out, _) = analyse(&d);
    assert!(out.contains("require.fileNotFound"), "{out}");
}

/// A doc-typed config key resolves in the asking file's own scope, so which other files the cache reuses, or which
/// file a run analyses first, does not change it: `self::KEY` resolves in a class and not in a free function.
#[test]
fn a_memoized_config_type_is_resolved_per_file() {
    let outside = "<?php\n\nnamespace ConfigShapeOffsets;\n\nuse Illuminate\\Support\\Facades\\Config;\n\nfunction outside(): void\n{\n    Config::collection('scoped.0');\n}\n";
    let d = laravel(
        "config_memo_reused",
        &[
            (
                "config/scoped.php",
                "<?php\n\n/** @return array{self::KEY: string} */\nreturn [];\n",
            ),
            (
                "src/CallerZero.php",
                "<?php\n\nnamespace ConfigShapeOffsets;\n\nuse Illuminate\\Support\\Facades\\Config;\n\nclass CallerZero\n{\n    public const KEY = 0;\n\n    public function run(): void\n    {\n        Config::collection('scoped.0');\n    }\n}\n",
            ),
            ("src/ZOutside.php", outside),
        ],
    );
    let out = analyse_one_thread(&d);
    assert!(out.contains("CallerZero"), "{out}");
    assert!(!out.contains("ZOutside"), "{out}");
    std::fs::write(d.join("src/ZOutside.php"), format!("{outside}// touched\n")).unwrap();
    let out = analyse_one_thread(&d);
    assert!(!out.contains("ZOutside"), "{out}");
    let caller = std::fs::read_to_string(d.join("src/CallerZero.php")).unwrap();
    std::fs::write(
        d.join("src/CallerZero.php"),
        caller.replace("Config::collection('scoped.0');", "// removed"),
    )
    .unwrap();
    let out = analyse_one_thread(&d);
    assert!(
        !out.contains("CallerZero") && !out.contains("ZOutside"),
        "{out}"
    );
}

const MAIL_VIEW_USER: &str = "<?php\n\nnamespace App;\n\n/** @param view-string $view */\nfunction render(string $view): void\n{\n}\n\nfunction NAME(): void\n{\n    render('mail::welcome');\n}\n";

/// The view namespaces a provider registers with `loadViewsFrom()` count for every file that checks a `view-string`,
/// not only for the first file that asked for them in the run that read the providers.
#[test]
fn a_view_namespace_added_by_a_provider_is_seen_by_every_file() {
    view_namespace_added_by_a_provider("view_namespace_provider", "app/Providers");
}

fn view_namespace_added_by_a_provider(name: &str, providers: &str) {
    let keep = format!("{providers}/.keep");
    let d = laravel(
        name,
        &[
            (keep.as_str(), ""),
            (
                "src/A.php",
                &MAIL_VIEW_USER
                    .replace("NAME", "a")
                    .replace("function render", "function render_a")
                    .replace("render(", "render_a("),
            ),
            (
                "src/B.php",
                &MAIL_VIEW_USER
                    .replace("NAME", "b")
                    .replace("function render", "function render_b")
                    .replace("render(", "render_b("),
            ),
        ],
    );
    let out = analyse_one_thread(&d);
    assert!(out.contains("A.php") && out.contains("B.php"), "{out}");
    std::fs::write(
        d.join(format!("{providers}/MailServiceProvider.php")),
        format!(
            "<?php\n\nnamespace App\\Providers;\n\nclass MailServiceProvider\n{{\n    public function boot(): void\n    {{\n        $this->loadViewsFrom(__DIR__.'{}/resources/mail', 'mail');\n    }}\n}}\n",
            "/..".repeat(providers.matches('/').count() + 1)
        ),
    )
    .unwrap();
    std::fs::create_dir_all(d.join("resources/mail")).unwrap();
    std::fs::write(d.join("resources/mail/welcome.blade.php"), "hi\n").unwrap();
    let out = analyse_one_thread(&d);
    assert!(!out.contains("A.php") && !out.contains("B.php"), "{out}");
}

/// A generic argument written in a tag only Pun reads (`@phan-inherits`) reaches the users of a subclass declared
/// elsewhere, even when the file that declares the generic class built it first in the run.
#[test]
fn a_phan_inherits_argument_change_analyses_the_subclass_users_again() {
    let declaring = |t: &str| {
        format!(
            "<?php\n\nnamespace App;\n\n/** @phan-inherits Box<{t}> */\nclass IntBox extends Box\n{{\n}}\n\n/** @return Box<string> */\nfunction a(IntBox $x): Box\n{{\n    return $x;\n}}\n"
        )
    };
    let d = project(
        "analyse_cache",
        "phan_inherits_subclass",
        &[
            ("pun.toml", CONFIG),
            ("src/A.php", &declaring("int")),
            (
                "src/B.php",
                "<?php\n\nnamespace App;\n\n/** @return Box<string> */\nfunction b(Sub $x): Box\n{\n    return $x;\n}\n",
            ),
            (
                "src/Box.php",
                "<?php\n\nnamespace App;\n\n/** @template T */\nabstract class Box\n{\n}\n",
            ),
            (
                "src/C.php",
                "<?php\n\nnamespace App;\n\nclass Sub extends IntBox\n{\n}\n",
            ),
        ],
    );
    let first = analyse_one_thread(&d);
    assert!(first.contains("App\\\\Sub"), "{first}");
    std::fs::write(d.join("src/A.php"), declaring("string")).unwrap();
    analyse_one_thread(&d);
}

/// A subclass built after its parent in the same scope keeps what building the parent read: a file that reuses the
/// subclass is analysed again when only the parent's doc comment changes.
#[test]
fn a_subclass_built_after_its_parent_keeps_the_parents_reads() {
    let middle = |t: &str| {
        format!(
            "<?php\n\nnamespace App;\n\n/** @phan-inherits Box<{t}> */\nclass Mid extends Box\n{{\n}}\n"
        )
    };
    let d = project(
        "analyse_cache",
        "subclass_after_parent",
        &[
            ("pun.toml", CONFIG),
            (
                "src/a_first.php",
                "<?php\n\nnamespace App;\n\n/**\n * @param array{Mid, Sub} $p\n * @return array{Box<string>, Box<int>}\n */\nfunction first(array $p): array\n{\n    return $p;\n}\n\n/** @param Box<string> $b */\nfunction takes(Box $b): void\n{\n}\n",
            ),
            (
                "src/b_second.php",
                "<?php\n\nnamespace App;\n\nfunction second(Sub $x): void\n{\n    takes($x);\n}\n",
            ),
            (
                "src/Box.php",
                "<?php\n\nnamespace App;\n\n/** @template T */\nabstract class Box\n{\n}\n",
            ),
            ("src/Mid.php", &middle("string")),
            (
                "src/Sub.php",
                "<?php\n\nnamespace App;\n\nclass Sub extends Mid\n{\n}\n",
            ),
        ],
    );
    let first = analyse_one_thread(&d);
    assert!(!first.contains("b_second.php"), "{first}");
    std::fs::write(d.join("src/Mid.php"), middle("int   ")).unwrap();
    analyse_one_thread(&d);
}

/// A doc comment only Pun reads (`@phan-inherits`) on a class declared inside a function body reaches the files that
/// use the class: editing it analyses them again.
#[test]
fn a_phan_inherits_change_on_a_class_declared_in_a_body_analyses_its_users_again() {
    let declaring = |t: &str| {
        format!(
            "<?php\n\nnamespace App;\n\nfunction boot(): void\n{{\n    /** @phan-inherits Box<{t}> */\n    class Legacy extends Box\n    {{\n    }}\n}}\n"
        )
    };
    let d = project(
        "analyse_cache",
        "phan_inherits_nested_class",
        &[
            ("pun.toml", CONFIG),
            ("src/A.php", &declaring("int")),
            (
                "src/B.php",
                "<?php\n\nnamespace App;\n\n/** @return Box<string> */\nfunction b(Legacy $x): Box\n{\n    return $x;\n}\n",
            ),
            (
                "src/Box.php",
                "<?php\n\nnamespace App;\n\n/** @template T */\nabstract class Box\n{\n}\n",
            ),
        ],
    );
    let first = analyse_one_thread(&d);
    assert!(first.contains("B.php"), "{first}");
    std::fs::write(d.join("src/A.php"), declaring("string")).unwrap();
    analyse_one_thread(&d);
}
