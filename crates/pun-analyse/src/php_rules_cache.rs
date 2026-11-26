//! `[analyse] php-rules`' result cache, like PHPStan's: the errors of each check (a file, or a trait's body in the
//! context of a class) from earlier runs, so an unchanged check skips the worker and a run whose checks are all
//! unchanged starts none.
//!
//! An entry holds while the run key and the content of the checked file and of each file it depends on are
//! unchanged. Its dependencies, as PHPStan's DependencyResolver finds them, are the files declaring the classes and
//! functions the file names or its expressions' types hold, with their parent classes, interfaces and traits. The run
//! key covers the caller's key (Pun's version, binary and config), the worker, its setup, the neon files with their
//! includes, Composer's lock file and installed packages, and the PHP files the workers had loaded (the rules, the
//! bootstrap files and what those load, but the DI container PHPStan compiles from what the key already covers into
//! the worker's temporary directory, which a new machine lacks), whose list the cache keeps and hashes again on each
//! run.
//!
//! The file is JSON: the run key's parts with their hashes, the loaded files with theirs, a table of paths, and the
//! entries, which name their file and dependencies by their index in the table. A file that is missing, unreadable or
//! from another key is an empty cache; [`Cache::summary`] says which part of the key changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::php_rules::RuleError;

/// The checked file and, for a trait's body, the class that uses it and the trait.
pub(crate) type Check = (PathBuf, Option<(String, String)>);

/// A part of the run key: what it is (`caller key`, `composer.lock`, a neon file...) and the hash of its content.
pub(crate) type Part = (String, String);

#[derive(Serialize, Deserialize)]
struct Stored {
    base: Vec<Part>,
    loaded: Vec<usize>,
    /// The hash of each loaded file, in `loaded`'s order.
    loaded_hashes: Vec<String>,
    paths: Vec<PathBuf>,
    entries: Vec<StoredEntry>,
}

#[derive(Serialize, Deserialize)]
struct StoredEntry {
    file: usize,
    context: Option<(String, String)>,
    deps: Vec<usize>,
    digest: String,
    errors: Vec<RuleError>,
}

#[derive(Clone)]
struct Entry {
    deps: Vec<PathBuf>,
    digest: String,
    errors: Vec<RuleError>,
}

pub(crate) struct Cache {
    file: PathBuf,
    /// The run key without the loaded files, and its hash.
    base: Vec<Part>,
    base_key: String,
    /// Why the stored entries don't hold, when they don't.
    rejected: Option<String>,
    hits: AtomicUsize,
    misses: AtomicUsize,
    old: HashMap<Check, Entry>,
    old_loaded: Vec<PathBuf>,
    kept: Mutex<HashMap<Check, Entry>>,
    hashes: Mutex<HashMap<PathBuf, String>>,
}

