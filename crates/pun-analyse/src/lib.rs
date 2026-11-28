mod autoload;
mod bitset;
mod carbon;
mod codebase;
mod dependencies;
mod extension;
mod fingerprint;
mod incremental;
pub mod io;
mod larastan;
mod laravel;
mod mago;
mod mockery;
mod parser;
mod project;
mod relative_path;
mod rules;

use std::path::{Path, PathBuf};

use pun_phpstan::config::Resolved;
use rayon::prelude::*;

pub use carbon::Carbon;
pub use codebase::CouldNotReadFile;
pub use extension::AnalyseExtension;
pub use incremental::{CacheStats, Record};
pub use laravel::Laravel;
pub use mago::{Dumped, MagoIssue, MagoRun, run_mago};
pub use mockery::Mockery;
pub use project::{
    ExceptionChecks, Exceptions, MethodRules, Project, ProjectOptions, Reported, StrictRules,
    TooWide,
};
pub use pun_tokens::PhpVersion;
pub use relative_path::RelativePath;
pub use rules::{Disallowed, Larastan, LarastanSwitches, TypeCoverage};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Error {
    pub file: String,
    pub line: i64,
    pub identifier: String,
    pub message: String,
    pub tip: Option<String>,
    pub can_be_ignored: bool,
    pub fixable: bool,
}

pub const IDENTIFIERS: &[&str] = rules::IDENTIFIERS;

pub fn reports(identifier: &str) -> bool {
    rules::reports(identifier)
}

pub fn analyse(project_root: &Path, config: &Resolved) -> Result<Vec<Error>, CouldNotReadFile> {
    run(&Project::from_config(project_root, &config.parameters))
}

pub fn run(project: &Project) -> Result<Vec<Error>, CouldNotReadFile> {
    Ok(analyse_project(project)?.errors)
}

pub struct Analysis {
    pub errors: Vec<Error>,
    pub general: Vec<String>,
    pub fixable_ignored: usize,
}

pub fn analyse_project(project: &Project) -> Result<Analysis, CouldNotReadFile> {
    Ok(analyse_with(project, None)?.0)
}

/// [`analyse_project`] with a result cache: `previous` is the record of an earlier run with the same binary,
/// configuration and flags. Returns this run's record and what it reused.
pub fn analyse_project_cached(
    project: &Project,
    previous: Option<Record>,
) -> Result<(Analysis, Record, CacheStats), CouldNotReadFile> {
    if let Some(previous) = previous.as_ref()
        && incremental::unchanged(project, previous)
    {
        let stats = CacheStats {
            unchanged: true,
            analysed: 0,
            reused: project.analysed.len(),
        };
        let analysis = previous.analysis();
        return Ok((analysis, previous.clone_record(), stats));
    }
    let (analysis, record, stats) = analyse_with(project, Some(previous.as_ref()))?;
    Ok((analysis, record.unwrap_or_default(), stats))
}

