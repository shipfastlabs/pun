# Plan: lint (`pun lint`)

Status: **walked through with Pushpak**; answers in "Decisions". This file is the plan for the linter: how it works
today, the decided design, and the build list (§7). Decisions with dates: ../PLAN.md §2. Shared schema: config.md;
output: output.md.

## 1. What it is

`pun lint` holds every rule that can change what code does. `pun lint` reports, and `pun lint --fix` applies.
It replaces three things:

| Upstream | What moves into lint |
| --- | --- |
| Rector (+ rector-laravel, rector-phpunit, pest-plugin-rector) | every rule except the Symfony, Doctrine and downgrade ones, the ones upstream deprecates and Rector's post-processors (../PLAN.md §2) |
| PHP-CS-Fixer / Pint | the risky fixers (risky rules are lint, format.md §4) and the code rules moved from format |
| Mago's linter | its rules, as normal lint rules under Pun names |

The goal is the same changes as `rector process` for the rules a project uses. With the format rules on top,
`pun check --fix` writes the same bytes as `rector process && pint`.

## 2. How it works today

A file goes through two engines in one run (crates/pun-cli/src/run.rs):

1. **The native rules** (crates/pun-lint) on Mago's syntax tree (§6): each rule returns problems with a span,
   message and help, and fixes as text edits on byte ranges. `Linter::fix` applies them with
   `pun_lint::problem::apply_fixes` in rounds until no rule has a fix left (at most 100 rounds). `@pun-ignore` comments
   (rules.md §4) and the per-file levels of `[[lint.overrides]]` (crates/pun-cli/src/lint_plan.rs) apply here.
2. **The token path:** the lint rules that are PHP-CS-Fixer fixers (the risky fixers, the rules moved from format and
   the `Pun/*` ports of Mago rules) run in the formatter's pipeline (format.md §2), in PHP-CS-Fixer's priority order,
   so `pun check --fix` still writes Pint's bytes. These report a file at its first changed line, not one problem per
   change (Build 3).

All but 1 lint rule run natively; the one left is rector-laravel 2.7.3's `laravel/prefer-pivot-class` (PLAN.md §0
has the counts). Type-aware rules still read no expression types from analyse (Build 8).

## 3. The config

The schema is config.md §3 and §5: `[lint] sets`, `[lint.groups]` and `[lint.rules]` take `"off"`, `"warn"` or
`"error"`, options in a table with `level`, `fix = "off"` for report-only, and `[[lint.overrides]]` per path.
Only `error` fails; `--deny-warnings` and `--max-warnings <n>` tighten that, and `-A` / `-W` / `-D` set a level for one
run. `--fix` applies every fix of every enabled rule except those with `fix = "off"`. A PHP-CS-Fixer or Rector name in
`[lint.rules]` is an error that names the Pun ID to use.

pun-config resolves sets, groups and rule lines into `Config.lint.rules` (config.md Build 3); pun-cli applies them per
file with the overrides and `-A`/`-W`/`-D` (lint_plan.rs).

## 4. Rules: what exists

All lint rules in crates/pun-config/rules/mapping.toml are for 1.0. Each has its group, fix safety, sets and upstream
source: one in-scope upstream rule (Rector, a risky or moved PHP-CS-Fixer fixer, or a PHPStan rule class), several
listed in `merged`, or Mago (`origin = "mago"`). rules.md §7 has the reasons for what is in and out.

The 821 lint rules (`python3 -I scripts/check-rule-mapping.py`; a merged rule counts once per source):

| | Count |
| --- | ---: |
| Upstream source: Rector core / rector-laravel / rector-phpunit / pest-plugin-rector | 360 / 136 / 109 / 60 |
| Upstream source: PHP-CS-Fixer fixer (76 risky, 39 moved from format) / Pint / PHPStan rule classes | 115 / 1 / 31 |
| Mago only / Pun only | 81 / 1 |
| Also carrying a Mago mapping | 62 |
| Fix safety: safe / unsafe / mixed / report only | 337 / 396 / 8 / 80 |
| Sets: dead-code / modernize / code-quality / types / early-return / privatization / naming / coding-style / none | 72 / 107 / 224 / 86 / 2 / 5 / 0 / 105 / 225 |
| Ecosystem groups: laravel / phpunit / pest (upgrade groups such as `laravel-11` and `php-7.4` not counted) | 108 / 134 / 59 |

The groups are the topic groups shared with format (format.md §8, config.md Build 1); intent stays as data.

