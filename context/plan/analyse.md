# Plan: analyse (`pun analyse`)

Status: **walked through with Pushpak**; answers in "Decisions". This file is the plan for the analyser: how it works
today, the decided design, and the build list (§6). Decisions with dates: ../PLAN.md §2. Shared schema: config.md;
output: output.md.

## 1. What it is

`pun analyse` (alias `pun analyze`) answers one question: is this code wrong? It reports type errors, undefined symbols,
impossible conditions, dead code that needs types to spot, and misuse of Laravel. It never edits a file (commands.md).

It replaces:

| Upstream | What comes over |
| --- | --- |
| PHPStan core | every rule, natively. The levels become presets |
| Larastan | built in, nothing to install: models, relations, facades, container, config |
| phpstan-strict-rules, phpstan-deprecation-rules | the type-aware rules; strict-rules' syntax-only bans are lint rules |

The goal: on a project migrated from phpstan.neon, `pun analyse` reports the same problems as `phpstan analyse` (same
file, line and rule; messages may be reworded), so CI can switch.

## 2. How it works today

- **pun-phpstan** holds PHPStan's plumbing: the NEON parser, includes, parameters and schema validation,
  `ignoreErrors`, `@phpstan-ignore` comments, PHPStan's type model with its type descriptions (so messages print
  types the way PHPStan does) and `accepts()`, and the built-in functions and classes in `data/symbols.json`.
- **pun-analyse** runs the rules:
  1. It parses with Mago's parser and resolves names.
  2. It collects every symbol of the project and vendor into Mago's codex.
  3. It infers types with Mago's analyser (§7), with Larastan's knowledge as built-in providers and hooks
     (`src/larastan/`, `src/laravel/`) that the `laravel` extension registers (extensions.md).
  4. It checks each file in parallel. The rules read Mago's types converted to pun-phpstan's
     (`rules/types/convert.rs`).
  5. It drops what ignores and the baseline cover.
- **`pun analyse`** (crates/pun-cli/src/analyse.rs) reads `[analyse]` from pun.toml, selects rules by preset, prints
  Pun's output, PHPStan's machine formats and agent output, and writes Pun's baseline (`--generate-baseline`).
  `pun check` runs it after lint and format.
- **Rules:** 364 of the 370 analyse rules are ported (PLAN.md §0), across levels 0 to max. Known differences from
  PHPStan are in the test gap lists (`tests/fixtures/**/gaps*.txt`, `GAPS` constants). On the corpus at level 0, last
  measured on Mago 1.57.0, the differences were pinkary 0, larastan 6, pest 5 and laravel-framework 0 (PLAN.md §3,
  Track C).

## 3. The config

The schema is config.md §3 and §5. pun-config reads it into `Config.analyse` (preset, paths, exclude, baseline, the
type switches and the enabled rules), and `pun analyse` runs from it.

```toml
[analyse]
preset = "recommended"          # basic | essential | recommended | strict | max | phpstan/level_0..10 | phpstan/level_max
exclude = ["tests/**"]          # added to [files] exclude; not analysed and not scanned for symbols
baseline = "pun-baseline.toml"

[analyse.rules]
"types/missing-iterable-value-type" = "off"      # on or off only; no warn
"dead-code/if-always-constant" = { treat-php-doc-types-as-certain = false }   # PHPStan's treatPhpDocTypesAsCertain
```

- **PHPStan level presets** (2026-11-24): `phpstan/level_<n>` and `phpstan/level_max` run what PHPStan's level runs
  with default parameters (`pun_config::phpstan_level_rules`, from mapping.toml's `[upstream.phpstan-classes]` levels
  and `gated_by` defaults): no opt-in, strict-rules or deprecation-rules rules; bleeding-edge ones only with
  `bleeding-edge = true`; Larastan's defaults with Laravel on; the level's switches; and the lint rules that are PHPStan
  checks in `[lint]`. `pun migrate` writes the level's preset plus only what phpstan.neon changes; with
  `--offer-preset` on a terminal it offers Pun's closest preset, with a count of the new problems it would report.
