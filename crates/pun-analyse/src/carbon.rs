//! nesbot/carbon's PHPStan extension (`extension.neon`): its bootstrap file declares `Carbon\DatePeriodBase`, and
//! `MacroExtension` gives every class that implements `CarbonInterface` the methods `Carbon::macro()` registered.

use std::path::{Path, PathBuf};

use mago_analyzer::plugin::PluginRegistry;
use mago_analyzer::settings::Settings;
use mago_codex::metadata::CodebaseMetadata;
use mago_codex::metadata::function_like::{FunctionLikeKind, FunctionLikeMetadata, MethodMetadata};
use mago_codex::visibility::Visibility;
use mago_names::ResolvedNames;
use mago_span::Span;
use mago_syntax::cst::Program;
use mago_word::{Word, ascii_lowercase_word, empty_word};
use pun_phpstan::types::Type;
use pun_phpstan::{Array, Value};

use crate::Project;
use crate::larastan::macros::{Definition, registrations};

/// The bootstrap file Carbon's `extension.neon` names, from the project root.
const BOOTSTRAP: &str = "vendor/nesbot/carbon/lazy/Carbon/UnprotectedDatePeriod.php";
const INTERFACE: &[u8] = b"carbon\\carboninterface";
/// Classes whose `macro()` registers a Carbon macro without implementing `CarbonInterface` themselves: Laravel's
/// `Date` facade forwards it to its date class.
const FORWARDERS: [&[u8]; 1] = [b"illuminate\\support\\facades\\date"];

#[derive(Clone, Debug)]
pub struct Carbon;

impl Carbon {
    /// On when `bootstrapFiles` holds Carbon's bootstrap file and the file is installed: `extension.neon` adds both.
    pub fn from_params(root: &Path, params: &Array) -> Option<Carbon> {
        let bootstrap = root.join(BOOTSTRAP);
        let files = params.get_str("bootstrapFiles").and_then(Value::as_array)?;
        (crate::io::is_file(&bootstrap)
            && (files.values().filter_map(Value::as_str))
                .any(|f| Path::new(f) == bootstrap || root.join(f) == bootstrap))
        .then_some(Carbon)
    }
}

/// The lowercased class names `class` extends or implements, itself included, read before the codebase is
/// populated.
pub(crate) fn ancestors(codebase: &CodebaseMetadata, class: Word, out: &mut Vec<Word>) {
    if out.contains(&class) || out.len() > 256 {
        return;
    }
    out.push(class);
    let Some(k) = codebase.class_likes.get(&class) else {
        return;
    };
    let parents = (k.direct_parent_class.iter())
        .chain(&k.direct_parent_interfaces)
        .chain(&k.used_traits)
        .map(|p| ascii_lowercase_word(p.as_bytes()))
        .collect::<Vec<_>>();
    for p in parents {
        ancestors(codebase, p, out);
    }
}

fn is_carbon(codebase: &CodebaseMetadata, class: Word) -> bool {
    let mut all = Vec::new();
    ancestors(codebase, class, &mut all);
    all.iter().any(|a| a.as_bytes() == INTERFACE)
}

/// The function-like a macro definition names, as `MacroExtension::getMethod()` reflects it: its signature, and
/// whether the method is static (an unbound closure, or a static method).
fn signature(
    codebase: &CodebaseMetadata,
    definition: &Definition,
) -> Option<(FunctionLikeMetadata, bool)> {
    let get = |class: Word, name: &str| {
        let key = (
            class,
            ascii_lowercase_word(name.trim_start_matches('\\').as_bytes()),
        );
        codebase.function_likes.get(&key).map(|f| (**f).clone())
    };
    match definition {
        Definition::Closure(_, name, is_static) => {
            Some((codebase.get_closure(name)?.clone(), *is_static))
        }
        Definition::Function(name) => Some((get(empty_word(), name)?, false)),
        Definition::Method(class, method) => {
            let class = ascii_lowercase_word(class.trim_start_matches('\\').as_bytes());
            let f = get(class, method)?;
            let is_static = f.method_metadata.as_ref().is_some_and(|m| m.is_static);
            Some((f, is_static))
        }
        Definition::Mixin(_) | Definition::Returned(..) => None,
    }
}

