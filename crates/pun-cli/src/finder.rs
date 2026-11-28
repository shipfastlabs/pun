use anyhow::{Result, bail};
use mago_database::GlobSettings;
use mago_database::matcher::{ExclusionMatcher, contains_glob_metacharacters};
use pcre2::bytes::Regex;
use pun_config::Config;
use pun_tokens::php_compat::{fnmatch, preg_compile, preg_quote};
use rayon::prelude::*;
use std::path::{Path, PathBuf};

const PINT_NOT_NAME: [&str; 4] = [
    "_ide_helper_actions.php",
    "_ide_helper_models.php",
    "_ide_helper.php",
    ".phpstorm.meta.php",
];
fn pint_not_name(cfg: &Config) -> Vec<&str> {
    let mut not_name = PINT_NOT_NAME.to_vec();
    if !cfg.blade {
        not_name.push("*.blade.php");
    }
    not_name.extend(cfg.finder.not_name.iter().map(String::as_str));
    not_name
}

fn real_path(cfg: &Config, p: &Path) -> Result<PathBuf, FinderError> {
    cfg.root
        .join(p)
        .canonicalize()
        .map_err(|_| FinderError::NotReadable(p.to_path_buf()))
}

const PINT_EXCLUDE: [&str; 4] = ["bootstrap/cache", "build", "node_modules", "storage"];
const VCS: [&str; 9] = [
    ".svn",
    "_svn",
    "CVS",
    "_darcs",
    ".arch-params",
    ".monotone",
    ".bzr",
    ".git",
    ".hg",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinderError {
    NotReadable(PathBuf),
    DirectoryNotFound(String),
}

impl std::fmt::Display for FinderError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            FinderError::NotReadable(p) => {
                write!(f, "The path \"{}\" is not readable.", p.display())
            }
            FinderError::DirectoryNotFound(d) => write!(f, "The \"{d}\" directory does not exist."),
        }
    }
}

impl std::error::Error for FinderError {}

struct Filters {
    name: Regex,
    dot: Regex,
}

enum Visit {
    Descend,
    File,
    Skip,
}

struct Finder {
    dirs: Vec<PathBuf>,
    append: Vec<PathBuf>,
    excluded_names: Vec<String>,
    excluded_pattern: Option<Regex>,
    not_names: Vec<Regex>,
    not_paths: Vec<Regex>,
    matcher: Option<(PathBuf, ExclusionMatcher<String>)>,
}

impl Finder {
    fn php_cs_fixer() -> Self {
        Finder {
            dirs: vec![],
            append: vec![],
            excluded_names: vec![],
            excluded_pattern: None,
            not_names: vec![],
            not_paths: vec![],
            matcher: None,
        }
        .exclude(["vendor"])
    }

    fn pint(cfg: &Config) -> Result<Self> {
        let mut f = Self::php_cs_fixer()
            .not_name(pint_not_name(cfg))?
            .exclude(PINT_EXCLUDE);
        let (globs, plain): (Vec<&String>, Vec<&String>) =
            (cfg.finder.exclude.iter()).partition(|e| contains_glob_metacharacters(e.as_str()));
        f = f
            .exclude(plain.into_iter().map(String::as_str))
            .matching(&cfg.root, globs.into_iter().cloned())?;
        for d in &cfg.finder.in_dirs {
            let p = cfg.root.join(d);
            if !p.is_dir() {
                return Err(FinderError::DirectoryNotFound(d.clone()).into());
            }
            f.dirs.push(p);
        }
        for p in &cfg.finder.not_path {
            f.not_paths.push(path_regex(p)?);
        }
        Ok(f)
    }

