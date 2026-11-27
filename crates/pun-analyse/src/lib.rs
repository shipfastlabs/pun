mod autoload;
mod bitset;
mod carbon;
mod codebase;
mod dependencies;
mod extension;
mod larastan;
mod laravel;
mod mago;
mod mockery;
mod parser;
mod pest;
mod project;
mod relative_path;
mod rules;

use std::path::{Path, PathBuf};

use pun_phpstan::config::Resolved;
use rayon::prelude::*;

pub use carbon::Carbon;
pub use codebase::CouldNotReadFile;
pub use extension::AnalyseExtension;
pub use laravel::Laravel;
pub use mago::{Dumped, MagoIssue, MagoRun, run_mago};
pub use mockery::Mockery;
pub use pest::Pest;
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
    let php_version = project.php_version.id();
    let natives = std::thread::spawn(move || rules::Natives::load(php_version));
    let (codebase, sources) = codebase::build(project, None)?;
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
    let natives = natives.join().expect("loading the natives never panics");
    let settings = rules::Checks::of(project, &codebase, natives);
    let checked: Vec<_> = project
        .analysed
        .par_iter()
        .map(|f| rules::check_file(f, &codebase, &settings, &files, &sources))
        .collect();
    let mut fixable_ignored: usize = checked.iter().map(|(_, n)| n).sum();
    let errors: Vec<_> = (checked.into_iter().flat_map(|(errors, _)| errors))
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
    std::thread::spawn(move || drop((codebase, sources)));
    let analysed: Vec<String> = project.analysed.iter().map(|p| display(p)).collect();
    let errors = rules::conditions_in_traits(errors);
    let errors = rules::impure_points(errors);
    let mut errors = rules::larastan_finish(project.larastan.as_ref(), errors);
    settings.keep_reported(&mut errors);
    std::thread::spawn(move || drop(settings));
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
    Ok(Analysis {
        errors,
        general: processed.string_errors,
        fixable_ignored,
    })
}

fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

pub(crate) fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn php_files(dir: &Path, extensions: &[String]) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries
        .flatten()
        .map(|e| {
            let p = e.path();
            let dir = match e.file_type() {
                Ok(t) if !t.is_symlink() => t.is_dir(),
                _ => p.is_dir(),
            };
            (p, dir)
        })
        .collect();
    entries.sort();
    entries
        .into_par_iter()
        .flat_map_iter(|(p, dir)| {
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
        })
        .collect()
}