/// The analysis, and with `cache` (the previous record, if any) this run's record.
fn analyse_with(
    project: &Project,
    cache: Option<Option<&Record>>,
) -> Result<(Analysis, Option<Record>, CacheStats), CouldNotReadFile> {
    let start = std::time::SystemTime::now();
    let php_version = project.php_version.id();
    let natives = std::thread::spawn(move || rules::Natives::load(php_version));
    let prepare = || -> Result<_, CouldNotReadFile> {
        let (codebase, sources) = codebase::build(project, None)?;
        let natives = natives.join().expect("loading the natives never panics");
        let settings = rules::Checks::of(project, &codebase, natives);
        Ok((codebase, sources, settings))
    };
    let (prepared, setup) = if cache.is_some() {
        mago_codex::reads::setup_recording(prepare)
    } else {
        (prepare(), std::collections::HashMap::new())
    };
    let mut setup = setup;
    let (codebase, sources, settings) = prepared?;
    let files: std::collections::HashMap<_, _> = project
        .analysed
        .iter()
        .chain(&project.scanned)
        .chain(project.autoloaded.iter().map(|a| &a.path))
        .chain(&project.stubs)
        .map(|p| {
            (
                mago_database::file::FileId::new(display(p).as_bytes()),
                p.clone(),
            )
        })
        .collect();
    let run = cache.map(|_| incremental::Run::new(&codebase, &sources, start));
    let project_fingerprint = incremental::project_fingerprint(project);
    let global = {
        let mut h = blake3::Hasher::new();
        h.update(&incremental::settings_fingerprint(project));
        h.update(&settings.fingerprint());
        h.update(&incremental::aliases(&codebase));
        incremental::hash(h.finalize().as_bytes())
    };
    let previous = cache.flatten();
    let valid = match (previous, &run) {
        (Some(previous), Some(run)) if previous.matches(global) => run.valid(previous),
        _ => std::collections::HashMap::new(),
    };
    let checked: Vec<incremental::Checked> = project
        .analysed
        .par_iter()
        .map(|f| {
            if let Some(r) = valid.get(&incremental::key(f)) {
                return incremental::Checked::Reused(*r);
            }
            if cache.is_some() {
                let ((errors, fixable_ignored), reads) = mago_codex::reads::recording_alone(|| {
                    rules::check_file(f, &codebase, &settings, &files, &sources)
                });
                incremental::Checked::Fresh {
                    errors,
                    fixable_ignored,
                    reads: Box::new(reads),
                }
            } else {
                let (errors, fixable_ignored) =
                    rules::check_file(f, &codebase, &settings, &files, &sources);
                incremental::Checked::Fresh {
                    errors,
                    fixable_ignored,
                    reads: Box::default(),
                }
            }
        })
        .collect();
    let stats = CacheStats {
        unchanged: false,
        analysed: (checked.iter())
            .filter(|c| matches!(c, incremental::Checked::Fresh { .. }))
            .count(),
        reused: (checked.iter())
            .filter(|c| matches!(c, incremental::Checked::Reused(_)))
            .count(),
    };
    let per_file: Vec<_> = checked.iter().map(|c| c.errors(previous)).collect();
    let mut fixable_ignored: usize = per_file.iter().map(|(_, n)| n).sum();
    let errors: Vec<_> = (per_file.into_iter().flat_map(|(errors, _)| errors))
        .chain(rules::unused_traits(project, &codebase, &sources))
        .collect();
    let analysed_ids = (project.analysed.iter())
        .map(|p| mago_database::file::FileId::new(display(p).as_bytes()))
        .collect();
    let dependencies = dependencies::collect(&codebase, &files, &analysed_ids, &errors);
    // NotAnalysedTraitRule (level 4) asks whether only files are analysed when an analysed file declares a trait, so
    // it may have skipped the errors an entry ignores: no entry is reported as unmatched then.
    let collected_data_rule_skipped =
        project.only_files && project.level >= 4 && !dependencies.trait_declaring_files.is_empty();
    let errors = rules::renamed_named_arguments(
        errors,
        &sources,
        project
            .options
            .contains(crate::ProjectOptions::REPORT_IGNORES_WITHOUT_COMMENTS),
        project.method_rules.renamed_named_arguments,
    );
    let errors = rules::type_coverage_finish(
        project.type_coverage.as_ref(),
        errors,
        &sources,
        project
            .options
            .contains(crate::ProjectOptions::REPORT_IGNORES_WITHOUT_COMMENTS),
    );
    let analysed: Vec<String> = project.analysed.iter().map(|p| display(p)).collect();
    let errors = rules::conditions_in_traits(errors);
    let errors = rules::impure_points(errors);
    // Larastan's last checks read the translation files and the views: setup paths too, which every result depends on.
    let mut errors = if cache.is_some() {
        let (errors, read) = mago_codex::reads::setup_recording(|| {
            rules::larastan_finish(project.larastan.as_ref(), errors)
        });
        for (key, answer) in read {
            mago_codex::reads::keep_answer(&mut setup, key, answer);
        }
        errors
    } else {
        rules::larastan_finish(project.larastan.as_ref(), errors)
    };
    settings.keep_reported(&mut errors);
    let processed = project.ignore_errors.process(
        errors,
        project.only_files,
        &analysed,
        false,
        collected_data_rule_skipped,
        Some(&dependencies),
    );
    fixable_ignored += processed.ignored.iter().filter(|(e, _)| e.fixable).count();
    let mut errors: Vec<Error> = processed
        .errors
        .into_iter()
        .map(|e| Error {
            file: e.file,
            line: e.line.unwrap_or(0),
            identifier: e.identifier.unwrap_or_default(),
            message: e.message,
            tip: e.tip,
            can_be_ignored: e.can_be_ignored,
            fixable: e.fixable,
        })
        .collect();
    errors.sort_by(|a, b| (&a.file, a.line, &a.message).cmp(&(&b.file, b.line, &b.message)));
    let analysis = Analysis {
        errors,
        general: processed.string_errors,
        fixable_ignored,
    };
    let record = run.map(|run| {
        run.record(
            project_fingerprint,
            global,
            setup,
            &project.analysed,
            &checked,
            previous,
            &analysis,
        )
    });
    std::thread::spawn(move || drop((codebase, sources, settings)));
    Ok((analysis, record, stats))
}