    fn exclude<'a>(mut self, dirs: impl IntoIterator<Item = &'a str>) -> Self {
        let mut pats: Vec<String> = self
            .excluded_pattern
            .take()
            .map_or_default(|r| vec![r.as_str().to_string()]);
        for d in dirs {
            let d = d.trim_end_matches('/');
            if d.contains('/') {
                pats.push(format!("(?:^|/)(?:{})(?:/|$)", quote(d)));
            } else {
                self.excluded_names.push(d.to_string());
            }
        }
        if !pats.is_empty() {
            self.excluded_pattern = Some(Regex::new(&pats.join("|")).expect("quoted pattern"));
        }
        self
    }

    fn matching(mut self, root: &Path, patterns: impl IntoIterator<Item = String>) -> Result<Self> {
        let m = ExclusionMatcher::compile(patterns, GlobSettings::default())
            .map_err(|e| anyhow::anyhow!("invalid exclude: {e}"))?;
        self.matcher = (!m.is_empty()).then(|| (root.to_path_buf(), m));
        Ok(self)
    }

    fn matched(&self, path: &Path) -> bool {
        self.matcher.as_ref().is_some_and(|(root, m)| {
            m.is_match(&path.strip_prefix(root).unwrap_or(path).to_string_lossy())
        })
    }

    fn not_name<'a>(mut self, globs: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        for g in globs {
            self.not_names.push(php_regex(&glob_to_regex(g))?);
        }
        Ok(self)
    }

    fn files(&self) -> Result<Vec<PathBuf>> {
        let filters = Filters {
            name: Regex::new(r"\.php$").unwrap(),
            dot: Regex::new(r"(^|/)\..+(/|$)").unwrap(),
        };
        let mut out = Vec::new();
        for dir in &self.dirs {
            out.extend(self.walk(&filters, dir, dir)?);
        }
        out.extend(self.append.iter().cloned());
        Ok(out)
    }

    /// The files under `dir` in the order a single walk from `root` lists them: each subdirectory is walked on its own
    /// thread and its files take its place in the listing. A listing error comes after the files and errors of the
    /// entries before it, as in one walk.
    fn walk(&self, filters: &Filters, root: &Path, dir: &Path) -> Result<Vec<PathBuf>> {
        enum Part {
            File(PathBuf),
            Dir(PathBuf),
        }
        let mut parts = Vec::new();
        let mut failed = None;
        for entry in walkdir::WalkDir::new(dir).min_depth(1).max_depth(1) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            };
            match self.visit(filters, root, &entry) {
                Visit::Descend => parts.push(Part::Dir(entry.into_path())),
                Visit::File => parts.push(Part::File(entry.into_path())),
                Visit::Skip => {}
            }
        }
        let walked: Vec<Result<Vec<PathBuf>>> = parts
            .into_par_iter()
            .map(|part| match part {
                Part::File(file) => Ok(vec![file]),
                Part::Dir(sub) => self.walk(filters, root, &sub),
            })
            .collect();
        let mut out = Vec::new();
        for files in walked {
            out.extend(files?);
        }
        match failed {
            Some(e) => Err(e.into()),
            None => Ok(out),
        }
    }

    fn visit(&self, filters: &Filters, root: &Path, entry: &walkdir::DirEntry) -> Visit {
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let kind = entry.file_type();
        if kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()) {
            let fname = entry.file_name().to_string_lossy();
            let pruned = self.excluded_names.iter().any(|n| *n == fname)
                || (fname.len() > 1 && fname.starts_with('.'))
                || VCS.contains(&fname.as_ref())
                || self
                    .excluded_pattern
                    .as_ref()
                    .is_some_and(|r| r.is_match(rel.as_bytes()).unwrap_or(false))
                || self.matched(entry.path());
            return if !pruned && kind.is_dir() {
                Visit::Descend
            } else {
                Visit::Skip
            };
        }
        if entry.path_is_symlink() && !entry.path().is_file() {
            return Visit::Skip;
        }
        let fname = entry.file_name().to_string_lossy();
        let m = |r: &Regex, s: &str| r.is_match(s.as_bytes()).unwrap_or(false);
        if !m(&filters.name, &fname) || self.not_names.iter().any(|r| m(r, &fname)) {
            return Visit::Skip;
        }
        if m(&filters.dot, &rel)
            || self.not_paths.iter().any(|r| m(r, &rel))
            || self.matched(entry.path())
        {
            return Visit::Skip;
        }
        Visit::File
    }
}

