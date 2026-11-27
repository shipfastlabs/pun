//! Larastan 3.10.0's type tests, run through PHPStan 2.3.1 with Larastan by scripts/export-larastan-types.php:
//! tests/Type's assertion files (TypeInferenceTestCase) and tests/Integration's analysed files (IntegrationTest), each
//! test class's files at level 8 with its own config (FileAssertRule runs at every level). Pun analyses the same files in
//! tests/fixtures/larastan-types/root, a copy of the Larastan checkout with the vendor files Pun reads, and must report
//! PHPStan's errors:
//!
//! - every assertion line's `phpstan.*` errors (PHPStan's own assertions hold, so it has none);
//! - every Type file's other errors and every Integration file's errors, with an identifier Pun reports.
//!
//! Cases that still differ are listed in tests/fixtures/larastan-types/gaps.txt, one per line (`<file>:<line>` for an
//! assertion line, `<file>` for a file's other errors), and the list must be exact: a fixed case has to leave it.
//! `PUN_LARASTAN_TYPES_REPORT=1` prints every failing case's difference.
//!
//! Type/PunEloquentBuildersTest.json is Pun's own reduced case (root/tests/Type/data/pun-eloquent-builders.php), its
//! assertions PHPStan's types for it with GeneralTypeTest's config. Type/PunGenericEloquentUnionsTest.json (IDE-helper
//! `Builder|Model` and `Collection|Model[]` unions, `builder-of` of a template) and Type/PunMacroClosureThisTest.json
//! (`$this` in a `macro()` closure) are reduced cases too, their assertions and errors from PHPStan 2.3.6 with
//! Larastan 3.15.0 at level 8 in a Laravel 13 app. So are Type/PunBuilderMacrosTest.json (Query\Builder macros called
//! through an Eloquent builder), Type/PunModelSelfQueryTest.json (`self::query()` and `$model::query()` in a base
//! model and a trait), Type/PunClosureVarThisTest.json (`/** @var X $this */` in a plain closure) and
//! Type/PunMixinMacrosTest.json (`mixin()` and macros named in a loop over a literal array), the oracle app's
//! provider calling each fixture's `Macros::register()`, and Type/PunLocalMacroTest.json (a method-callable macro
//! whose class sits in a file a provider requires, in the testbench skeleton Larastan boots:
//! root/vendor/orchestra/testbench-core/laravel/app/Providers/PunLocalServiceProvider.php).
//! Type/PunDynamicStaticCallsTest.json (strict-rules' `staticMethod.dynamicCall` on the methods Larastan forwards,
//! `__callStatic` mixins and SoftDeletes' `@method static restore()`) runs with strict-rules' rules.neon and only
//! `dynamicCallOnStaticMethod` on, as its PHPStan run did. Type/PunStrictSignaturesTest.json (the `covariant` and
//! `contravariant` method signatures, static methods included, and `varTag.type` that strict-rules' parameters turn
//! on, and a final class's `static` parent return) runs with strict-rules' rules.neon and every check off.
//! Type/PunCheckedExceptionsTest.json (`missingType.checkedException` through a `throw_if()` template, an Eloquent
//! builder's forward to the query builder, the Storage and Cache facades and closures called through a variable) runs
//! with `exceptions.check.missingCheckedExceptionInThrows` on; its PHPStan run is the strict-signatures one's.
//! Type/PunDynamicStaticCallTypesTest.json (the receiver types `staticMethod.dynamicCall` sees: a closure parameter
//! narrowed by a nested callable, a relation-forwarded callback's `static`, a call after a nullsafe link, `when()`
//! returning the relation, a stored closure sent to a union, a macro over an `@method` tag) has the config and
//! PHPStan run of PunDynamicStaticCallsTest. Type/PunFacadeStaticMethodsTest.json (`staticMethod.notFound` on a
//! method that neither a facade's `@method` tags nor its root declare) has errors from PHPStan 2.3.7 with Larastan
//! 3.15.0 at level 8 in a Laravel 13 app. Type/PunGenericScopeClosuresTest.json (closures sent to `where()` and
//! `whereHas()` in scopes generic in their model, where `builder-of<TModel, 'rel'>` holds a template) has errors from
//! PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app. Type/PunObjectShapesTest.json (classes passed
//! to an `object{...}` shape and a template bound to one: `@property` tags only with `__get`, Larastan's model
//! columns and key, native, protected and static properties, and the property types) has its assertions and errors
//! from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app with the same `users` table.
//! Type/PunNullsafeRelationsTest.json (a relation property read through `?->`, then a method chain and `??`) has
//! its assertions from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app.
//! Type/PunNullableModelColumnsTest.json (a migration column read with `->` on a nullable model, as from
//! `Collection::first()`) has its assertions and errors from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a
//! Laravel 13 app with the same `users` table.
//! Type/PunTemplateRelationCollectionsTest.json (`get()` on a `HasMany` whose related model is a template, a
//! `collection-of` holding the template) has its assertion from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a
//! Type/PunPaginatorThroughTest.json (`through()` on the paginators, whose `@return $this` reads its
//! `@phpstan-this-out`) has its assertions and errors from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a
//! Laravel 13 app.
//! Type/PunAuthContractsTest.json (methods on the Authenticatable, CanResetPassword and Authorizable contracts found
//! on the auth model, as AuthsMethodsExtension finds them) has its assertions and errors from PHPStan 2.3.7 with
//! Larastan 3.15.0 at level 8 in a Laravel 13 app whose first auth model also extends `Illuminate\Foundation\Auth\User`.
//! Type/PunUntypedFactoriesTest.json (`Model::factory()` of a model whose untyped `newFactory()` and unbound
//! HasFactory name no concrete factory, which ModelFactoryDynamicStaticMethodReturnTypeExtension types as an
//! ErrorType) has its assertions and errors from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app.
//! Type/PunTableAttributeTest.json (Laravel 13's `#[Table]` naming a model's table, named or positional, inherited by a
//! subclass, and a declared `$table` winning over it) has its assertions and errors from PHPStan 2.3.7 with Larastan
//! 3.15.0 at level 8 in a Laravel 13 app with a `users` table.
//! Type/PunPackageViewsTest.json (`namespace::view` names a provider registers with `loadViewsFrom()`, here
//! root/vendor/orchestra/testbench-core/laravel/app/Providers/PunViewsServiceProvider.php) has its errors from PHPStan
//! 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app whose registered provider loads the same views.
//! Type/PunVendorMixinsTest.json (a mixin a discovered package's provider registers, its class found through the
//! package's PSR-4 autoload: root/vendor/pun/vendor-mixins, listed in root/vendor/composer/installed.json) has its
//! assertion from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app with the same package installed.
//! Type/PunBenevolentTranslationsTest.json (`__()` and `trans_choice()` joined with other values in a ternary, `match`
//! and `.=`, a union that stays benevolent when every member comes from a benevolent input) has its assertions and
//! errors from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app.
//! Type/PunConfigFacadeGetTest.json (`checkConfigTypes` types `config()` and a repository's `get()` from the config
//! file, while the `Config::get()` facade call stays `mixed`) has its assertions and errors from PHPStan 2.3.7 with
//! Larastan 3.15.0 at level 8 in a Laravel 13 app with the same `config/test.php`.
//! Type/PunCollectionFilterChecksTest.json (`filter()` and `reject()` with an `is_scalar()` or `is_array()` callback)
//! has its assertions from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13.44.1 app.
//! Type/PunPassthruVersionsTest.json (`insertOrIgnoreReturning()`, which BuilderHelper's versioned passthru forwards to
//! the query builder from Laravel 13.30.1) has its assertions from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a
//! Laravel 13.44.1 app.
//! Type/PunReturnedMacrosTest.json (Route macros whose closure a method returns, registered by a class the package
//! provider of root/vendor/pun/vendor-route-macros instantiates) has its assertions from PHPStan 2.3.7 with Larastan
//! 3.15.0 at level 8 in a Laravel 13.44.1 app with the same package installed.
//! Type/PunContainerBindingsTest.json (`resolve()` and `app()` of a string the framework's providers bind to a closure
//! returning one `new` instance, such as `db.transactions`) has its assertions from PHPStan 2.3.7 with Larastan 3.15.0
//! at level 8 in a Laravel 13.44.1 app.
//! Type/PunRelationNarrowingTest.json (a relation property without a `@property` tag narrowed by `instanceof` and
//! `!== null`) has its assertions and errors from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13 app.
//! Type/PunMixinModelPropertiesTest.json (a relation and its `_count` read through a resource's `@mixin` model) has its
//! assertions from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13.44.1 app.
//! Type/PunStorageFacadeReturnsTest.json (Storage facade calls typed by the filesystem adapter's method, which
//! StorageMethodsClassReflectionExtension finds before the facade's `@method` tag) has its assertions from PHPStan
//! 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel 13.44.1 app.
//! Type/PunStorageFacadeCallablesTest.json (`Storage::get(...)`, the facade's `@method` tag because the filesystem
//! manager's `get()` is protected) has its assertion from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel
//! 13.44.1 app.
//! Type/PunInheritedCastsTest.json (`$casts` redeclared below a base model that doesn't, typed by Larastan's Model stub
//! and not HasAttributes' `@var array`) has its errors from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel
//! 13.44.1 app.
//! Type/PunRelationWhereHasTest.json (an untyped closure parameter of `whereHas()` and its kin forwarded by a relation,
//! the stub's `$relation is string` conditional resolved from the argument) has its assertions from PHPStan 2.3.7 with
//! Larastan 3.15.0 at level 8 in a Laravel 13.44.1 app.
//! Type/PunRequireExtendsNarrowingTest.json (a model narrowed by `instanceof` on an interface with
//! `@phpstan-require-extends Model`) has its assertions from PHPStan 2.3.7 with Larastan 3.15.0 at level 8 in a Laravel
//! 13.44.1 app.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use pun_analyse::{Project, RelativePath, reports, run};
use pun_phpstan::config::{Options, resolve};
use pun_phpstan::{Key, Value as Parameter};
use serde_json::Value;

