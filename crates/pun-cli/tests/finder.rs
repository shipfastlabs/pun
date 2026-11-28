//! Discovery parity with the real Pint (`php .oracle/pint/builds/pint`): every scenario builds a tree whose PHP
//! files all start with `<?PHP` (pint.json enables only `full_opening_tag`), so `pint --test --format=txt` lists
//! every discovered regular file ("N) path") and counts every discovered file, symlinks included ("Found x of N").
//! Our side must produce the same set and count. Skipped when the oracle is not installed.
mod common;

use punphp::finder::{GitSelect, discover, git_paths, git_selection, is_path_excluded};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const BAD: &str = "<?PHP\necho 1;\n";

fn tree(files: &[&str], pint_json: &str) -> PathBuf {
    let root = common::tree("finder", files, BAD);
    std::fs::write(root.join("pint.json"), pint_json).unwrap();
    root
}

fn oracle() -> Option<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.oracle/pint/builds/pint");
    p.exists().then_some(p)
}

fn json(extra: &str) -> String {
    format!(r#"{{"preset":"empty","rules":{{"full_opening_tag":true}}{extra}}}"#)
}

fn cmd(program: &Path, dir: &Path) -> Command {
    let mut c = Command::new(program);
    c.current_dir(dir)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("HOME", std::env::var("HOME").unwrap())
        .env("COLUMNS", "80");
    c
}

type Listing = Result<(BTreeSet<String>, usize), String>;

fn pint(dir: &Path, args: &[&str]) -> Listing {
    let out = cmd(Path::new("php"), dir)
        .arg(oracle().unwrap())
        .args(["--test", "--format=txt"])
        .args(args)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let Some(found) = text.lines().find(|l| l.starts_with("Found ")) else {
        return Err(text);
    };
    let total = found
        .split(" of ")
        .nth(1)
        .unwrap()
        .split(' ')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let files = text
        .lines()
        .filter_map(|l| {
            l.trim_start()
                .split_once(") ")
                .filter(|(n, _)| n.parse::<u32>().is_ok())
                .map(|(_, f)| f.to_string())
        })
        .collect();
    Ok((files, total))
}

fn listing(root: &Path, files: anyhow::Result<Vec<PathBuf>>) -> Listing {
    let files = files.map_err(|e| e.to_string())?;
    let total = files.len();
    let regular = files
        .iter()
        .filter(|p| !p.is_symlink())
        .map(|p| {
            p.strip_prefix(root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    Ok((regular, total))
}

fn load(root: &Path, config: Option<&str>) -> Result<pun_config::Config, String> {
    let pint = root.join(config.unwrap_or("pint.json"));
    pun_config::migrate::from_pint(&pint, root, None, &|_| true)
        .and_then(|m| pun_config::from_toml(root, &m.toml))
        .map_err(|e| e.to_string())
}

fn ours(root: &Path, config: Option<&str>, paths: &[&str]) -> Listing {
    let cfg = load(root, config)?;
    let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    listing(root, discover(&cfg, &paths))
}

fn check(root: &Path, config: Option<&str>, paths: &[&str]) {
    let mut args: Vec<String> = paths.iter().map(ToString::to_string).collect();
    if let Some(c) = config {
        args.push(format!("--config={c}"));
    }
    let want = pint(root, &args.iter().map(String::as_str).collect::<Vec<_>>());
    let got = ours(root, config, paths);
    match (&want, &got) {
        (Err(w), Err(g)) => assert!(
            w.contains(g.as_str()),
            "{} {paths:?}: pint said\n{w}\nwe said\n{g}",
            root.display()
        ),
        _ => assert_eq!(got, want, "{} {paths:?} {config:?}", root.display()),
    }
}

const COMMON: &[&str] = &[
    "app/A.php",
    "app/v.blade.php",
    "app/build/B.php",
    "app/.hidden/C.php",
    "app/.D.php",
    "vendor/x/E.php",
    "app/vendor/E2.php",
    "x/bootstrap/cache/F.php",
    "bootstrap/cache/F2.php",
    "bootstrap/app.php",
    "storage/G.php",
    "app/storage/G2.php",
    "app/_ide_helper.php",
    "_ide_helper_models.php",
    "a/_ide_helper_actions.php",
    ".phpstorm.meta.php",
    "app/skip/H.php",
    "app/I.txt",
    "app/notme.php",
    "app/U.PHP",
    "app/with space.php",
    "app/.git/K.php",
    "app/CVS/K.php",
    "node_modules/n/N.php",
    "src/Sub/Deep/T.php",
    "src/Sub/ZTest.php",
    "src/a.php",
    "src/b.php",
    "src/ax.php",
    "src/README",
];

fn with_links(root: &Path) {
    std::os::unix::fs::symlink(root.join("app"), root.join("linkdir")).unwrap();
    std::os::unix::fs::symlink(root.join("app/A.php"), root.join("app/L.php")).unwrap();
    std::os::unix::fs::symlink(root.join("missing.php"), root.join("app/dangling.php")).unwrap();
}

#[test]
fn finder_matches_pint() {
    if oracle().is_none() {
        return;
    }
    let root = tree(COMMON, &json(""));
    with_links(&root);
    std::fs::write(root.join(".gitignore"), "app/A.php\n").unwrap();
    check(&root, None, &[]);
    check(
        &root,
        None,
        &["app", "vendor/x/E.php", "app/A.php", "storage/G.php"],
    );
    check(&root, None, &["nope"]);
    check(&root, None, &["."]);

    let root = tree(
        COMMON,
        &json(r##","exclude":["skip","src/Sub/Deep/"],"notPath":["#^app/with#","build"]"##),
    );
    check(&root, None, &[]);
    check(&root, None, &["app/build/B.php"]);
    let root = tree(COMMON, &json(r#","notPath":"/^SRC/i","exclude":"app""#));
    check(&root, None, &[]);
    let root = tree(COMMON, &json(r#","notPath":["/a/b/"]"#));
    check(&root, None, &[]);

    let root = tree(COMMON, &json(r#","in":["src","app"],"exclude":["Deep"]"#));
    check(&root, None, &[]);
    check(&root, None, &["storage"]);
    let root = tree(COMMON, &json(r#","in":["nodir"]"#));
    check(&root, None, &[]);

    let modules = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/pinkary/node_modules");
    if modules.exists() {
        let files: Vec<&str> = COMMON
            .iter()
            .copied()
            .filter(|f| !f.starts_with("node_modules/"))
            .collect();
        let root = tree(
            &files,
            &json("").replace(
                r#""full_opening_tag":true"#,
                r#""full_opening_tag":true,"Pint/laravel_blade":true"#,
            ),
        );
        std::os::unix::fs::symlink(&modules, root.join("node_modules")).unwrap();
        check(&root, None, &[]);
    }

    let root = tree(
        &["app/Café.php", "app/Cafe.php", "app/né/X.php", "app/ü.php"],
        &json(r#","notPath":["né"]"#),
    );
    check(&root, None, &[]);

    let root = tree(COMMON, &json(r#","exclude":["src"]"#));
    std::fs::write(root.join("other.json"), json(r#","exclude":["app"]"#)).unwrap();
    check(&root, Some("other.json"), &[]);
}

#[test]
fn stdin_exclusion_matches_pint() {
    if oracle().is_none() {
        return;
    }
    let root = tree(
        &[],
        &json(
            r#","exclude":["legacy","app/Old"],"notPath":["gen/*","s/?.php","t/[!x]*.php","u/[a-c].php","v/\\*.php","w/[\\].php","y/[\\]]*.php"]"#,
        ),
    );
    let cfg = load(&root, None).unwrap();
    let mut blade_cfg = cfg.clone();
    blade_cfg.blade = true;
    for name in [
        "app/A.php",
        "storage/x.php",
        "storage",
        "app/storage/x.php",
        "bootstrap/cache/c.php",
        "build/b.php",
        "node_modules/n.php",
        "legacy/l.php",
        "app/legacy/l.php",
        "app/Old/o.php",
        "app/Older/o.php",
        "FooStub.php",
        "src/FooStub.php",
        "x.php",
        "z.php",
        "gen/a/b.php",
        "src/gen/a.php",
        "v.blade.php",
        "_ide_helper.php",
        "app/.phpstorm.meta.php",
        "[a].php",
        "s/a.php",
        "s/ab.php",
        "t/a.php",
        "t/x.php",
        "u/b.php",
        "u/d.php",
        "v/*.php",
        "v/a.php",
        "w/\\.php",
        "w/a.php",
        "y/].php",
        "y/a.php",
    ] {
        let out = cmd(Path::new("php"), &root)
            .arg(oracle().unwrap())
            .arg(format!("--stdin-filename={name}"))
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut c| {
                use std::io::Write;
                c.stdin.take().unwrap().write_all(BAD.as_bytes())?;
                c.wait_with_output()
            })
            .unwrap();
        let excluded = out.stdout == BAD.as_bytes();
        assert_eq!(
            is_path_excluded(&cfg, Path::new(name)),
            excluded,
            "relative {name}"
        );
        assert_eq!(
            is_path_excluded(&cfg, &root.join(name)),
            excluded,
            "absolute {name}"
        );
        if name.ends_with(".blade.php") {
            assert!(
                !is_path_excluded(&blade_cfg, Path::new(name)),
                "the laravel/blade rule keeps {name}"
            );
        }
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = cmd(Path::new("git"), dir)
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn git_selection_matches_pint() {
    if oracle().is_none() {
        return;
    }
    let root = tree(
        &[
            "app/Clean.php",
            "app/Mod.php",
            "app/Del.php",
            "app/Ren.php",
            "app/Staged.php",
            "vendor/V.php",
            "storage/S.php",
        ],
        &json(""),
    );
    let ours = |sel: &GitSelect| listing(&root, git_paths(&load(&root, None).unwrap(), sel));
    assert!(
        pint(&root, &["--dirty"])
            .unwrap_err()
            .contains("The [--dirty] option is only available when using Git.")
    );
    for (sel, flag) in [
        (GitSelect::Dirty, "dirty"),
        (GitSelect::Diff("main".into()), "diff"),
        (GitSelect::Staged, "staged"),
    ] {
        let message = format!("The [--{flag}] option is only available when using Git.");
        assert_eq!(ours(&sel).unwrap_err(), message);
    }

    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "init"]);
    assert_eq!(ours(&GitSelect::Dirty).unwrap(), (BTreeSet::new(), 0));
    std::fs::write(root.join("vendor/V.php"), "<?PHP\necho 0;\n").unwrap();
    assert_eq!(ours(&GitSelect::Dirty), pint(&root, &["--dirty"]));
    git(&root, &["checkout", "-q", "vendor/V.php"]);

    git(&root, &["checkout", "-qb", "feature"]);
    std::fs::write(root.join("app/Mod.php"), "<?PHP\necho 2;\n").unwrap();
    std::fs::remove_file(root.join("app/Del.php")).unwrap();
    git(&root, &["mv", "app/Ren.php", "app/Renamed.php"]);
    std::fs::write(root.join("app/Staged.php"), "<?PHP\necho 3;\n").unwrap();
    git(&root, &["add", "app/Staged.php"]);
    std::fs::write(root.join("app/Staged.php"), "<?PHP\necho 4;\n").unwrap();
    std::fs::write(root.join("app/New.php"), BAD).unwrap();
    std::fs::write(root.join("vendor/V.php"), "<?PHP\necho 5;\n").unwrap();
    std::fs::write(root.join("app/notes.txt"), "x").unwrap();
    assert_eq!(ours(&GitSelect::Dirty), pint(&root, &["--dirty"]));
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "feature"]);
    std::fs::write(root.join("app/Clean.php"), "<?PHP\necho 6;\n").unwrap();
    std::fs::write(root.join("app/Untracked.php"), BAD).unwrap();
    assert_eq!(
        ours(&GitSelect::Diff("main".into())),
        pint(&root, &["--diff=main"])
    );
    let message = ours(&GitSelect::Diff("nope".into())).unwrap_err();
    assert_eq!(
        message,
        "The [--diff] option is only available when using Git."
    );
    assert!(
        pint(&root, &["--diff=nope"])
            .unwrap_err()
            .contains(&message)
    );
    git(&root, &["add", "app/Clean.php"]);
    let staged = ours(&GitSelect::Staged).unwrap();
    assert_eq!(staged, (["app/Clean.php".to_string()].into(), 1));
}

fn rel(root: &Path, files: &[PathBuf]) -> BTreeSet<String> {
    let names = files.iter().map(|p| p.strip_prefix(root).unwrap_or(p));
    names.map(|p| p.to_string_lossy().into_owned()).collect()
}

/// Git selection doesn't follow a changed symlink out of the project (Pint does, and formats the file it leads to);
/// one that stays inside still selects its target, as in Pint.
#[test]
fn git_selection_stays_in_the_project() {
    let top = common::tree("finder", &["ext/O.php", "repo/app/A.php"], BAD);
    let root = top.join("repo");
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-qm", "init"]);
    git(&root, &["checkout", "-qb", "feature"]);
    std::os::unix::fs::symlink(top.join("ext/O.php"), root.join("app/Out.php")).unwrap();
    std::os::unix::fs::symlink("A.php", root.join("app/In.php")).unwrap();
    let cfg = pun_config::from_toml(&root, "").unwrap();
    let inside = BTreeSet::from(["app/A.php".to_string()]);
    assert_eq!(
        rel(&root, &git_paths(&cfg, &GitSelect::Dirty).unwrap()),
        inside
    );
    let diff = GitSelect::Diff("main".into());
    assert_eq!(rel(&root, &git_paths(&cfg, &diff).unwrap()), inside);
    git(&root, &["add", "."]);
    assert_eq!(
        rel(&root, &git_paths(&cfg, &GitSelect::Staged).unwrap()),
        inside
    );
}

/// `--staged` (Pun's own flag) names every staged PHP file of a project nested in the repository: names relative to
/// the project, names git quotes (a tab, non-ASCII), and a symlink turned into a file. The other stages start from the
/// changed files the format-only excludes leave in.
#[test]
fn staged_selects_every_staged_file() {
    let files = [
        "sub/app/A.php",
        "sub/app/Mod.php",
        "sub/fmt/F.php",
        "sub/app/Caf\u{e9}.php",
        "sub/app/T\tab.php",
    ];
    let top = common::tree("finder", &files, BAD);
    let root = top.join("sub");
    std::os::unix::fs::symlink("A.php", root.join("app/Type.php")).unwrap();
    git(&top, &["init", "-q"]);
    git(&top, &["add", "."]);
    git(&top, &["commit", "-qm", "init"]);
    std::fs::remove_file(root.join("app/Type.php")).unwrap();
    for f in &files {
        std::fs::write(top.join(f), "<?PHP\necho 2;\n").unwrap();
    }
    std::fs::write(root.join("app/Type.php"), BAD).unwrap();
    git(&top, &["add", "."]);
    let cfg = pun_config::from_toml(&root, "[format]\nexclude = [\"fmt\"]\n").unwrap();
    let changed = [
        "app/A.php",
        "app/Caf\u{e9}.php",
        "app/Mod.php",
        "app/T\tab.php",
        "app/Type.php",
    ];
    let (format, every) = git_selection(&cfg, &GitSelect::Staged).unwrap();
    assert_eq!(rel(&root, &format), changed.map(String::from).into());
    let mut all = changed.map(String::from).to_vec();
    all.push("fmt/F.php".into());
    assert_eq!(rel(&root, &every), all.into_iter().collect());
}