pub fn discover(cfg: &Config, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let has_in = !cfg.finder.in_dirs.is_empty();
    let f = if paths.is_empty() && has_in {
        Finder::pint(cfg)?
    } else {
        let paths: Vec<PathBuf> = if paths.is_empty() {
            vec![cfg.root.clone()]
        } else {
            paths.to_vec()
        };
        let mut f = if has_in {
            Finder::php_cs_fixer()
        } else {
            Finder::pint(cfg)?
        };
        for p in paths {
            push(&mut f, real_path(cfg, &p)?);
        }
        f
    };
    let mut seen = std::collections::HashSet::new();
    Ok((f.files()?)
        .into_iter()
        .filter(|p| seen.insert(p.clone()))
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Format,
    Lint,
    Analyse,
}

pub fn files(cfg: &Config, command: Option<Command>, paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let dirs = match command {
        Some(Command::Format) => {
            let mut out = discover(cfg, paths)?;
            out.sort();
            return Ok(out);
        }
        None | Some(Command::Lint) => &cfg.files.paths,
        Some(Command::Analyse) => &cfg.analyse.paths,
    };
    let mut f = Finder::php_cs_fixer().matching(&cfg.root, exclude_patterns(cfg, command))?;
    if command.is_some() {
        f = f.not_name(["*.blade.php"])?;
    }
    if paths.is_empty() {
        for d in dirs {
            let p = cfg.root.join(d);
            if !p.exists() {
                return Err(FinderError::DirectoryNotFound(d.clone()).into());
            }
            push(&mut f, p);
        }
        if dirs.is_empty() {
            f.dirs.push(cfg.root.clone());
        }
    } else {
        for p in paths {
            push(&mut f, real_path(cfg, p)?);
        }
    }
    let mut out: Vec<PathBuf> = f.files()?;
    out.retain(|p| !f.matched(p));
    out.sort();
    out.dedup();
    Ok(out)
}

fn exclude_patterns(cfg: &Config, command: Option<Command>) -> Vec<String> {
    let own = match command {
        None | Some(Command::Format) => &cfg.files.exclude,
        Some(Command::Lint) => &cfg.lint.exclude,
        Some(Command::Analyse) => &cfg.analyse.exclude,
    };
    let defaults = command.map_or(&[][..], |_| &PINT_EXCLUDE[..]);
    (own.iter().cloned())
        .chain(defaults.iter().map(ToString::to_string))
        .collect()
}

pub struct Exclusions(Finder);

impl Exclusions {
    pub fn new(cfg: &Config, command: Command) -> Result<Exclusions> {
        Self::globs(&cfg.root, exclude_patterns(cfg, Some(command)))
    }

    pub fn globs(root: &Path, patterns: Vec<String>) -> Result<Exclusions> {
        Ok(Exclusions(Finder::php_cs_fixer().matching(root, patterns)?))
    }

    pub fn is_excluded(&self, path: &Path) -> bool {
        self.0.matched(path)
    }
}

fn push(f: &mut Finder, p: PathBuf) {
    if p.is_file() {
        f.append.push(p);
    } else {
        f.dirs.push(p);
    }
}

#[derive(Debug, Clone)]
pub enum GitSelect {
    Dirty,
    Diff(String),
    Staged,
}

/// The files `--dirty`, `--diff` or `--staged` formats: the changed PHP files Pint's finder lists.
pub fn git_paths(cfg: &Config, sel: &GitSelect) -> Result<Vec<PathBuf>> {
    pick(cfg, &git_names(cfg, sel)?)
}

/// The files `sel` formats ([`git_paths`]), then the changed files the other stages pick theirs from: the format-only
/// excludes (`[format] exclude` and `not-path`) don't take a file out of linting or analysis.
pub fn git_selection(cfg: &Config, sel: &GitSelect) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let wanted = git_names(cfg, sel)?;
    let format = pick(cfg, &wanted)?;
    if cfg.finder.exclude == cfg.files.exclude && cfg.finder.not_path.is_empty() {
        return Ok((format.clone(), format));
    }
    let mut shared = cfg.clone();
    shared.finder.exclude.clone_from(&cfg.files.exclude);
    shared.finder.not_path.clear();
    Ok((format, pick(&shared, &wanted)?))
}

