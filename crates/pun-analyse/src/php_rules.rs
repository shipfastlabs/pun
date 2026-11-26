//! `[analyse] php-rules` (prototype): the project's own PHPStan rule classes, run by a pool of long-lived `php` workers
//! (`php_rules_worker.php`, protocol in its header) on PHPStan's container and reflection, with the expression types
//! Pun inferred. Experimental, pending shipfastlabs/pun#55: it runs only with `--php-rules` or `PUN_PHP_RULES=1`.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const WORKER: &str = include_str!("php_rules_worker.php");

/// An error a project rule reported.
#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub(crate) struct RuleError {
    pub(crate) message: String,
    pub(crate) line: i64,
    pub(crate) identifier: Option<String>,
    pub(crate) tip: Option<String>,
    #[serde(rename = "nonIgnorable")]
    pub(crate) non_ignorable: bool,
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    token: String,
}

impl Worker {
    fn send(&mut self, message: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{message}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("can't write to the php-rules worker: {e}"))
    }

    /// The worker's next message; output that isn't the worker's goes to stderr.
    fn receive(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.stdout.read_line(&mut line) {
                Ok(0) => return Err("the php-rules worker stopped".to_string()),
                Ok(_) => {}
                Err(e) => return Err(format!("can't read from the php-rules worker: {e}")),
            }
            match line.split_once(&self.token) {
                Some((before, reply)) => {
                    let _ = std::io::stderr().write_all(before.as_bytes());
                    let reply: Value = serde_json::from_str(reply.trim_end())
                        .map_err(|e| format!("the php-rules worker sent {reply:?}: {e}"))?;
                    if let Some(e) = reply.get("error").and_then(Value::as_str) {
                        return Err(e.to_string());
                    }
                    return Ok(reply);
                }
                None => {
                    let _ = std::io::stderr().write_all(line.as_bytes());
                }
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A pool of workers, shared by the analysis threads: each file checks out an idle worker, and a new one starts (in
/// that thread, so several start at once) while fewer than `max` run.
pub struct PhpRules {
    setup: Value,
    root: PathBuf,
    /// `[analyse] php-rules-paths`: the paths a checked file must be in, when set.
    only: Option<Vec<PathBuf>>,
    max: usize,
    pool: Mutex<Pool>,
    idle: Condvar,
    failure: Mutex<Option<String>>,
    identifiers: Mutex<HashSet<String>>,
    cache: Option<crate::php_rules_cache::Cache>,
}

#[derive(Default)]
struct Pool {
    idle: Vec<Worker>,
    started: usize,
    /// A worker failed: the run reports the failure and the others stop.
    stopped: bool,
}

impl PhpRules {
    /// Prepares the workers for the project in `root`: its vendor/autoload.php and vendor/phpstan/phpstan/phpstan.phar,
    /// the rules the `neon` files register but those of the `native` classes, and `paths` as PHPStan's analysed paths
    /// (and, with `only`, the only paths whose files the rules check). At most `max` workers run, each
    /// started when a file needs it. `cache` is the result cache's file and the caller's part of its run key (Pun's
    /// version, binary and config), or `None` to run without it.
    pub fn start(
        root: &Path,
        neon: &[PathBuf],
        native: &[String],
        paths: &[PathBuf],
        only: bool,
        max: usize,
        cache: Option<(PathBuf, &str)>,
    ) -> PhpRules {
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let setup = json!({
            "root": path(root),
            "autoload": path(&root.join("vendor/autoload.php")),
            "phar": path(&root.join("vendor/phpstan/phpstan/phpstan.phar")),
            "neon": neon.iter().map(|n| path(&root.join(n))).collect::<Vec<_>>(),
            "native": native,
            "paths": paths.iter().map(|p| path(p)).collect::<Vec<_>>(),
        });
        let cache = cache.map(|(file, key)| {
            // the setup without the analysed paths: a run on a few files (`pun analyse app/Foo.php`) keeps the cache;
            // the configured paths and `php-rules-paths` are in the caller's key
            let mut setup = setup.clone();
            setup["paths"] = Value::Null;
            let setup = setup.to_string();
            let parts = [("caller key", key), ("worker", WORKER), ("setup", &setup)];
            let key = crate::php_rules_cache::run_key(root, neon, &parts);
            crate::php_rules_cache::Cache::open(file, key)
        });
        PhpRules {
            setup,
            root: root.to_path_buf(),
            only: only.then(|| paths.to_vec()),
            max: max.max(1),
            pool: Mutex::new(Pool::default()),
            idle: Condvar::new(),
            failure: Mutex::new(None),
            identifiers: Mutex::new(HashSet::new()),
            cache,
        }
    }

    /// Starts one worker and waits until it has booted PHPStan.
    fn spawn(&self) -> Result<Worker, String> {
        static WORKERS: AtomicUsize = AtomicUsize::new(0);
        let nanos = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
            .map_or(0, |d| d.as_nanos());
        let n = WORKERS.fetch_add(1, Ordering::Relaxed);
        let token = format!("\u{1e}pun-php-rules-{}-{n}-{nanos}:", std::process::id());
        // `php -r`: the worker needs no file, which a containerized php (lerd) can't read from /tmp
        let code = WORKER.trim_start_matches("<?php");
        // no memory limit, as PHPStan's `--memory-limit=-1`: php's default 128M doesn't parse a large file. pcov and
        // Xdebug off: a php with coverage on (lerd's has pcov.enabled=1) runs the worker about twice as slow, and
        // PHPStan restarts itself without Xdebug for the same reason
        let child = Command::new("php")
            .args([
                "-d",
                "memory_limit=-1",
                "-d",
                "pcov.enabled=0",
                "-d",
                "xdebug.mode=off",
                "-r",
                code,
            ])
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn();
        let mut child = child.map_err(|e| format!("can't run php for the php-rules: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("can't talk to the php-rules worker".to_string());
        };
        let mut worker = Worker {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            token: token.clone(),
        };
        let mut setup = self.setup.clone();
        setup["token"] = json!(token);
        worker.send(&setup)?;
        worker.receive()?;
        Ok(worker)
    }

    /// An idle worker, a new one while fewer than `max` run, or `None` once a worker failed.
    fn checkout(&self) -> Option<Result<Worker, String>> {
        let mut pool = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if pool.stopped {
                return None;
            }
            if let Some(worker) = pool.idle.pop() {
                return Some(Ok(worker));
            }
            if pool.started < self.max {
                pool.started += 1;
                drop(pool);
                return Some(self.spawn());
            }
            pool = (self.idle.wait(pool)).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Keeps the failure for [`PhpRules::failure`] and stops the pool: Pun can't report half the rules' errors.
    fn fail(&self, e: String) {
        let mut pool = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pool.stopped = true;
        pool.idle.clear();
        drop(pool);
        self.idle.notify_all();
        let mut failure = self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        failure.get_or_insert(e);
    }

    /// Runs the rules on `file`, or on the body of a trait it declares in the context of a class that uses it
    /// (`in_trait`: the class and the trait); `type_at(start, end)` answers the worker's type questions with a PHPDoc type. A
    /// failure stops the pool and is kept for [`PhpRules::failure`]. With the result cache, an unchanged check
    /// answers from it, and a new result is kept with `deps()`, the files it depends on.
    pub(crate) fn check(
        &self,
        file: &Path,
        in_trait: Option<(&str, &str)>,
        type_at: impl FnMut(u32, u32) -> Option<String>,
        deps: impl FnOnce() -> Vec<PathBuf>,
    ) -> Vec<RuleError> {
        let check = (
            file.to_path_buf(),
            in_trait.map(|(c, t)| (c.to_string(), t.to_string())),
        );
        let errors = match self.cache.as_ref().and_then(|c| c.hit(&check)) {
            Some(errors) => errors,
            None => {
                let Some(errors) = self.run(file, in_trait, type_at) else {
                    return Vec::new();
                };
                if let Some(cache) = &self.cache {
                    cache.keep(check, deps(), errors.clone());
                }
                errors
            }
        };
        let mut ids = self
            .identifiers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ids.extend(
            errors
                .iter()
                .map(|e| e.identifier.clone().unwrap_or_default()),
        );
        errors
    }

    /// Runs the rules on one check in a worker; `None` when the pool stopped.
    fn run(
        &self,
        file: &Path,
        in_trait: Option<(&str, &str)>,
        mut type_at: impl FnMut(u32, u32) -> Option<String>,
    ) -> Option<Vec<RuleError>> {
        let mut worker = match self.checkout() {
            None => return None,
            Some(Ok(worker)) => worker,
            Some(Err(e)) => {
                self.fail(e);
                return None;
            }
        };
        let mut ask = || -> Result<Vec<RuleError>, String> {
            let mut request = json!({"file": file.to_string_lossy()});
            if let Some((class, trait_name)) = in_trait {
                request["class"] = json!(class);
                request["trait"] = json!(trait_name);
            }
            worker.send(&request)?;
            loop {
                let mut reply = worker.receive()?;
                if let Some([start, end]) = reply
                    .get("type")
                    .and_then(|t| serde_json::from_value::<[u32; 2]>(t.clone()).ok())
                {
                    worker.send(&json!({"type": type_at(start, end)}))?;
                    continue;
                }
                let errors = reply.get_mut("errors").map(Value::take).unwrap_or_default();
                return serde_json::from_value(errors)
                    .map_err(|e| format!("the php-rules worker sent bad errors: {e}"));
            }
        };
        match ask() {
            Ok(errors) => {
                let mut pool = self
                    .pool
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !pool.stopped {
                    pool.idle.push(worker);
                }
                drop(pool);
                self.idle.notify_one();
                Some(errors)
            }
            Err(e) => {
                self.fail(format!("{}: {e}", file.display()));
                None
            }
        }
    }

    /// Writes the result cache, with the PHP files the workers loaded (their rules and bootstrap files), unless a
    /// worker failed. The analysis is over, so every worker is idle.
    pub fn save(&self) {
        let Some(cache) = &self.cache else {
            return;
        };
        let mut loaded = Vec::new();
        let mut pool = self
            .pool
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for worker in &mut pool.idle {
            let files = (worker.send(&json!({"loaded": true})))
                .and_then(|()| worker.receive())
                .map(|mut reply| reply.get_mut("files").map(Value::take).unwrap_or_default());
            match files.map(serde_json::from_value::<Vec<PathBuf>>) {
                Ok(Ok(files)) => loaded.extend(files),
                _ => return,
            }
        }
        drop(pool);
        if self.failure().is_none() {
            cache.save(loaded);
        }
    }

    /// Whether the rules check `file`, an analysed file: it is in `[analyse] php-rules-paths`, when set.
    pub(crate) fn covers(&self, file: &Path) -> bool {
        (self.only.as_ref()).is_none_or(|paths| paths.iter().any(|p| file.starts_with(p)))
    }

    /// The result cache's line for `-v`: its hits and misses, and why it held nothing; `None` without the cache.
    pub fn cache_summary(&self) -> Option<String> {
        self.cache
            .as_ref()
            .map(crate::php_rules_cache::Cache::summary)
    }

    /// Why the worker stopped during the run, if it did.
    pub fn failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether a project rule reported `identifier` (`""` for an error without one) in this run.
    pub fn reported(&self, identifier: &str) -> bool {
        (self
            .identifiers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
        .contains(identifier)
    }
}