Unsupported on purpose (../PLAN.md §2, 2026-11-24): the rector-symfony, rector-doctrine and rector-downgrade-php
rules, the Symfony, Doctrine, JMS and Behat rules of Rector core, deprecated rules, post-processors, rules whose input
does not compile on PHP 8.4, and rules upstream registers in no set by mistake. `pun migrate` names each one as
unsupported and skips it (migrate.md §3). The other Rector rules still marked `unsupported` are being ported.

## 5. Rector's CLI, flag for flag

| Rector | Pun |
| --- | --- |
| `rector process --dry-run` | `pun lint` |
| `rector process` | `pun lint --fix` (doesn't format afterwards, as Rector doesn't; `pun check --fix` does both) |
| `--config rector.php` | `--config pun.toml`; rector.php is read only by `pun migrate` |
| `--only=SomeRector` | `--only <rule>` (to build) |
| `--output-format console/json/github/gitlab/junit` | `--format`, same formats as `pun format` (format.md §9) |
| `--no-diffs`, `--no-progress-bar` | not needed: Pun has neither flag; `-q` prints nothing |
| `--clear-cache` | `--no-cache` (the result cache, format.md §2) |
| `rector list-rules` | `pun explain <rule>` (config.md Build 8) |

## 6. How lint edits code (decided: text edits)

Rector changes php-parser's syntax tree and prints the changed nodes back with a format-preserving printer. Matching
its bytes would mean porting that printer and its quirks. Pun does what Ruff, Biome, oxlint and Mago do:
- each rule finds a node in Mago's syntax tree and returns text edits on byte ranges;
- the engine applies the non-overlapping edits and loops until no rule has more;
- `pun format` (or `pun check --fix`) then fixes the layout of what was inserted.

The result is the same code change as Rector's, but whitespace inside an inserted node can differ until format runs.
Parity is `format(pun lint --fix)` equal to `format(rector process)` of Rector 2.9.2, on Rector's own fixtures and the
corpus, except where Pun deliberately differs (PLAN.md §2, 2026-11-28). A later Rector release does not move it.

`pun analyse` uses the same Mago syntax tree, so `pun check` can parse each file once for lint and analyse.
Type-aware rules (`add-return-type` and others) ask pun-analyse for types, as Rector asks PHPStan.

## 7. Build

Steps 1, 2, 4, 5, 6 and 9 are done, and `pun check --fix` equals Pint. Left: step 3, the rest of 7 and step 8
(PLAN.md §3 Track B). The steps come after config.md Build 1-6.

1. **The engine on Mago's syntax tree.** Done: rules return problems and text edits, fixes loop with overlapping
   edits deferred to the next round, one parse per file, files in parallel, and a result cache per unchanged file.
2. **Levels, sets and overrides at run time.** Done.
3. **Per-line problems for the token path,** so PHP-CS-Fixer's risky rules report a line and a frame, not just a file.
   *Done when* each change of a token-path rule is its own problem with a span, and `@pun-ignore` line forms work for it.
4. **Ignore comments.** Done.
5. **Output** (output.md Build 1-3). Done.
6. **Mago's rules as normal lint rules.** Done; `pun lint --mago` and the `mago-lint` feature are gone.
7. **Rules in usage order,** each passing Rector's own fixtures: Pinkary's rector.php, then dead-code and modernize,
   then code-quality, types, early-return, privatization, naming, coding-style, then rector-laravel, the PHP 8.4+ and
   Laravel 12+ upgrade groups, Pest, PHPUnit and the rules in no set.
   *Done when* every lint rule is native (the 1.0 bar). Left: `laravel/prefer-pivot-class`.
8. **Type-aware rules** query analyse's index (analyse.md Build 3) instead of PHPStan. Until then the rules that need
   types either wait or read them from the syntax (Decisions).
   *Done when* pun-lint rules can read expression types, class subtyping and resolved constant values.
9. **`pun migrate` for rector.php** (migrate.md Build 4). Done.

## 8. `pun init` and lint presets

**A new, blank Laravel project:** `pun init` asks three things and writes pun.toml.
1. **Format preset:** `laravel` or `psr12`.
2. **Analyse preset:** `basic`, `essential`, `recommended` (default), `strict` or `max`, each including the one below
   (analyse.md decisions).
3. **Lint preset:** `essential`, `recommended` (default) or `strict`, each including the one below.

Laravel, Pest and PHPUnit are not questions. `pun init` reads composer.lock and writes `[extensions]` for them, which
lets their rules in the preset run, including the rules for the locked versions.

**A project coming from Pint, Rector or PHPStan:** `pun migrate` writes the exact rules the old config runs: Rector's
sets as `rector/*` sets, then only the rules that differ. With `--offer-preset` on a terminal it then offers the
closest preset, with a count of the new problems it would report.

