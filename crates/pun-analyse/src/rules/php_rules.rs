//! `[analyse] php-rules` (prototype): runs the worker on one checked file, answering its `getType()` questions
//! with the types the checker inferred.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mago_span::{Position, Span};
use pun_phpstan::ignore::Error;
use pun_phpstan::types::to_phpdoc_fully_qualified;

use super::Checker;

pub(super) fn check(c: &Checker, path: &Path, rules: &crate::PhpRules) -> Vec<Error> {
    // a trait's body is checked with the class that uses it, as PHPStan does
    let analysed = (c.trait_context.as_ref()).map_or(path, |t| Path::new(&t.class_file));
    if !rules.covers(analysed) {
        return Vec::new();
    }
    let context = (c.trait_context.as_ref()).map(|t| (t.class.as_str(), t.trait_name.as_str()));
    let type_at = |start, end| {
        // only spans Pun recorded a type for answer (driver.rs records every expression's); the worker stops the run
        // on a span the two parsers cut differently
        let span = Span::new(c.file.id, Position::new(start), Position::new(end));
        c.phpstan_type_at(span)
            .map(|t| to_phpdoc_fully_qualified(&t))
    };
    let found = rules.check(path, context, type_at, || dependencies(c));
    (found.into_iter())
        .map(|f| {
            let mut e = Error::new(f.message, c.label.clone(), Some(f.line));
            e.identifier = f.identifier;
            e.tip = f.tip;
            e.can_be_ignored = !f.non_ignorable;
            if let Some(t) = &c.trait_context {
                e.file_path.clone_from(&t.class_file);
                e.trait_file_path = Some(c.file_name());
            }
            e
        })
        .collect()
}

/// The files the check depends on, for the result cache, as PHPStan's DependencyResolver finds them: the files that
/// declare the classes and functions the file names or its expressions' types hold (driver.rs records every
/// expression's), and those of their parent classes, interfaces and traits, recursively.
fn dependencies(c: &Checker) -> Vec<PathBuf> {
    let mut pending: Vec<&[u8]> = c.names.iter().map(|(_, _, name, _)| name).collect();
    let objects: Vec<_> = (c.types.values())
        .flat_map(|t| t.types.iter())
        .flat_map(mago_codex::ttype::atomic::TAtomic::get_all_object_names)
        .collect();
    pending.extend(objects.iter().map(|w| w.as_bytes()));
    if let Some(t) = &c.trait_context {
        pending.extend([t.class.as_bytes(), t.trait_name.as_bytes()]);
    }
    let mut seen = HashSet::new();
    let mut files = HashSet::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        if let Some(k) = c.codebase.get_class_like(name) {
            files.insert(k.span.file_id);
            pending.extend(
                (k.all_parent_classes.iter())
                    .chain(&k.all_parent_interfaces)
                    .chain(&k.used_traits)
                    .map(|w| w.as_bytes()),
            );
        } else if let Some(f) = c.codebase.get_function(name) {
            files.insert(f.span.file_id);
        }
    }
    let mut deps: Vec<PathBuf> = files
        .iter()
        .filter_map(|id| c.files.get(id).cloned())
        .collect();
    deps.sort();
    deps
}

/// Every expression's span, for the worker's `getType()`.
pub(super) fn expressions(program: &mago_syntax::cst::Program) -> Vec<Span> {
    use mago_span::HasSpan;
    use mago_syntax::cst::Expression;
    use mago_syntax::walker::MutWalker;
    struct All(Vec<Span>);
    impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for All {
        fn walk_in_expression(&mut self, e: &'ast Expression<'arena>, (): &mut ()) {
            self.0.push(e.span());
        }
    }
    let mut all = All(Vec::new());
    all.walk_program(program, &mut ());
    all.0
}