/// The changed PHP files under the root that Pint's finder lists, outside the root no symlink among them leads: Pint
/// follows one there, and the rewrite would land in a file the project doesn't hold.
fn pick(cfg: &Config, wanted: &std::collections::HashSet<PathBuf>) -> Result<Vec<PathBuf>> {
    if wanted.is_empty() {
        return Ok(vec![]);
    }
    let mut f = Finder::pint(cfg)?;
    f.dirs.push(cfg.root.clone());
    let picked: Vec<PathBuf> = f
        .files()?
        .into_iter()
        .filter(|p| wanted.contains(p))
        .collect();
    if picked.is_empty() {
        Ok(picked)
    } else {
        let mut found = discover(cfg, &picked)?;
        found.retain(|p| p.starts_with(&cfg.root));
        Ok(found)
    }
}

/// The paths git names for `sel`, joined to the root. `--dirty` and `--diff` read git as Pint does, quoting and
/// worktree-relative names included; the Pun-only `--staged` reads NUL-separated names relative to the root, with
/// type changes (a symlink become a file) among the changes.
fn git_names(cfg: &Config, sel: &GitSelect) -> Result<std::collections::HashSet<PathBuf>> {
    fn diff(rev: Option<&str>) -> Vec<&str> {
        ["diff", "--name-only", "--diff-filter=AMR"]
            .into_iter()
            .chain(rev)
            .chain(["--", "**.php"])
            .collect()
    }
    let text = |args: &[&str]| {
        git(&cfg.root, args).map(|out| String::from_utf8_lossy(&out).replace('\r', "\n"))
    };
    let lists = |cmds: &[&[&str]]| -> Option<Vec<PathBuf>> {
        let mut all = Vec::new();
        for args in cmds {
            all.extend(
                text(args)?
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from),
            );
        }
        Some(all)
    };
    let untracked: &[&str] = &["ls-files", "--others", "--exclude-standard", "--", "**.php"];
    let staged = [
        "diff",
        "--cached",
        "--name-only",
        "-z",
        "--relative",
        "--diff-filter=AMRT",
        "--",
        "**.php",
    ];
    let names: Option<Vec<PathBuf>> = match sel {
        GitSelect::Dirty => text(&["status", "--short", "--", "**.php"]).map(|out| {
            out.lines()
                .filter(|l| !l.is_empty())
                .map(|l| {
                    (
                        l.get(3..).unwrap_or(""),
                        l.get(..3.min(l.len())).unwrap_or("").trim(),
                    )
                })
                .filter(|(_, s)| *s != "D")
                .map(|(f, s)| {
                    if s == "R" {
                        f.split_once(" -> ").map_or(f, |x| x.1)
                    } else {
                        f
                    }
                    .into()
                })
                .collect()
        }),
        GitSelect::Diff(branch) => lists(&[
            &diff(Some(&format!("{branch}...HEAD"))),
            &diff(Some("--cached")),
            &diff(None),
            untracked,
        ]),
        GitSelect::Staged => git(&cfg.root, &staged).map(|out| {
            use std::os::unix::ffi::OsStrExt;
            (out.split(|&b| b == 0))
                .filter(|n| !n.is_empty())
                .map(|n| std::ffi::OsStr::from_bytes(n).into())
                .collect()
        }),
    };
    let names = match (names, sel) {
        (Some(names), _) => names,
        (None, GitSelect::Dirty) => bail!("The [--dirty] option is only available when using Git."),
        (None, GitSelect::Diff(_)) => {
            bail!("The [--diff] option is only available when using Git.")
        }
        (None, GitSelect::Staged) => {
            bail!("The [--staged] option is only available when using Git.")
        }
    };
    Ok(names.iter().map(|n| cfg.root.join(n)).collect())
}

/// Whether `path` resolves outside `root` (canonical), a dangling symlink included: what is there isn't the project's.
/// A path with nothing there stays inside.
pub(crate) fn leaves(root: &Path, path: &Path) -> bool {
    match path.canonicalize() {
        Ok(real) => !real.starts_with(root),
        Err(_) => path.symlink_metadata().is_ok(),
    }
}