### Pun's ladder plus Rector's exact sets (decided 2026-11-24)

```toml
[lint]
preset = "recommended"                              # Pun's picks: essential < recommended < strict
sets = ["rector/dead-code", "rector/code-quality"]  # Rector's sets, exactly
```

- **Presets** (`preset` on each lint rule in mapping.toml, the lowest preset that holds it): `essential` is likely bugs
  and dead code with safe fixes; `recommended` adds modernize, code-quality and the ecosystem quality rules;
  `strict` adds types, privatization, early returns, coding style and every unsafe fix. Rules in no preset are opt-in.
- **Rector sets** (rules/rector-sets.json, written by scripts/gen-rector-sets.py from the upstream inventories):
  `rector/<set>`, `rector-laravel/<set>`, `rector-phpunit/<set>`, `pest/<set>`, and `rector/php-sets` for
  `withPhpSets()` (every `rector/phpNN` up to `[php] version`). A merged rule that only a set brings runs only that
  set's Rector rules (the same `checks` options `pun migrate` writes); Rector rules that are format rules in Pun stay
  in `[format]`.
- **A rule runs when the preset or a set holds it**, its gates allow it (PHP version, extension), and no group or rule
  line turns it off. No preset and no sets runs nothing but named rules and groups.
- The old sets (`dead-code`, `modernize`, ...) are config errors naming their `rector/*` twin.

## Decisions

Pushpak's answers:

- **Text edits** (§6). There is no php-parser printer port.
- **Topic groups**, the same group list as format and analyse (`lint/imports/no-unused`,
  `lint/laravel/no-request-all`); intent (correctness, suspicious, style) stays as data. Each upgrade target gets its
  own group (`lint/laravel-13/...`, `lint/php-8.5/...`), so one line turns a whole upgrade on.
- **Sets for new projects, explicit rules for migrated ones** (§8), with dead-code and modernize ticked by default in
  `pun init`.
- **`pun init` asks three things:** the format preset, the lint sets and the analyse preset.
- **Laravel, Pest and PHPUnit rules come from `[extensions]`** (../PLAN.md §2 "Extensions"), which `pun init` and
  `pun migrate` fill from composer.lock, version rules included; their group can be switched off in pun.toml. Nothing
  to install.
- **`--fix` applies every fix of every enabled rule**, as Rector does. There is no `--unsafe` flag. Safe/unsafe is a
  label in the output (`[fix:unsafe]`) so reviewers know which changes to read closely. A rule that should only
  report takes `fix = "off"`.
- **Only `error` fails the run; `warn` is printed and never fails.** Every enabled rule is `error` unless set to
  `warn`; `pun migrate` sets everything to `error`, because PHPStan and Rector fail on anything they report.
- **Severity syntax and behaviour follow oxlint:**
  - Each rule takes `"off"`, `"warn"` or `"error"`. Options go in a table with `level`:
    `"imports/ordered" = { level = "warn", sort = "alphabetical" }`.
  - A whole group takes a level too: `[lint.groups] style = "warn"`. A rule's own line wins over its group's.
  - `--deny-warnings` makes warnings fail the run, and `--max-warnings <n>` fails above n warnings.
  - `-A` / `-W` / `-D <rule or group>` (allow / warn / deny) set a level for one run.
  - `[[lint.overrides]]` with `files = ["tests/**"]` changes levels for some paths.
- **Every lint rule for 1.0 runs natively before 1.0.** That covers every
  non-deprecated rule of Rector, rector-laravel, rector-phpunit and the Pest plugin, upgrade sets below the floor
  included, except the Symfony, Doctrine and downgrade rules and Rector's post-processors (../PLAN.md §2). Porting order follows usage (Build 7), but 1.0 waits for all of them.
- **Mago's rules become normal lint rules**; `pun lint --mago` is gone.
- **`pun lint --fix` doesn't format**; `pun check --fix` does lint, then format (commands.md §3).
- **Unported rules are never delegated.** `pun migrate` names and skips them; a hand-written pun.toml that names one
  is a config error (exit 2).
- **Upgrades are lint rules in their own groups**, not a separate command.

Assistant's calls where the plan was silent:

- **Rule shape.** A rule is called for every node with its ancestor stack and returns reports (span, message, help,
  optional `pun_lint::problem::Fix`); it declares a lowest PHP version. A round whose fixes break the syntax leaves the
  file unchanged and is an error naming the rules.