- **No numeric levels:** `[analyse] level` and a number as preset are errors that name the PHPStan preset.
- **Laravel** analysis (Larastan's knowledge and rules) runs when `[extensions] laravel = "on"`. `pun migrate` and
  `pun init` turn it on when composer.lock locks laravel/framework. Larastan's rules stay off for a Laravel version
  Larastan 3 does not support.
- PHPStan's level 7 to 10 behaviour is four switches: `union-types`, `nullables`, `explicit-mixed`, `implicit-mixed`
  (config.md §5). The presets turn them on.
- `pun analyse` builds PHPStan's parameters from pun.toml (`Analyse::parameters` in pun-config): the level-tied ones
  (`checkClassCaseSensitivity`, `checkThisOnly`, `checkMaybeUndefinedVariables`, ...) follow the highest PHPStan level
  of the enabled rules and the switches; the others are rule options (`[phpstan-parameters]` in mapping.toml); each
  `strictRules.*` switch follows its rule, and with any strict-rules rule on, strict-rules' own parameters apply.
  `featureToggles.checkImportedClassNameCase` and `featureToggles.internalTag` have no pun.toml key and stay off.

## 4. Rules: what exists

There are 370 analyse rules in rules/mapping.toml, one per in-scope PHPStan rule (`[upstream.phpstan-classes]`):
322 from PHPStan core, 26 strict-rules, 7 deprecation-rules and 15 Larastan. A rule carries every identifier its
upstream rule emits. Each identifier stays in the mapping as a `variant` of one rule (`[upstream.phpstan]` and the
strict-rules, deprecation-rules and Larastan tables), so migrated ignores and baselines keep their narrow scope. Of the
433 upstream rules, 35 are lint or format rules (rules.md O4 and O5) and 28 are unsupported (PHPStan's own API checks,
playground-only and stub-only rules, one Larastan rule that does not exist).

| | Count |
| --- | ---: |
| Analyse rules | 370 |
| By group: types / classes / dead-code / phpdoc / undefined / functions / laravel / operators / attributes / deprecated / other | 118 / 64 / 42 / 32 / 18 / 16 / 15 / 15 / 14 / 8 / 28 |
| By preset (from the rule's PHPStan level): basic / essential / recommended / strict / max | 151 / 106 / 75 / 35 / 3 |
| PHPStan core identifiers (`[upstream.phpstan]`, without patterns) by level: 0 / 1 / 2 / 3 / 4 / 5 / 6 / opt-in | 312 / 13 / 124 / 37 / 78 / 7 / 6 / 57 |
| Ported | 364 |

The groups are the topic groups (config.md Build 1), so `undefined/method` and `types/argument` read naturally.

## 5. PHPStan's CLI, flag for flag

| PHPStan | Pun |
| --- | --- |
| `phpstan analyse [paths]` | `pun analyse [paths]` |
| `-c phpstan.neon` | `--config pun.toml`; phpstan.neon is read only by `pun migrate` |
| `-l 6` | `[analyse] preset = "strict"` in pun.toml (no levels) |
| `--generate-baseline` | `pun analyse --generate-baseline` (writes pun-baseline.toml) |
| `--error-format=json/github/gitlab/checkstyle/junit/raw/teamcity` | `--format` (or `--error-format`), the same formats; no `table` or `prettyJson` (PLAN.md §2, 2026-11-23) |
| `--memory-limit` | not needed: Pun has no PHP memory limit (removed 2026-11-23) |
| `--xdebug`, `--debug` | not needed |
| `clear-result-cache` | `--no-cache` (the result cache, PLAN.md §1) |
| `--pro`, `--watch` | no `--watch` on `pun analyse` yet (`pun format --watch` has one) |

## 6. Build

Steps 0, 1, 2, 8 and 9 are done. Steps 3 to 6 are in progress (PLAN.md §3 Track C has the open rows). The steps come
after config.md Build 1-3.

0. **Engine spike.** Done. Mago's analyser runs inside pun-analyse, and the Larastan provider works through Mago's
   hooks without a fork.
1. **Wire it up.** Done: the `pun analyse` command, `[analyse]` from pun.toml, the five presets, the output
   (output.md), PHPStan's machine formats and exit codes, agent output.
2. **Pun's own baseline.** Done: `pun analyse --generate-baseline` writes `pun-baseline.toml` (mago-reporting's
   baseline, loose by default); a run reads it and reports fixed entries.
3. **Finish level 0.** *Done when* the level-0 gap list (`tests/fixtures/rules/gaps.txt`) is empty and the corpus
   diffs are 0 (last measured on Mago 1.57.0: pinkary 0, larastan 6, pest 5, laravel-framework 0). Traits are
   analysed in the context of each class that uses them, only for traits in analysed files. Facade aliases belong to
   step 6.
4. **Type inference on Mago's analyser** (§7): expression types, narrowing, PHPDoc generics, mapped to PHPStan's type
   descriptions. Everything from `essential` up depends on it.
5. **Levels 1 to 10 as rules,** with the four switches, grouped into the presets basic, essential, recommended, strict,
   max. Then strict-rules and deprecation rules as plain rules.
   *Done when* each `phpstan/level_<n>` preset reports the same errors as `phpstan analyse --level <n>` on the corpus
   and on PHPStan's own rule tests, compared unfiltered (file, line, identifier). tests/levels.rs today compares only
   the identifiers Pun reports at a level, and has no level-0 or level-10 fixtures.
6. **Larastan built in** (§8): the Laravel knowledge and Larastan's 15 rules are in, with their differences in
   `tests/fixtures/larastan*/gaps.txt`. Left: the seven improvements.
7. **Pest built in** (§9), with the PHPUnit typing it needs. **After 1.0**: until then test files are analysed as
   plain PHP; `[analyse] exclude` globs leave them out entirely.
8. **A result cache.** Done (PLAN.md §1): a run analyses again only the files whose contents, or what they read,
   changed.
9. **`pun migrate` for phpstan.neon** (migrate.md Build 5). Done.

## 7. What Pun builds on

PHPStan walks each file's syntax tree with a scope that holds every variable's type, narrows after conditions and
assertions, and runs small rules that return errors with an identifier. Levels choose which rules run and how
strictly types compare. Larastan teaches it Laravel through extensions.

Mago's analyser walks the same kind of tree with a block context, narrows with a reconciler (as Psalm does), reads
generics and templates from PHPDoc, and reports issues by code. Its plugins are providers (return, property and
assertion types) and hooks (calls, expressions, statements, declarations), compiled into the binary.
mago-reporting has a baseline: strict (exact lines) or loose (counts per file, code and message).

**Pun's choice: build analyse on Mago's analyser, not a port of PHPStan's inference.**
- Messages may be reworded and migrate does not carry baselines over, so a PHPStan port is not needed. What must hold
  is one Pun rule for every PHPStan rule, and rules/mapping.toml has that.
- Mago's inference, narrowing and generics exist. A port of PHPStan's would be the largest single piece of work in Pun.
- **Rules:** every PHPStan rule is a Pun check that reads Mago's inferred types. Mago's issue codes fire far more
  widely than PHPStan's checks (hundreds of false errors on Pinkary at level 0), so a Mago issue is used directly
  only where it matches PHPStan exactly.
- **Laravel** (Larastan's knowledge) is the built-in `laravel` extension: Mago providers and hooks plus
  `pun_analyse::AnalyseExtension`, shaped like PHPStan's extension points (extensions.md). Providers give the types of
  models, relations, facades, the container and config; hooks add the Laravel checks.
- **Parity test:** PHPStan's own rule fixtures and the corpus, compared on file, line and Pun rule, not message text.
  Differences go in the gap lists.
- **Baseline:** `pun analyse --generate-baseline` writes pun-baseline.toml on mago-reporting's baseline, loose by
  default. `pun migrate` does not copy phpstan-baseline.neon; it tells the user to generate a new one.
  `ignoreErrors` in phpstan.neon is config, not a baseline, so migrate converts it to Pun rule IDs.
- **Kept from pun-phpstan:** the NEON reader, includes and `ignoreErrors` (for migrate), `@phpstan-ignore` parsing
  (migrate rewrites those comments), and PHPStan's type model for messages and `accepts()` checks.
- **Risk:** Mago infers differently from PHPStan in edge cases. The parity test measures this, and each gap is fixed
  in a Pun check or a Mago plugin.

## 8. Larastan, built in

Without Laravel knowledge every Laravel app drowns in false "undefined method" and "undefined property" errors on its
models. Pun builds Larastan's knowledge and rules into the `laravel` extension (§3).

**1. Laravel knowledge: types PHPStan can't see.**

| Area | What Pun must know | Where it comes from |
| --- | --- | --- |
| Model attributes | every column as a typed property; `$casts` / `casts()` (enums, dates, custom casts); accessors and mutators (`get…Attribute` and `Attribute::make`); `$appends` | migrations (`Schema::create` / `table`), or the schema dump in `database/schema/*.sql`, plus the model class |
| Relations | relation properties (`$user->posts` is `Collection<Post>`), generic relation types (`HasMany<Post, User>`), `withCount` / `withSum` attributes | the relation methods' return types and bodies |
| Query builder | `Model::where(...)` forwarded to `Builder<Model>`, dynamic `whereEmail()`, local scopes (`scope…` and `#[Scope]`), custom builders (`newEloquentBuilder`, `#[UseEloquentBuilder]`) | the model and its builder class |
| Collections | `Collection<TKey, TValue>` generics through `map`, `filter`, `pluck` and higher-order proxies (`$users->each->notify()`); custom collections (`#[CollectedBy]`) | Laravel's own PHPDoc generics, plus the proxies |
| Facades | `Cache::get` resolved to the class behind the facade, including real-time facades | each facade's accessor, resolved through the container |
| Container | `app(Foo::class)`, `app('cache')`, `resolve()`, `App::make()` return the bound type | class names, and the framework's own bindings list |
| Helpers | `auth()->user()` is the configured user model; `config('app.name')` returns the type of the value in config/; `request()`, `view()`, `response()`, `now()` (Carbon or the configured date class) | config/auth.php, config/*.php, the framework |
| Macros | `Str::macro`, `Collection::macro` and `Builder::macro` calls become real methods | the `macro(...)` calls in service providers |
| Factories, requests, jobs | `User::factory()` is `UserFactory`; `FormRequest::validated()` and `safe()`; `dispatch()` arguments match the job's constructor | factories, form requests, jobs |

None of this needs PHP to run: Pun reads the files.

**2. Larastan's rules** are mapped to Pun analyse rules in rules/mapping.toml (`undefined-relation`,
`no-env-outside-config`, `unused-view`, `octane-compatibility` and the rest).

**3. Improvements over Larastan** (all seven ship in 1.0):
- **Typed config:** `config('services.stripe.key')` has the real type of that value, and a missing key is an error.
- **Routes:** `route('posts.show')`, `to_route()` and `redirect()->route()` with a name that doesn't exist is an
  error, and so are missing route parameters.
- **Views:** `view('emails.welcome')` with no such Blade file is an error.
- **Translations:** `__('auth.failed')` and `trans()` keys checked against lang/, for every locale.
- **Env:** `env('STRIPE_KEY')` keys checked against `.env.example`.
- **Inertia:** `Inertia::render('Posts/Show')` with no such page component is an error.
- **No stale model data:** Pun re-reads migrations on every run, so it needs no ide-helper or fresh schema dump.

**Testing:** Larastan's own type and rule fixtures are the parity test, as PHPStan's fixtures are for core. The corpus
adds pinkary and larastan's own source.

## 9. Built-in support, not an extension system

Support for a framework is a Rust extension compiled into the binary (extensions.md), turned on in pun.toml's
`[extensions]`. There is no third-party extension system yet.

**In the core for 1.0** (the brief: "super Laravel support and Pest support", no bloat):
- **Laravel**, all of §8.
- **Pest** (after 1.0), as `pestphp/pest-plugin-phpstan` does: `$this` inside test closures is the test case,
  `expect()` chains are typed, and `uses()` / `pest()->extend()` classes are known. The part of PHPUnit typing Pest
  needs comes with it (`createMock()` returns `MockObject&Foo`, assertions narrow types), which also covers plain
  PHPUnit tests.
- **Deprecation rules** as plain rules; Mago's codex already records `@deprecated`.
- **strict-rules** as plain rules (the syntax-only ones are lint rules). They add no type knowledge.

**Later, as extensions:** Mockery, Carbon macros, Livewire, Filament and the like. `pun migrate` names each one as not
supported yet. The same goes for a project's own PHP rules and type extensions. `stubFiles` keep working.

## Decisions

Pushpak's answers:
- Every PHPStan core rule runs natively; Larastan is built in.
- **Engine:** Mago's analyser (§7, delegated to the assistant). The requirement is one Pun rule for every PHPStan rule.
- **Messages:** Pun may reword them.
- **Baseline:** its own feature (`pun analyse --generate-baseline`). Migrate never copies a PHPStan baseline.
- **PHPStan level presets** beside Pun's own (2026-11-24): `phpstan/level_0`..`phpstan/level_10`, `phpstan/level_max`,
  exact PHPStan levels; migrate writes them. Pun's presets stay its own picks.
- **No numeric levels. Five presets**, each including the one below, by the PHPStan levels each covers: `basic` (0-1),
  `essential` (2-3: PHPDoc and return types), `recommended` (4-5: dead code and argument types; the default),
  `strict` (6-8: missing types, union types and nulls), `max` (9-10: mixed). Migrate writes exact rules and offers the
  closest preset. The rules' `preset` field in mapping.toml uses these names.
- Laravel, Pest and PHPUnit are extensions that pun.toml's `[extensions]` turns on; `pun migrate` and `pun init` turn
  on each one composer.lock locks (PLAN.md §2, Extensions).
- **Analyse has no `warn`.** A problem analyse reports is an error; it never edits, so there is nothing softer to say.
  A rule is on or off (or a table of options). `off` stays because PHPStan users already turn rules off and migrate
  needs it to keep a project's rule set.
- Analyse never edits files; `pun check` runs it after lint and format.
- **A result cache** for analyse, as format, lint and check have (PLAN.md §1).
- **Extensions:** no third-party extension system now; built-in extensions only (extensions.md). Built in: Laravel
  (§8), Pest with the PHPUnit part it needs (after 1.0), deprecation rules and strict-rules (§9). A project's own
  PHPStan PHP code is named as unsupported by migrate.
- **All seven Larastan improvements (§8.3) ship in 1.0.**
- **Larastan's model knowledge** comes from migrations and the schema dump, read without running PHP.

Assistant's calls where the plan was silent:
- **Model attributes** come from one Mago expression hook that retypes `$model->column`, because Mago has no type
  provider for magic properties. A declared property or an `@property` tag wins, as in PHPStan.
- **Mago panics:** where Mago's analyser panics on a debug assertion, that file is skipped.
- **Inputs PHPStan cannot parse or stops on** (modifier errors Mago accepts, an internal error on a file) still get
  Pun's checks; those fixture cases are in the gap lists.
- **Names in messages are printed as PHPStan prints them:** built-in type names lowercased, class and constant names
  fully qualified, a constant value by its resolved name.
- **Anonymous classes** display as `<parent>@anonymous/<path>:<line>` (parent as written, else the first interface,
  else `class`; `:<index>` when several start on one line). The path is relative to the analysed paths' common
  directory below the working directory. The internal name is `AnonymousClass` plus an md5 of the path and line. A
  trait walked in a class's context is labelled `T.php (in context of class X)`.
- **`final` and `abstract`** are read from the source keyword, because Mago's flags also hold `@final` and body-less
  methods.
- **Below level 2**, rules that PHPStan restricts to `$this` check `$this` only; magic `__get`/`__set`/`__callStatic`
  classes are not reported at level 0.
- **printf/scanf checks** follow PHP 8.5's format parsing whatever the project's target PHP version, as PHPStan uses
  its runtime's.