pub fn is_path_excluded(cfg: &Config, file: &Path) -> bool {
    let base = cfg.root.to_string_lossy();
    let full = file.to_string_lossy().replace('\\', "/");
    let rel = full
        .strip_prefix(&format!("{base}/"))
        .unwrap_or(&full)
        .to_string();
    let name = file
        .file_name()
        .map_or_default(|n| n.to_string_lossy().into_owned());
    let not_name = pint_not_name(cfg);
    let under = |d: &str| {
        let d = d.replace('\\', "/");
        rel == d || rel.starts_with(&format!("{d}/"))
    };
    not_name
        .iter()
        .any(|p| fnmatch(p.as_bytes(), name.as_bytes()))
        || PINT_EXCLUDE
            .iter()
            .copied()
            .chain(cfg.finder.exclude.iter().map(String::as_str))
            .any(under)
        || cfg
            .finder
            .not_path
            .iter()
            .any(|p| fnmatch(p.as_bytes(), rel.as_bytes()))
        || (cfg.finder.exclude.iter())
            .any(|e| contains_glob_metacharacters(e) && glob_matches(e, &rel))
}

fn glob_matches(glob: &str, rel: &str) -> bool {
    ExclusionMatcher::compile([glob], GlobSettings::default()).is_ok_and(|m| m.is_match(rel))
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(out.stdout)
}

fn php_regex(p: &str) -> Result<Regex> {
    preg_compile(p).map_err(|m| anyhow::anyhow!("preg_match(): {m}"))
}

fn path_regex(s: &str) -> Result<Regex> {
    if pun_config::is_regex(s) {
        php_regex(s)
    } else {
        Ok(Regex::new(&quote(s))?)
    }
}

fn quote(s: &str) -> String {
    String::from_utf8_lossy(&preg_quote(s.as_bytes(), Some(b'/'))).into_owned()
}

fn glob_to_regex(glob: &str) -> String {
    let unanchored = glob.starts_with("**/");
    let glob = if unanchored {
        format!("/{glob}")
    } else {
        glob.to_string()
    };
    let g = glob.as_bytes();
    let (mut first, mut escaping, mut curlies) = (true, false, 0);
    let mut re = String::new();
    let mut i = 0;
    while i < g.len() {
        let ch = glob[i..].chars().next().unwrap();
        let mut car = ch.to_string();
        if first && g[i] != b'.' {
            re.push_str(r"(?=[^\.])");
        }
        first = g[i] == b'/';
        if first
            && g.len() > i + 2
            && &g[i + 1..i + 3] == b"**"
            && (g.len() == i + 3 || g[i + 3] == b'/')
        {
            let mut c = "[^/]++/".to_string();
            if g.len() == i + 3 {
                c.push('?');
            }
            car = format!("/(?:(?=[^\\.]){c})*");
            i += 2 + usize::from(g.len() > i + 3);
            re.push_str(&car);
            i += 1;
            escaping = false;
            continue;
        }
        match car.as_str() {
            "#" | "." | "(" | ")" | "|" | "+" | "^" | "$" => {
                re.push('\\');
                re.push_str(&car);
            }
            "*" => re.push_str(if escaping { "\\*" } else { "[^/]*" }),
            "?" => re.push_str(if escaping { "\\?" } else { "[^/]" }),
            "{" => {
                re.push_str(if escaping { "\\{" } else { "(" });
                if !escaping {
                    curlies += 1;
                }
            }
            "}" if curlies > 0 => {
                re.push_str(if escaping { "}" } else { ")" });
                if !escaping {
                    curlies -= 1;
                }
            }
            "," if curlies > 0 => re.push_str(if escaping { "," } else { "|" }),
            "\\" => {
                if escaping {
                    re.push_str("\\\\");
                    escaping = false;
                } else {
                    escaping = true;
                }
                i += 1;
                continue;
            }
            _ => re.push_str(&car),
        }
        escaping = false;
        i += ch.len_utf8();
    }
    if unanchored {
        re.insert(1 + "(?=[^\\.])".len(), '?');
    }
    format!("#^{re}$#")
}
