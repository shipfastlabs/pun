#![expect(
    clippy::print_stderr,
    reason = "each script is echoed before it runs, as Composer echoes it"
)]

use crate::Env;
use crate::error::{Context, Error, Result, bail};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

#[expect(
    clippy::struct_excessive_bools,
    reason = "one bool per option of Composer's that changes how scripts run"
)]
pub struct Runner {
    pub base: PathBuf,
    pub vendor: String,
    pub bin_dir: String,
    pub manifest: Map<String, Value>,
    pub dev_mode: bool,
    pub php: String,
    /// Print each command before it runs; Composer doesn't when a script runs as its own command (`composer test`).
    pub echo: bool,
    /// Composer's `process-timeout` in seconds for each command; 0 for none. Callbacks run without one, as they run
    /// inside Composer there.
    pub timeout: u64,
    /// `-q`: print nothing, not even what the commands print, but the warnings and errors Composer prints anyway.
    pub quiet: bool,
    /// `-v`: print each command, and each reference to a script, with its event's name.
    pub verbose: bool,
    /// Give the commands `COMPOSER_DEV_MODE`, as Composer's `run-script` and installs do; `exec` doesn't.
    pub dev_mode_env: bool,
    path: String,
    env: Vec<(String, Option<String>)>,
    stack: Vec<String>,
}

impl Runner {
    pub fn new(
        base: &Path,
        vendor: &str,
        bin_dir: &str,
        manifest: &Map<String, Value>,
        dev_mode: bool,
        env: Env,
    ) -> Self {
        Runner {
            base: base.to_path_buf(),
            vendor: vendor.to_string(),
            bin_dir: bin_dir.to_string(),
            manifest: manifest.clone(),
            dev_mode,
            php: which(
                &php_binary(env("PHP_BINARY")),
                &env("PATH").unwrap_or_default(),
            ),
            echo: true,
            timeout: process_timeout(
                manifest,
                global_timeout(crate::fetch::composer_dirs_in(env).map(|(home, _)| home)).as_ref(),
                env("COMPOSER_PROCESS_TIMEOUT"),
            ),
            quiet: false,
            verbose: false,
            dev_mode_env: true,
            path: env("PATH").unwrap_or_default(),
            env: vec![],
            stack: vec![],
        }
    }

    /// `composer exec`: `binary` (with `args`) as the one command of Composer's `__exec_command` event, which prints
    /// no `> binary` line.
    pub fn exec(&mut self, binary: &str, args: &[String]) -> Result<()> {
        self.stack.push(EXEC_EVENT.to_string());
        let result = self.run(EXEC_EVENT, binary, args);
        self.stack.pop();
        result
    }

    /// The `> command` line Composer prints before it runs `command` for `event`: with the event's name under `-v`;
    /// else none for `exec`'s own event, or a script that runs as its own command.
    fn announce(&self, event: &str, command: &str) {
        if self.quiet {
            return;
        }
        if self.verbose {
            eprintln!("> {event}: {command}");
        } else if self.echo && event != EXEC_EVENT {
            eprintln!("> {command}");
        }
    }

    fn listeners(&self, event: &str) -> Vec<String> {
        match self.manifest.get("scripts").and_then(|s| s.get(event)) {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => vec![],
        }
    }

    pub fn dispatch(&mut self, event: &str, args: &[String]) -> Result<()> {
        if self.stack.iter().any(|e| e == event) {
            bail!("Circular call to script handler '{event}' detected");
        }
        self.stack.push(event.to_string());
        let result = (|| {
            for callable in self.listeners(event) {
                self.run(event, &callable, args)?;
            }
            Ok(())
        })();
        self.stack.pop();
        result
    }