/// A file's size, modification and change times in nanoseconds and inode, or nothing when it is missing. The change
/// time moves on every write and no tool sets it back, so a file rewritten with its old size and modification time
/// still differs.
pub(crate) fn cache_stat(path: &Path) -> Vec<u8> {
    std::fs::metadata(path).map_or_else(|_| Vec::new(), |m| stat_bytes(&m))
}

fn stat_bytes(m: &std::fs::Metadata) -> Vec<u8> {
    use std::os::unix::fs::MetadataExt;
    let (mtime, ctime) = times(m);
    [
        m.len().to_le_bytes().as_slice(),
        &mtime.to_le_bytes(),
        &ctime.to_le_bytes(),
        &m.ino().to_le_bytes(),
    ]
    .concat()
}

/// A file's modification and change times, in nanoseconds since the epoch.
fn times(m: &std::fs::Metadata) -> (i128, i128) {
    use std::os::unix::fs::MetadataExt;
    let nanos = |secs: i64, nsec: i64| i128::from(secs) * 1_000_000_000 + i128::from(nsec);
    (
        nanos(m.mtime(), m.mtime_nsec()),
        nanos(m.ctime(), m.ctime_nsec()),
    )
}

/// [`cache_stat`] for a source the run read after `start`: [`mago_codex::reads::RACY`], a stat no file has, when the file
/// changed in the second before the run or since (git's «racily clean» files), because its contents may not be the
/// ones the run read; the next run then compares its contents.
pub(crate) fn source_stat(path: &Path, start: std::time::SystemTime) -> Vec<u8> {
    let Ok(m) = std::fs::metadata(path) else {
        return Vec::new();
    };
    let start = start
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i128::try_from(d.as_nanos()).unwrap_or(i128::MAX));
    let (mtime, ctime) = times(&m);
    if mtime.max(ctime) >= start - 1_000_000_000 {
        return mago_codex::reads::RACY.to_vec();
    }
    stat_bytes(&m)
}

fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub(crate) fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn php_files(dir: &Path, extensions: &[String]) -> Vec<PathBuf> {
    let Ok(entries) = crate::io::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries
        .flatten()
        .map(|e| {
            let p = e.path();
            let dir = match e.file_type() {
                Ok(t) if !t.is_symlink() => t.is_dir(),
                _ => crate::io::is_dir(&p),
            };
            (p, dir)
        })
        .collect();
    entries.sort();
    let found = |(p, dir): (PathBuf, bool)| {
        if dir {
            php_files(&p, extensions)
        } else if p
            .extension()
            .is_some_and(|e| extensions.iter().any(|x| e == x.as_str()))
        {
            vec![p]
        } else {
            Vec::new()
        }
    };
    // A recording lives on its thread: listing on other workers would lose what it read, or count it for another
    // file.
    if mago_codex::reads::active() {
        return entries.into_iter().flat_map(found).collect();
    }
    entries.into_par_iter().flat_map_iter(found).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source written just before or during the run may hold other contents than the ones the run read: its stat
    /// is the racy marker, never a stat the file could have later. One written well before the run keeps its stat.
    #[test]
    fn a_source_changed_since_the_run_began_is_racy() {
        let dir = std::env::temp_dir().join(format!("pun-source-stat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.php");
        std::fs::write(&path, "<?php\n").unwrap();
        let now = std::time::SystemTime::now();
        assert_eq!(source_stat(&path, now), mago_codex::reads::RACY);
        let later = now + std::time::Duration::from_secs(60);
        assert_eq!(source_stat(&path, later), cache_stat(&path));
        assert_ne!(cache_stat(&path), mago_codex::reads::RACY);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Listing a tree inside a recording counts every subdirectory's listing for that recording, however many
    /// threads the pool has: a recording lives on its thread.
    #[test]
    fn a_tree_listed_inside_a_recording_is_recorded_whole() {
        let dir = std::env::temp_dir().join(format!("pun-php-files-{}", std::process::id()));
        let subdirectories: Vec<PathBuf> = (0..64).map(|i| dir.join(format!("d{i}"))).collect();
        for sub in &subdirectories {
            std::fs::create_dir_all(sub).unwrap();
            std::fs::write(sub.join("a.php"), "<?php\n").unwrap();
        }
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let (files, reads) =
            pool.install(|| mago_codex::reads::recording(|| php_files(&dir, &["php".to_string()])));
        assert_eq!(files.len(), subdirectories.len());
        for sub in &subdirectories {
            let listing = (sub.clone(), mago_codex::reads::PathRead::Listing);
            assert!(reads.paths.contains_key(&listing), "{}", sub.display());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