impl crate::AnalyseExtension for Carbon {
    fn stub_files(&self, _: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    fn bundled_stub(&self, _: &Path) -> Option<&'static [u8]> {
        None
    }

    fn class_aliases(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Pun cannot run the bootstrap files, so it reads their `X::macro()` calls, and those of the analysed and
    /// scanned files, and declares each macro on every class that implements `CarbonInterface` and has no method of
    /// that name. PHPStan asks `hasMacro()` on the class, which an interface can't answer, so `CarbonInterface`
    /// itself gets none.
    fn prepare_codebase(&self, project: &Project, codebase: &mut CodebaseMetadata) {
        let registers = |path: &&PathBuf| {
            crate::io::read(path).is_ok_and(|b| {
                memchr::memmem::find_iter(&b, b"::").any(|at| {
                    b[at..]
                        .get(..7)
                        .is_some_and(|w| w.eq_ignore_ascii_case(b"::macro"))
                })
            })
        };
        use rayon::prelude::*;
        let candidates: Vec<&PathBuf> = project.analysed.iter().chain(&project.scanned).collect();
        let files = (candidates.par_iter())
            .filter(|p| registers(p))
            .map(|p| (*p).clone())
            .collect();
        let mut macros = Vec::new();
        for (class, name, definition) in registrations(files) {
            let class = ascii_lowercase_word(class.trim_start_matches('\\').as_bytes());
            if (FORWARDERS.contains(&class.as_bytes()) || is_carbon(codebase, class))
                && let Some(found) = signature(codebase, &definition)
            {
                // A later registration of the same name replaces the earlier one, as at runtime.
                macros.retain(|(n, _): &(String, _)| !n.eq_ignore_ascii_case(&name));
                macros.push((name, found));
            }
        }
        if macros.is_empty() {
            return;
        }
        // Each class gets the macros it has no method for, in the class itself: MacroMethodReflection's declaring
        // class is the class the method is called on.
        let wanted: Vec<(Word, Vec<usize>)> = (codebase.class_likes.iter())
            .filter(|(_, k)| !k.kind.is_interface() && !k.kind.is_trait())
            .filter_map(|(key, _)| {
                let mut all = Vec::new();
                ancestors(codebase, *key, &mut all);
                if !all.iter().any(|a| a.as_bytes() == INTERFACE) {
                    return None;
                }
                let missing = (0..macros.len()).filter(|&i| {
                    let lower = ascii_lowercase_word(macros[i].0.as_bytes());
                    !all.iter().any(|a| {
                        codebase.class_likes.get(a).is_some_and(|k| {
                            k.methods.contains(&lower)
                                || k.pseudo_methods.contains(&lower)
                                || k.static_pseudo_methods.contains(&lower)
                        })
                    })
                });
                Some((*key, missing.collect()))
            })
            .collect();
        for (class, missing) in wanted {
            // A method is the class's own when its span sits in the class: an empty one at the class's start
            let Some(at) = codebase.class_likes.get(&class).map(|k| k.span) else {
                continue;
            };
            for (name, (f, is_static)) in missing.into_iter().map(|i| &macros[i]) {
                let lower = ascii_lowercase_word(name.as_bytes());
                let mut method = f.clone();
                method.span = Span::new(at.file_id, at.start, at.start);
                method.kind = FunctionLikeKind::Method;
                method.name = lower;
                method.original_name = mago_word::word(name);
                let mut m = MethodMetadata::default();
                m.is_static = *is_static;
                m.visibility = Visibility::Public;
                method.method_metadata = Some(m);
                codebase
                    .function_likes
                    .insert((class, lower), Box::new(method));
                if let Some(k) = codebase.class_likes.get_mut(&class) {
                    k.methods.insert(lower);
                }
            }
        }
    }

    fn load(&mut self, _: &Project, _: &CodebaseMetadata) {}

    fn register(&self, _: &mut PluginRegistry, _: &[u8], _: &ResolvedNames<'_>) {}

    fn settings(&self, _: &mut Settings) {}

    fn typed_expressions<'arena>(
        &self,
        _: &ResolvedNames<'arena>,
        _: &Program<'arena>,
    ) -> Vec<Span> {
        Vec::new()
    }

    fn type_node(&self, _: &CodebaseMetadata, _: &str, _: &[Type]) -> Option<Type> {
        None
    }
}