    fn run(&mut self, event: &str, callable: &str, args: &[String]) -> Result<()> {
        let (callable, args) = if callable.contains("@no_additional_args") {
            (
                callable
                    .replace(" @no_additional_args", "")
                    .replace("@no_additional_args", ""),
                &[][..],
            )
        } else {
            (callable.to_string(), args)
        };
        if callable.is_empty() {
            return Ok(());
        }
        let quoted = args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ");
        if let Some(rest) = callable.strip_prefix("@putenv ") {
            match rest.split_once('=') {
                Some((k, v)) => self.env.push((k.to_string(), Some(v.to_string()))),
                None => self.env.push((rest.to_string(), None)),
            }
            return Ok(());
        }
        if callable.starts_with('@')
            && !callable.starts_with("@php ")
            && !callable.starts_with("@composer ")
        {
            let mut words = split_args(&callable[1..]);
            let name = if words.is_empty() {
                String::new()
            } else {
                words.remove(0)
            };
            let mut forwarded = vec![];
            let mut had = false;
            for w in words {
                if w == "@additional_args" {
                    forwarded.extend(args.iter().cloned());
                    had = true;
                } else {
                    forwarded.push(w);
                }
            }
            if !had {
                forwarded.extend(args.iter().cloned());
            }
            if self.verbose && !self.quiet {
                // `$formattedEventNameWithArgs`: the event's name, and its arguments in brackets.
                let with_args = if args.is_empty() {
                    event.to_string()
                } else {
                    format!("{event} ({})", args.join(", "))
                };
                eprintln!("> {with_args}: {callable}");
            }
            if self.listeners(&name).is_empty() {
                eprintln!("You made a reference to a non-existent script {callable}");
            }
            return self
                .dispatch(&name, &forwarded)
                .with_context(|| format!("Script {callable} was called via {event}"));
        }
        if !callable.contains(' ') && callable.contains("::") {
            // Composer's own callback, which turns the timeout off for the commands after it.
            if callable
                .trim_start_matches('\\')
                .eq_ignore_ascii_case("Composer\\Config::disableProcessTimeout")
            {
                self.announce(event, &callable);
                self.timeout = 0;
                return Ok(());
            }
            return self.callback(event, &callable, args);
        }
        let with_args = |c: &str| match (c.contains("@additional_args"), quoted.is_empty()) {
            (true, _) => c.replace("@additional_args", &quoted),
            (false, true) => c.to_string(),
            (false, false) => format!("{c} {quoted}"),
        };
        let exec = if let Some(rest) = callable.strip_prefix("@composer ") {
            format!("composer {}", with_args(rest))
        } else if let Some(rest) = callable.strip_prefix("@php ") {
            format!("{} {}", quote(&self.php), self.php_script(&with_args(rest)))
        } else {
            self.root_binary(&callable, with_args(&callable))
        };
        self.announce(event, &with_args(&callable));
        // `/bin/sh`, as Symfony's Process starts it for Composer: the child's PATH puts the bin dir first. PHP's
        // proc_open names it `sh` (`execl("/bin/sh", "sh", "-c", ...)`), which the shell's own errors start with.
        let mut command = self.command("/bin/sh");
        std::os::unix::process::CommandExt::arg0(&mut command, "sh");
        // Composer gives the script the terminal only when its own output is one; otherwise it reads the script's
        // output through pipes and forwards it (ProcessExecutor::executeTty())
        let pipe = !self.quiet && !std::io::IsTerminal::is_terminal(&std::io::stdout());
        let status = wait(command.arg("-c").arg(&exec), &exec, self.timeout, pipe)?;
        failed(&callable, event, status)
    }

    /// `doDispatch`: a command that a binary of the root's own `bin` ends with, at a word boundary, runs through the
    /// interpreter that binary's shebang names, else php (`BinaryInstaller::determineBinaryCaller`), from its path.
    fn root_binary(&self, callable: &str, exec: String) -> String {
        let bins: Vec<&str> = match self.manifest.get("bin") {
            Some(Value::String(bin)) => vec![bin.as_str()],
            Some(Value::Array(bins)) => bins.iter().filter_map(Value::as_str).collect(),
            _ => return exec,
        };
        let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        // `preg_replace('{^callable}')`: only a command that starts with it, which one with `@additional_args` may not.
        let Some(bin) = bins.into_iter().find(|bin| {
            exec.starts_with(callable)
                && bin.strip_suffix(callable).is_some_and(|before| {
                    word(callable.as_bytes()[0]) != before.bytes().last().is_some_and(word)
                })
        }) else {
            return exec;
        };
        let caller = crate::bins::binary_caller(bin, &self.base.join(bin));
        format!("{caller} {bin}{}", &exec[callable.len()..])
    }