/// Adds `bytes` to `h`, length-prefixed so two different sequences never hash alike.
fn part(h: &mut blake3::Hasher, bytes: &[u8]) {
    h.update(&(bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

fn hex(h: &blake3::Hasher) -> String {
    h.finalize().to_hex()[..32].to_string()
}

impl Cache {
    /// The cache in `file` for the run key `base`.
    pub(crate) fn open(file: PathBuf, base: Vec<Part>) -> Cache {
        let mut h = blake3::Hasher::new();
        for (name, hash) in &base {
            part(&mut h, name.as_bytes());
            part(&mut h, hash.as_bytes());
        }
        let mut cache = Cache {
            file,
            base_key: hex(&h),
            base,
            rejected: None,
            hits: AtomicUsize::new(0),
            misses: AtomicUsize::new(0),
            old: HashMap::new(),
            old_loaded: Vec::new(),
            kept: Mutex::default(),
            hashes: Mutex::default(),
        };
        let Ok(bytes) = std::fs::read(&cache.file) else {
            cache.rejected = Some("no cache file".to_string());
            return cache;
        };
        let reject = |mut cache: Cache, why: String| {
            cache.rejected = Some(why);
            cache
        };
        let Ok(stored) = serde_json::from_slice::<Stored>(&bytes) else {
            return reject(cache, "unreadable or from an older Pun".to_string());
        };
        let path = |i: &usize| stored.paths.get(*i).cloned();
        let Some(loaded) = stored.loaded.iter().map(path).collect::<Option<Vec<_>>>() else {
            return reject(cache, "unreadable".to_string());
        };
        if let Some(name) = changed(&stored.base, &cache.base) {
            return reject(cache, format!("run key changed: {name}"));
        }
        if loaded.len() != stored.loaded_hashes.len() {
            return reject(cache, "unreadable".to_string());
        }
        let changed_file =
            (loaded.iter().zip(&stored.loaded_hashes)).find(|(f, h)| cache.hash(f) != **h);
        if let Some((file, _)) = changed_file {
            let why = format!("run key changed: loaded file {}", file.display());
            return reject(cache, why);
        }
        for e in &stored.entries {
            let (Some(file), Some(deps)) = (path(&e.file), e.deps.iter().map(path).collect())
            else {
                return reject(cache, "unreadable".to_string());
            };
            let entry = Entry {
                deps,
                digest: e.digest.clone(),
                errors: e.errors.clone(),
            };
            cache.old.insert((file, e.context.clone()), entry);
        }
        cache.old_loaded = loaded;
        cache
    }

    /// One line for `-v`: the checks answered from the cache and those run, and why the stored entries didn't hold.
    pub(crate) fn summary(&self) -> String {
        let (hits, misses) = (
            self.hits.load(Ordering::Relaxed),
            self.misses.load(Ordering::Relaxed),
        );
        let why = (self.rejected.as_ref()).map_or_else(String::new, |why| format!(" ({why})"));
        format!("php-rules cache: {hits} hits, {misses} misses{why}")
    }

    /// The hash of `path`'s content (`-` when it can't be read), once per run.
    fn hash(&self, path: &Path) -> String {
        let hashes = || self.hashes.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(h) = hashes().get(path) {
            return h.clone();
        }
        let h = std::fs::read(path).map_or_else(
            |_| "-".to_string(),
            |bytes| blake3::hash(&bytes).to_hex()[..32].to_string(),
        );
        hashes().insert(path.to_path_buf(), h.clone());
        h
    }

    fn digest(&self, check: &Check, deps: &[PathBuf]) -> String {
        let mut h = blake3::Hasher::new();
        part(&mut h, self.base_key.as_bytes());
        for file in std::iter::once(&check.0).chain(deps) {
            part(&mut h, file.as_os_str().as_encoded_bytes());
            part(&mut h, self.hash(file).as_bytes());
        }
        if let Some((class, trait_name)) = &check.1 {
            part(&mut h, class.as_bytes());
            part(&mut h, trait_name.as_bytes());
        }
        hex(&h)
    }

    /// The errors `check` found in an earlier run, when neither its file nor a file it depends on changed since.
    pub(crate) fn hit(&self, check: &Check) -> Option<Vec<RuleError>> {
        let e = self
            .old
            .get(check)
            .filter(|e| self.digest(check, &e.deps) == e.digest);
        let Some(e) = e else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        self.hits.fetch_add(1, Ordering::Relaxed);
        let errors = e.errors.clone();
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.insert(check.clone(), e.clone());
        Some(errors)
    }

    /// Records what `check` found, depending on `deps`.
    pub(crate) fn keep(&self, check: Check, deps: Vec<PathBuf>, errors: Vec<RuleError>) {
        let digest = self.digest(&check, &deps);
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.insert(
            check,
            Entry {
                deps,
                digest,
                errors,
            },
        );
    }

    /// Writes this run's entries, and the earlier ones of checks it didn't make whose file still exists, with the
    /// files the workers loaded (`loaded`, plus those of earlier runs). Through a temporary file, so a concurrent run
    /// reads the old cache or the new one, never a mix.
    pub(crate) fn save(&self, loaded: Vec<PathBuf>) {
        let mut entries =
            std::mem::take(&mut *self.kept.lock().unwrap_or_else(PoisonError::into_inner));
        for (check, e) in &self.old {
            if !entries.contains_key(check) && check.0.exists() {
                entries.insert(check.clone(), e.clone());
            }
        }
        let mut loaded: Vec<PathBuf> = loaded.into_iter().chain(self.old_loaded.clone()).collect();
        loaded.sort();
        loaded.dedup();
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut index: HashMap<PathBuf, usize> = HashMap::new();
        let mut at = |p: &PathBuf| {
            *index.entry(p.clone()).or_insert_with(|| {
                paths.push(p.clone());
                paths.len() - 1
            })
        };
        let mut entries: Vec<_> = entries.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let entries = (entries.into_iter())
            .map(|((file, context), e)| StoredEntry {
                file: at(&file),
                context,
                deps: e.deps.iter().map(&mut at).collect(),
                digest: e.digest,
                errors: e.errors,
            })
            .collect();
        let stored = Stored {
            base: self.base.clone(),
            loaded_hashes: loaded.iter().map(|f| self.hash(f)).collect(),
            loaded: loaded.iter().map(&mut at).collect(),
            paths,
            entries,
        };
        if let Ok(bytes) = serde_json::to_vec(&stored) {
            let _ = write(&self.file, &bytes);
        }
    }
}

/// The name of the first part of the run key `new` whose hash differs from `old`'s, or that only one of them has.
fn changed(old: &[Part], new: &[Part]) -> Option<String> {
    let differs = |a: &[Part], b: &[Part]| {
        (a.iter())
            .find(|(name, hash)| !b.iter().any(|(n, h)| n == name && h == hash))
            .map(|(name, _)| name.clone())
    };
    differs(new, old).or_else(|| differs(old, new))
}

/// Writes `bytes` to `file` owner-only (the errors quote the project's code) through a temporary sibling.
fn write(file: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let dir = file.parent().unwrap_or(Path::new("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut out| out.write_all(bytes))
        .and_then(|()| std::fs::rename(&tmp, file));
    written.inspect_err(|_| drop(std::fs::remove_file(&tmp)))
}

/// The run key without the loaded files: the named `parts`, and the content of the `neon` files (relative to `root`)
/// with their includes and of Composer's lock file and installed packages, each named by its path.
pub(crate) fn run_key(root: &Path, neon: &[PathBuf], parts: &[(&str, &str)]) -> Vec<Part> {
    let hash = |bytes: &[u8]| blake3::hash(bytes).to_hex()[..32].to_string();
    let mut key: Vec<Part> = (parts.iter())
        .map(|(name, p)| (name.to_string(), hash(p.as_bytes())))
        .collect();
    let composer = ["composer.lock", "vendor/composer/installed.json"].map(|f| root.join(f));
    for file in neon_files(root, neon).iter().chain(&composer) {
        let name = file
            .strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string();
        key.push((name, hash(&std::fs::read(file).unwrap_or_default())));
    }
    key
}

/// The neon files `neon` (relative to `root`) and the files they include, recursively. An include with a parameter
/// (`%rootDir%/...`) is PHPStan's own, which the run key covers through Composer's lock file.
fn neon_files(root: &Path, neon: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut pending: Vec<PathBuf> = neon.iter().map(|n| root.join(n)).collect();
    while let Some(file) = pending.pop() {
        if out.contains(&file) {
            continue;
        }
        let includes = (std::fs::read(&file).ok())
            .and_then(|bytes| pun_phpstan::neon::decode_bytes(&bytes).ok())
            .and_then(|v| v.as_array().and_then(|a| a.get_str("includes")).cloned());
        let dir = file.parent().unwrap_or(root).to_path_buf();
        out.push(file);
        let Some(includes) = includes.as_ref().and_then(|i| i.as_array()) else {
            continue;
        };
        pending.extend(
            (includes.values())
                .filter_map(|i| i.as_str())
                .filter(|i| !i.contains('%'))
                .map(|i| dir.join(i)),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cache in `dir` for the run key `base`, after a run that checked `a.php` with no errors and loaded `rule.php`.
    fn warm(dir: &Path, base: &[(&str, &str)]) -> Check {
        let check: Check = (dir.join("a.php"), None);
        let cache = Cache::open(dir.join("cache.json"), run_key(dir, &[], base));
        assert_eq!(
            cache.summary(),
            "php-rules cache: 0 hits, 0 misses (no cache file)"
        );
        assert!(cache.hit(&check).is_none());
        cache.keep(check.clone(), Vec::new(), Vec::new());
        cache.save(vec![dir.join("rule.php")]);
        check
    }

    #[test]
    fn says_why_the_entries_did_not_hold() {
        let dir = std::env::temp_dir().join(format!("pun-php-rules-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.php"), "<?php\n").unwrap();
        std::fs::write(dir.join("rule.php"), "<?php\n").unwrap();
        let base = [("caller key", "pun 1"), ("worker", "w")];
        let check = warm(&dir, &base);
        let open =
            |base: &[(&str, &str)]| Cache::open(dir.join("cache.json"), run_key(&dir, &[], base));

        let cache = open(&base);
        assert_eq!(cache.hit(&check), Some(Vec::new()));
        assert_eq!(cache.summary(), "php-rules cache: 1 hits, 0 misses");

        let cache = open(&[("caller key", "pun 2"), ("worker", "w")]);
        assert!(cache.hit(&check).is_none());
        assert_eq!(
            cache.summary(),
            "php-rules cache: 0 hits, 1 misses (run key changed: caller key)"
        );

        std::fs::write(dir.join("composer.lock"), "{}").unwrap();
        assert!(
            open(&base)
                .summary()
                .ends_with("(run key changed: composer.lock)")
        );
        std::fs::remove_file(dir.join("composer.lock")).unwrap();

        std::fs::write(dir.join("rule.php"), "<?php // changed\n").unwrap();
        let rule = dir.join("rule.php");
        let want = format!("(run key changed: loaded file {})", rule.display());
        assert!(open(&base).summary().ends_with(&want));

        std::fs::write(
            dir.join("cache.json"),
            r#"{"key": "the format before the parts"}"#,
        )
        .unwrap();
        assert!(
            open(&base)
                .summary()
                .ends_with("(unreadable or from an older Pun)")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