type Found = BTreeSet<(i64, String, String)>;

fn is_assertion(identifier: &str) -> bool {
    matches!(
        identifier,
        "phpstan.type"
            | "phpstan.nativeType"
            | "phpstan.superType"
            | "phpstan.variable"
            | "phpstan.unknownExpectation"
    )
}

fn found<'e>(errors: impl Iterator<Item = (i64, &'e str, &'e str)>) -> Found {
    errors
        .filter(|(_, id, _)| reports(id))
        .map(|(line, id, message)| (line, id.to_string(), message.to_string()))
        .collect()
}

fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() && p.file_name().unwrap() != "root" {
            json_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "json") {
            out.push(p);
        }
    }
}

fn analyse(root: &Path, test: &Value) -> BTreeMap<String, Found> {
    let configs = test["configs"].as_array().unwrap();
    assert_eq!(configs.len(), 1, "{}", test["test"]);
    let config = resolve(&Options {
        config: Some(
            root.join(configs[0].as_str().unwrap())
                .to_string_lossy()
                .into_owned(),
        ),
        level: Some("8".to_string()),
        ..Options::for_project(&root.to_string_lossy())
    })
    .unwrap();
    let mut params = config.parameters;
    params.set(Key::from("level"), Parameter::Int(8));
    let mut project = Project::from_config(root, &params);
    let mut runs: Vec<(Vec<PathBuf>, BTreeSet<&str>)> = Vec::new();
    for case in test["cases"].as_array().unwrap() {
        let file = root.join(case["file"].as_str().unwrap());
        let names: BTreeSet<&str> = case["declares"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n.as_str().unwrap())
            .collect();
        match runs
            .iter_mut()
            .find(|(_, declared)| declared.is_disjoint(&names))
        {
            Some((files, declared)) => {
                files.push(file);
                declared.extend(names);
            }
            None => runs.push((vec![file], names)),
        }
    }
    let prefix = format!("{}/", root.display());
    let mut by_file: BTreeMap<String, Found> = BTreeMap::new();
    for (files, _) in runs {
        // PHPStan ran on the case files, so an anonymous class shows relative to them
        let paths: Vec<String> = files
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect();
        project.relative_path = RelativePath::new(&root.to_string_lossy(), &paths);
        project.analysed = files;
        for e in run(&project).unwrap() {
            let file = e.file.strip_prefix(&prefix).unwrap_or(&e.file).to_string();
            by_file
                .entry(file)
                .or_default()
                .extend(found(std::iter::once((
                    e.line,
                    e.identifier.as_str(),
                    e.message.as_str(),
                ))));
        }
    }
    by_file
}