    fn callback(&mut self, event: &str, callable: &str, args: &[String]) -> Result<()> {
        self.announce(event, callable);
        let runner = Helper::new(
            "script-callback.php",
            include_str!("../assets/script-callback.php"),
        )?;
        let status = (self.command(&self.php.clone()))
            .arg(&runner.path)
            .arg(&self.vendor)
            .arg(event)
            .arg(if self.dev_mode { "1" } else { "0" })
            .arg(callable)
            .args(args)
            .env(
                "PUN_ROOT_PACKAGE",
                Value::Object(self.manifest.clone()).to_string(),
            )
            .status();
        drop(runner);
        let status = status.with_context(|| format!("can't run {} for {callable}", self.php))?;
        failed(callable, event, status)
    }

    /// Composer's `@php <script>`: a first word with no quote or slash that is no file in the project is looked up
    /// on the PATH, the bin dir first, so `@php phpstan` runs `vendor/bin/phpstan` on Composer's PHP.
    fn php_script(&self, path_and_args: &str) -> String {
        let end = (path_and_args
            .find(|c: char| matches!(c, '\'' | '"' | '/' | '\\') || c.is_whitespace()))
        .unwrap_or(path_and_args.len());
        let word = &path_and_args[..end];
        if word.is_empty() || self.base.join(word).exists() {
            return path_and_args.to_string();
        }
        let found = which(word, &format!("{}:{}", self.bin_dir, self.path));
        format!("{found}{}", &path_and_args[end..])
    }

    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        c.current_dir(&self.base);
        let path = &self.path;
        if Path::new(&self.bin_dir).is_dir() && !path.split(':').any(|p| p == self.bin_dir) {
            c.env("PATH", format!("{}:{path}", self.bin_dir));
        }
        if self.dev_mode_env {
            c.env("COMPOSER_DEV_MODE", if self.dev_mode { "1" } else { "0" });
        }
        if self.quiet {
            c.stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
        c.env("PHP_BINARY", &self.php);
        for (k, v) in &self.env {
            match v {
                Some(v) => c.env(k, v),
                None => c.env_remove(k),
            };
        }
        c
    }
}

/// The event `composer exec` dispatches its binary on.
const EXEC_EVENT: &str = "__exec_command";

/// The events of Composer's `ScriptEvents`: a script named after one runs on that event and is no command of its own.
pub const SCRIPT_EVENTS: [&str; 12] = [
    "pre-install-cmd",
    "post-install-cmd",
    "pre-update-cmd",
    "post-update-cmd",
    "pre-status-cmd",
    "post-status-cmd",
    "pre-archive-cmd",
    "post-archive-cmd",
    "pre-autoload-dump",
    "post-autoload-dump",
    "post-root-package-install",
    "post-create-project-cmd",
];