- **Rounds.** Pun's fix loop runs until nothing is left, as Ruff's does (at most 100 rounds). Rector 2.8.4 also reruns
  its rules, but on the same syntax tree, so rules whose fix would match again use `Rule::once`. Pun still merges
  more in some cases (three equal `if`s become one where Rector merges two); those cases are left out of the Oracle
  fixtures.
- **The Rector oracle** is rector-src at the tag Pinkary's lock pins (fixtures, `.oracle/rector`) plus the released
  rector/rector locked by `scripts/oracle-rector/composer.lock` (runnable, `.oracle/rector-bin`).
- **Meaning over bytes.** Where Rector's output changes what the code does or is not valid PHP (dropped parentheses,
  reordered side effects, dropped `else` branches, `return ();`), Pun writes the valid fix that keeps the meaning.
  Where Rector crashes or stops with an error, Pun leaves the code alone. Cases that differ from Rector are `Pun/`
  fixtures.
- **Parentheses.** A rule builds new code from the source text of the nodes it keeps and adds parentheses where PHP's
  precedence needs them, also around an expression moved into a new place. It leaves them out on the right of the
  same associative operator, as Rector prints it.
- **Bodies.** Where one statement becomes several in a single-statement body (`if ($a) stmt;`), Pun adds braces, as
  Rector does. In a colon body (`if ($a): ... endif;`) Pun keeps the colon syntax where Rector reprints with braces;
  the laravel preset's format makes both the same. A statement ending in `?>` is left alone.
- **Shared code** lives in `crates/pun-lint/src/rules/support/` (statement lists as php-parser sees them, statement
  removal and replacement, variable usage, list-item removal, class members); rules call it instead of
  copying it. `rules/support/statement_edits.rs` still copies token helpers of `support/statements.rs` (PLAN.md row).
- **Syntax stands in for types until Build 8.** Rules decide that a value is a `bool`, `int`, `array` or `string` from
  the syntax (`syntax_types`: literals, typed parameters never written, locals every write of which has the type,
  built-in functions listed in `crates/pun-lint/data/`). Pure functions and built-in throwables come from the same
  data, generated from pun-phpstan's symbols by `scripts/gen-lint-data.py`. Inside a trait nothing has a type, as in
  Rector's run. Where the syntax cannot tell, the rule keeps the code. Each widens with Build 8.
- **Facts.** Rules that read classes, parents, interfaces and traits scan the whole project (`facts::set_project_scan`).
  The fixtures load rector-laravel's Illuminate stubs (`tests/rector-laravel-stubs`), as its test suite does.
- **Types where Mago differs from PHPStan.** pun-lint gives `preg_match()`/`preg_match_all()` PHPStan's out types
  for `$matches` (data/phpstan-preg-match.stub), follows `@mixin` along method-call chains as PHPStan does without
  Larastan, and types `parse_url()` with a constant component and a narrowed `method_exists()` as PHPStan does.
- **Mago's rules** are report-only unless noted. Their expected problems (`start end message`) come from the Mago
  binary on each rule's doc snippets plus extra cases, in `tests/mago/<group-name>/{Mago,Own}`.
  `tags/no-invalid-open-tag` reports the tag itself, and `control-flow/no-negated-condition` has an unsafe fix that
  Mago lacks.
- **Docblocks.** Pun prints a changed docblock as Rector 2.6.1 did. Rector 2.8.4's format-preserving docblock printer
  and its Doctrine annotation parser are not ported yet, so a few docblock cases differ (tests/rector_types_doc.rs's
  `Pun/` cases) and some refactor Oracle cases still hold Rector 2.6.1's output (tests/rector_refactor.rs).
- **Comparing with Rector on a project** (`scripts/lint-vs-rector.py`): `laravel/prefer-request-validate` changes
  nodes in place, so Rector's JSON never names it and the script runs it alone. The bench's rector.php imports
  rector-laravel's config/config.php, as every rector-laravel set does; without it `laravel/prefer-request-server`
  writes invalid PHP.

Known differences from Rector, left for later (PLAN.md §3 Track B has the rows):

- An alternative-syntax body that a rule empties: Rector rewrites it with braces; Pun does so only in
  `no-useless-continue`.
- A heredoc that Rector prints anew gets php-parser's indent in Rector and keeps its bytes in Pun.
- When a `switch` with only a `default`, or an `if`+`return null` pair, is a whole function or `if`/loop body, Rector
  reprints the parent. Pun does so only for `no-redundant-null-check`'s function bodies, so a signature split over
  several lines stays split elsewhere.
- `no-useless-override` and `no-useless-constructor` treat union types in another order as different and compare
  intersections in declared order; they and `no-redundant-default-argument` do not read a non-empty array or class
  constant default, and keep the code. A parent's `@method` tag is not read.