#[test]
fn larastan_type_tests() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/larastan-types");
    let root =
        std::env::var_os("PUN_LARASTAN_TYPES_ROOT").map_or_else(|| dir.join("root"), PathBuf::from);
    let gaps: BTreeSet<String> = std::fs::read_to_string(dir.join("gaps.txt"))
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect();
    let mut tests = Vec::new();
    json_files(&dir, &mut tests);
    tests.sort();
    let mut failing = BTreeMap::new();
    let mut counts = [(0, 0); 3];
    for path in tests {
        let test: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let got = analyse(&root, &test);
        for case in test["cases"].as_array().unwrap() {
            let file = case["file"].as_str().unwrap();
            let want = found(case["errors"].as_array().unwrap().iter().map(|e| {
                (
                    e["line"].as_i64().unwrap_or(0),
                    e["identifier"].as_str().unwrap_or(""),
                    e["message"].as_str().unwrap(),
                )
            }));
            let got = got.get(file).cloned().unwrap_or_default();
            let mut compare =
                |name: String, want: Found, got: Found, count: &mut (usize, usize), n: usize| {
                    count.1 += n;
                    if want == got {
                        count.0 += n;
                    } else {
                        let missing: Vec<_> = want.difference(&got).collect();
                        let extra: Vec<_> = got.difference(&want).collect();
                        failing.insert(name, format!("  missing {missing:?}\n  extra   {extra:?}"));
                    }
                };
            let Some(asserts) = case["asserts"].as_array() else {
                compare(file.to_string(), want, got, &mut counts[2], 1);
                continue;
            };
            let mut lines: BTreeMap<i64, usize> = BTreeMap::new();
            for line in asserts {
                *lines.entry(line.as_i64().unwrap()).or_default() += 1;
            }
            let at = |errors: &Found, line: i64| -> Found {
                errors
                    .iter()
                    .filter(|(l, id, _)| *l == line && is_assertion(id))
                    .cloned()
                    .collect()
            };
            for (&line, &n) in &lines {
                compare(
                    format!("{file}:{line}"),
                    at(&want, line),
                    at(&got, line),
                    &mut counts[0],
                    n,
                );
            }
            let other = |errors: &Found| -> Found {
                errors
                    .iter()
                    .filter(|(l, id, _)| !is_assertion(id) || !lines.contains_key(l))
                    .cloned()
                    .collect()
            };
            compare(
                file.to_string(),
                other(&want),
                other(&got),
                &mut counts[1],
                1,
            );
        }
    }
    let [asserts, others, integration] = counts;
    eprintln!(
        "assertions {}/{}, Type files' other errors {}/{}, Integration files {}/{}",
        asserts.0, asserts.1, others.0, others.1, integration.0, integration.1
    );
    if std::env::var_os("PUN_LARASTAN_TYPES_REPORT").is_some() {
        for (name, diff) in &failing {
            eprintln!("{name}\n{diff}");
        }
    }
    assert!(asserts.1 >= 1390, "only {} assertions", asserts.1);
    let failing_names: BTreeSet<String> = failing.keys().cloned().collect();
    let fixed: Vec<_> = gaps.difference(&failing_names).collect();
    let new: Vec<_> = failing
        .iter()
        .filter(|(name, _)| !gaps.contains(*name))
        .collect();
    assert!(
        fixed.is_empty() && new.is_empty(),
        "fixed (remove from gaps.txt): {fixed:#?}\nnew failures:\n{}",
        new.iter()
            .map(|(name, diff)| format!("{name}\n{diff}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