/// Runs `command` to its end, or stops it after `timeout` seconds (0, or more than the clock can hold, for never) as
/// Symfony's Process does: SIGTERM, then SIGKILL when it is still running a moment later. Composer then fails with the
/// Process's message. With `pipe`, the child's output goes through Pun, which forwards it until the child ends, as
/// the Process reads its pipes only while it waits: a process the child left running writes nowhere afterwards.
fn wait(
    command: &mut Command,
    exec: &str,
    timeout: u64,
    pipe: bool,
) -> Result<std::process::ExitStatus> {
    use std::time::{Duration, Instant};
    let limit = Duration::from_secs(timeout);
    let limit = (timeout != 0 && Instant::now().checked_add(limit).is_some()).then_some(limit);
    if pipe {
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }
    let mut child = command.spawn()?;
    let forwarding = pipe.then(|| forward(&mut child));
    let exited = exit_signal(child.id());
    let timed_out = limit.is_some_and(|limit| exited.recv_timeout(limit).is_err());
    if timed_out {
        if let Ok(pid) = libc::pid_t::try_from(child.id()) {
            // SAFETY: `kill` takes any pid and signal; only this thread reaps the child, which it hasn't yet, so the
            // pid is still the child's.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        if exited.recv_timeout(Duration::from_millis(1)).is_err() {
            let _ = child.kill();
        }
    }
    let status = child.wait();
    if let Some((stop, done)) = forwarding {
        // the output the child wrote before it ended, which the Process reads once more after it exits
        for _ in 0..2 {
            if done.recv_timeout(Duration::from_millis(100)).is_err() {
                break;
            }
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    if !timed_out {
        return Ok(status?);
    }
    eprintln!("The following exception is caused by a process timeout");
    eprintln!("Check https://getcomposer.org/doc/06-config.md#process-timeout for details");
    bail!("The process \"{exec}\" exceeded the timeout of {timeout} seconds.")
}

/// Copies the child's piped stdout and stderr to Pun's own until the returned flag is set. Each copy reports on the
/// channel when its pipe closes.
fn forward(
    child: &mut std::process::Child,
) -> (
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::sync::mpsc::Receiver<()>,
) {
    use std::io::{Read, Write};
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (sender, done) = std::sync::mpsc::channel();
    let pipes = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    ];
    for (pipe, is_stderr) in pipes.into_iter().zip([false, true]) {
        let Some(mut pipe) = pipe else { continue };
        let (stop, sender) = (stop.clone(), sender.clone());
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            while let Ok(n @ 1..) = pipe.read(&mut buffer) {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let mut out: Box<dyn Write> = if is_stderr {
                    Box::new(std::io::stderr())
                } else {
                    Box::new(std::io::stdout())
                };
                let _ = out.write_all(&buffer[..n]).and_then(|()| out.flush());
            }
            let _ = sender.send(());
        });
    }
    (stop, done)
}

/// A message when process `pid` exits, from a thread that waits for it without reaping it (`WNOWAIT`): the caller
/// still owns the exit status, and the pid stays the child's until the caller reaps it.
fn exit_signal(pid: u32) -> std::sync::mpsc::Receiver<()> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        loop {
            // SAFETY: `info` is a zeroed `siginfo_t` that `waitid` only writes into, and it outlives the call.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: as above; `waitid` with WNOWAIT leaves the child to be reaped by its owner.
            let ret = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    &raw mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            if ret == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break;
            }
        }
        let _ = sender.send(());
    });
    receiver
}

/// The `process-timeout` of the global Composer config (`$COMPOSER_HOME/config.json`), which composer.json overrides.
fn global_timeout(home: Option<PathBuf>) -> Option<Value> {
    let home = home?;
    let config: Value =
        serde_json::from_slice(&std::fs::read(home.join("config.json")).ok()?).ok()?;
    config.get("config")?.get("process-timeout").cloned()
}

/// Composer's `process-timeout`, never below 0: `COMPOSER_PROCESS_TIMEOUT` read as PHP's `(int)` reads it, else
/// composer.json's `config.process-timeout`, else the global config's, else 300. Composer's schema takes only an
/// integer there and refuses to load any other value; Pun, which doesn't check the schema, passes over it.
fn process_timeout(
    manifest: &Map<String, Value>,
    global: Option<&Value>,
    env: Option<String>,
) -> u64 {
    let project = manifest
        .get("config")
        .and_then(|c| c.get("process-timeout"));
    let value = match env {
        Some(v) => php_int(&v),
        None => (project.and_then(Value::as_i64))
            .or_else(|| global.and_then(Value::as_i64))
            .unwrap_or(300),
    };
    u64::try_from(value).unwrap_or(0)
}

/// PHP's `(int)` of a string: leading whitespace, a sign and the digits after it; 0 without digits.
fn php_int(s: &str) -> i64 {
    let s = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(digits.len());
    let n = digits[..end]
        .parse::<i64>()
        .unwrap_or(if end == 0 { 0 } else { i64::MAX });
    if negative { -n } else { n }
}

/// Composer's error for a script that exited with `status`, carrying its code: a shell killed by a signal exits
/// 128 plus the signal, as Symfony's Process reports it.
fn failed(callable: &str, event: &str, status: std::process::ExitStatus) -> Result<()> {
    use std::os::unix::process::ExitStatusExt;
    if status.success() {
        return Ok(());
    }
    let code = (status.code())
        .or_else(|| status.signal().map(|s| 128 + s))
        .unwrap_or(1);
    Err(Error::Script {
        message: format!(
            "Script {callable} handling the {event} event returned with error code {code}"
        ),
        code,
    })
}

/// A PHP helper pun writes for one run, in a directory made for it alone and readable only by this user, so no other
/// account can put its own code at the path PHP opens. The directory goes when the helper is dropped.
pub(crate) struct Helper {
    dir: PathBuf,
    pub path: PathBuf,
}

impl Helper {
    pub(crate) fn new(name: &str, source: &str) -> std::io::Result<Helper> {
        use std::io::Write;
        use std::os::unix::fs::DirBuilderExt;
        let nanos = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
            .map_or(0, |d| d.as_nanos());
        let mut attempt = 0;
        loop {
            let dir = std::env::temp_dir()
                .join(format!("pun-{}-{nanos:x}-{attempt}", std::process::id()));
            // `mkdir` fails on any existing entry, a link included, so the directory is new and ours.
            match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    let helper = Helper {
                        path: dir.join(name),
                        dir,
                    };
                    (std::fs::OpenOptions::new().write(true).create_new(true))
                        .open(&helper.path)?
                        .write_all(source.as_bytes())?;
                    return Ok(helper);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => {
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `program`'s path in `path`, the PATH pun started with, so a bin the scripts' PATH puts first can't stand in for
/// PHP (Composer runs its own `PHP_BINARY`); `program` itself when it has a slash or isn't found.
fn which(program: &str, path: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    if program.contains('/') {
        return program.to_string();
    }
    (path.split(':').filter(|d| !d.is_empty()))
        .map(|d| Path::new(d).join(program))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .map_or_else(|| program.to_string(), |p| p.to_string_lossy().into_owned())
}

fn php_binary(env: Option<String>) -> String {
    env.filter(|p| !p.is_empty())
        .unwrap_or_else(|| "php".into())
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn split_args(command: &str) -> Vec<String> {
    let (mut out, mut word, mut quote, mut started) = (vec![], String::new(), None::<char>, false);
    for ch in command.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(ch);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, c) => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pun-scripts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("vendor/bin")).unwrap();
        d
    }

    /// The process's environment with `COMPOSER_HOME` at `dir/home` and no `COMPOSER_PROCESS_TIMEOUT`, so the
    /// developer's own Composer config can't change what a test sees.
    fn env(dir: &Path) -> impl Fn(&str) -> Option<String> {
        let home = dir.join("home").to_string_lossy().into_owned();
        move |k| match k {
            "COMPOSER_HOME" => Some(home.clone()),
            "COMPOSER_PROCESS_TIMEOUT" => None,
            _ => std::env::var(k).ok(),
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "a test helper called with temporaries"
    )]
    fn run(dir: &Path, scripts: Value, event: &str) -> Result<()> {
        let manifest = json!({"scripts": scripts}).as_object().unwrap().clone();
        let vendor = dir.join("vendor").to_string_lossy().into_owned();
        let bin = dir.join("vendor/bin").to_string_lossy().into_owned();
        Runner::new(dir, &vendor, &bin, &manifest, true, &env(dir)).dispatch(event, &[])
    }

    #[test]
    fn commands_references_and_environment() {
        let d = project("run");
        for (bin, line) in [("tool", "tool"), ("composer", "composer $*")] {
            std::fs::write(
                d.join("vendor/bin").join(bin),
                format!("#!/bin/sh\necho {line} >> out.txt\n"),
            )
            .unwrap();
            std::fs::set_permissions(
                d.join("vendor/bin").join(bin),
                std::os::unix::fs::PermissionsExt::from_mode(0o755),
            )
            .unwrap();
        }
        let scripts = json!({
            "a": ["@putenv FOO=bar", "echo \"$FOO $COMPOSER_DEV_MODE\" > out.txt", "@b 'x y'", "tool", "@composer show x", "@putenv FOO", "echo \"[$FOO]\" >> out.txt"],
            "b": "echo b >> out.txt",
        });
        run(&d, scripts, "a").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("out.txt")).unwrap(),
            "bar 1\nb x y\ntool\ncomposer show x\n[]\n"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn arguments_like_composer() {
        let d = project("args");
        std::fs::remove_dir_all(d.join("vendor/bin")).unwrap();
        let manifest = json!({"scripts": {
            "a": ["@b @additional_args  extra ", "echo c @additional_args >> out.txt", "echo n @no_additional_args >> out.txt", "@php -r 'echo \"php\\n\";' >> out.txt", "@missing", "@"],
            "b": "echo b >> out.txt",
        }});
        let manifest = manifest.as_object().unwrap().clone();
        let vendor = d.join("vendor").to_string_lossy().into_owned();
        let bin = d.join("vendor/bin").to_string_lossy().into_owned();
        let args = ["one".to_string(), "two words".to_string()];
        Runner::new(&d, &vendor, &bin, &manifest, true, &env(&d))
            .dispatch("a", &args)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("out.txt")).unwrap(),
            "b one two words extra\nc one two words\nn\nphp\n"
        );
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(
            (php_binary(None), php_binary(Some(String::new()))),
            ("php".into(), "php".into())
        );
        assert_eq!(
            php_binary(Some("/usr/bin/php8.4".into())),
            "/usr/bin/php8.4"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn failures_use_composers_messages() {
        let d = project("fail");
        let error =
            |scripts: Value, event: &str| format!("{:#}", run(&d, scripts, event).unwrap_err());
        let code = |scripts: Value| run(&d, scripts, "a").unwrap_err().script_code();
        assert_eq!(code(json!({"a": "exit 3"})), Some(3));
        assert_eq!(code(json!({"a": "@b", "b": "exit 4"})), Some(4));
        assert_eq!(code(json!({"a": "kill -9 $$"})), Some(137));
        assert_eq!(code(json!({"a": "@a"})), None);
        assert_eq!(
            error(json!({"a": "exit 3"}), "a"),
            "Script exit 3 handling the a event returned with error code 3"
        );
        assert_eq!(
            error(json!({"a": "@a"}), "a"),
            "Script @a was called via a: Circular call to script handler 'a' detected"
        );
        assert_eq!(
            error(json!({"a": "@b", "b": "exit 4"}), "a"),
            "Script @b was called via a: Script exit 4 handling the b event returned with error code 4"
        );
        run(&d, json!({"a": ""}), "a").unwrap();
        run(&d, json!({}), "post-install-cmd").unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Composer runs scripts with `/bin/sh` and its own PHP; a dependency's `sh` or `php` in the bin dir, which the
    /// scripts' PATH puts first, runs neither.
    #[test]
    fn the_bin_dir_does_not_supply_the_interpreters() {
        let d = project("interpreters");
        for bin in ["sh", "php"] {
            let path = d.join("vendor/bin").join(bin);
            std::fs::write(&path, "#!/bin/sh\necho impostor >> out.txt\n").unwrap();
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        }
        let scripts =
            json!({"a": ["echo shell >> out.txt", "@php -r 'echo \"php\\n\";' >> out.txt"]});
        run(&d, scripts, "a").unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("out.txt")).unwrap(),
            "shell\nphp\n"
        );
        assert_eq!(which("sh", "/nowhere"), "sh");
        assert_eq!(which("./php", "/bin"), "./php");
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The callback runner goes in a new private directory: a link at a guessable name in the shared temporary
    /// directory is neither written through nor run.
    #[test]
    fn helpers_are_private_and_new() {
        use std::os::unix::fs::PermissionsExt;
        let d = project("helper");
        let sentinel = d.join("sentinel");
        std::fs::write(&sentinel, "kept").unwrap();
        let old = std::env::temp_dir().join(format!("pun-script-{}.php", std::process::id()));
        let _ = std::fs::remove_file(&old);
        std::os::unix::fs::symlink(&sentinel, &old).unwrap();
        std::fs::write(d.join("vendor/autoload.php"), "<?php\nfinal class Hook { public static function run() { file_put_contents('out.txt', 'ran'); } }\n").unwrap();
        run(&d, json!({"x": "Hook::run"}), "x").unwrap();
        std::fs::remove_file(&old).unwrap();
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "kept");
        assert_eq!(std::fs::read_to_string(d.join("out.txt")).unwrap(), "ran");

        let (a, b) = (
            Helper::new("a.php", "<?php").unwrap(),
            Helper::new("a.php", "<?php").unwrap(),
        );
        assert_ne!(a.dir(), b.dir());
        let mode = std::fs::metadata(a.dir()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
        assert_eq!(std::fs::read_to_string(&a.path).unwrap(), "<?php");
        let dir = a.dir().to_path_buf();
        drop(a);
        assert!(!dir.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Composer 2.11.2 reads the global `process-timeout` from `$COMPOSER_HOME/config.json`: with 1 there and none in
    /// composer.json, `sleep 2` stops at 1 s. The home is the one the runner's environment names.
    #[test]
    fn global_timeout_from_the_given_composer_home() {
        let d = project("global-timeout");
        std::fs::create_dir_all(d.join("home")).unwrap();
        std::fs::write(
            d.join("home/config.json"),
            r#"{"config": {"process-timeout": 1}}"#,
        )
        .unwrap();
        let err = run(&d, json!({"slow": "sleep 2"}), "slow").unwrap_err();
        assert_eq!(
            err.to_string(),
            "The process \"sleep 2\" exceeded the timeout of 1 seconds."
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Composer 2.11.2 with `process-timeout: 1`: `sleep 3; echo done` stops at 1 s with Symfony's message and prints
    /// nothing; `Composer\Config::disableProcessTimeout` lets the commands after it run on.
    #[test]
    fn commands_stop_at_composers_timeout() {
        let d = project("timeout");
        let manifest = json!({"config": {"process-timeout": 1}, "scripts": {
            "slow": "sleep 3; echo done >> out.txt",
            "off": ["Composer\\Config::disableProcessTimeout", "sleep 2", "echo ok >> out.txt"],
            // Composer takes the class with a leading backslash, and in any case, as PHP does.
            "slash": ["\\Composer\\Config::disableProcessTimeout", "sleep 2", "echo slash >> out.txt"],
            "case": ["composer\\config::DISABLEPROCESSTIMEOUT", "sleep 2", "echo case >> out.txt"],
        }});
        let manifest = manifest.as_object().unwrap().clone();
        let vendor = d.join("vendor").to_string_lossy().into_owned();
        let bin = d.join("vendor/bin").to_string_lossy().into_owned();
        let runner = || Runner::new(&d, &vendor, &bin, &manifest, true, &env(&d));
        let started = std::time::Instant::now();
        let err = runner().dispatch("slow", &[]).unwrap_err();
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert_eq!(
            (err.to_string(), err.script_code()),
            (
                "The process \"sleep 3; echo done >> out.txt\" exceeded the timeout of 1 seconds."
                    .into(),
                None
            )
        );
        // A command that ignores SIGTERM gets SIGKILL, as Symfony's `stop(0)` sends it.
        let stubborn =
            json!({"config": {"process-timeout": 1}, "scripts": {"t": "trap '' TERM; sleep 4"}});
        let stubborn = stubborn.as_object().unwrap().clone();
        let started = std::time::Instant::now();
        let err = Runner::new(&d, &vendor, &bin, &stubborn, true, &env(&d))
            .dispatch("t", &[])
            .unwrap_err();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "{err}"
        );
        std::thread::sleep(std::time::Duration::from_millis(2500));
        assert!(!d.join("out.txt").exists());
        for name in ["off", "slash", "case"] {
            runner().dispatch(name, &[]).unwrap();
        }
        // A timeout beyond what the clock can hold (`--timeout` takes up to PHP_INT_MAX seconds) means none.
        let mut forever = runner();
        forever.timeout = i64::MAX.unsigned_abs();
        forever.dispatch("slow", &[]).unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("out.txt")).unwrap(),
            "ok\nslash\ncase\ndone\n"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Composer 2.11.2 runs `@php tool x` with `vendor/bin/tool` (a PHP file without a shebang) found on the PATH, the
    /// bin dir first; a name that is a file in the project stays as it is.
    #[test]
    fn php_scripts_are_found_on_the_path() {
        let d = project("php-path");
        let tool = d.join("vendor/bin/tool");
        std::fs::write(
            &tool,
            "<?php echo \"tool \", implode(\",\", array_slice($argv, 1)), \"\\n\";\n",
        )
        .unwrap();
        std::fs::set_permissions(&tool, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        std::fs::write(d.join("local"), "<?php echo \"local\\n\";\n").unwrap();
        run(
            &d,
            json!({"a": ["@php tool x >> out.txt", "@php local >> out.txt"]}),
            "a",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("out.txt")).unwrap(),
            "tool x\nlocal\n"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Composer 2.11.2's `process-timeout`: the environment (through PHP's `(int)`, values from
    /// `php -r 'echo (int) "..."'`) over composer.json over the global config over 300, never below 0. A value that
    /// is no integer, which Composer's schema refuses, counts as none.
    #[test]
    fn process_timeout_like_composer() {
        let m = |config: Value| json!({"config": config}).as_object().unwrap().clone();
        let none = Map::new();
        assert_eq!(process_timeout(&none, None, None), 300);
        let home = project("global-config");
        assert_eq!(global_timeout(Some(home.clone())), None);
        std::fs::write(
            home.join("config.json"),
            r#"{"config": {"process-timeout": 60}}"#,
        )
        .unwrap();
        assert_eq!(global_timeout(Some(home.clone())), Some(json!(60)));
        assert_eq!(global_timeout(None), None);
        std::fs::remove_dir_all(&home).unwrap();
        let global = json!(60);
        assert_eq!(process_timeout(&none, Some(&global), None), 60);
        assert_eq!(
            process_timeout(&m(json!({"process-timeout": 5})), Some(&global), None),
            5
        );
        assert_eq!(
            process_timeout(&m(json!({"process-timeout": 0})), None, None),
            0
        );
        assert_eq!(
            process_timeout(&m(json!({"process-timeout": -5})), None, None),
            0
        );
        for refused in [
            json!(1.5),
            json!("20"),
            json!(true),
            json!(null),
            json!([1]),
        ] {
            let config = m(json!({ "process-timeout": refused }));
            assert_eq!(process_timeout(&config, None, None), 300, "{refused}");
            assert_eq!(
                process_timeout(&config, Some(&global), None),
                60,
                "{refused}"
            );
        }
        assert_eq!(
            process_timeout(&m(json!({"process-timeout": 9})), None, Some("0".into())),
            0
        );
        assert_eq!(process_timeout(&none, None, Some("-5".into())), 0);
        for (text, int) in [
            (" 12abc", 12),
            ("+7", 7),
            ("abc", 0),
            ("", 0),
            ("99999999999999999999", i64::MAX),
            ("\t3", 3),
        ] {
            assert_eq!(php_int(text), int, "{text:?}");
        }
    }

    #[test]
    fn php_callbacks() {
        let d = project("callback");
        std::fs::write(
            d.join("vendor/autoload.php"),
            "<?php\nnamespace Acme;\nfinal class Hooks {\n    public static function dump($event) {\n        file_put_contents('out.txt', $event->getName().' '.(int) $event->isDevMode().' '.basename($event->getComposer()->getConfig()->get('vendor-dir')).' '.$event->getComposer()->getPackage()->getName());\n    }\n    public static function fail($event) { return false; }\n}\n",
        )
        .unwrap();
        run(
            &d,
            json!({"post-autoload-dump": "Acme\\Hooks::dump"}),
            "post-autoload-dump",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join("out.txt")).unwrap(),
            "post-autoload-dump 1 vendor __root__"
        );
        let error = format!(
            "{:#}",
            run(&d, json!({"x": "Acme\\Hooks::fail"}), "x").unwrap_err()
        );
        assert_eq!(
            error,
            "Script Acme\\Hooks::fail handling the x event returned with error code 1"
        );
        run(&d, json!({"x": "Acme\\Missing::run"}), "x").unwrap();
        std::fs::remove_dir_all(&d).unwrap();
    }
}
