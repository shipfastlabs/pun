# Plan: rule placement, overlaps, naming and the mapping data

Pun replaces five upstream tools (PHP-CS-Fixer/Pint, Rector, PHPStan with its extensions, Larastan, Mago's linter), and
their rules overlap a lot. This file says where each rule lives, how overlaps resolve, how rules are named, how they
are ignored, and what the mapping data holds. Rule IDs, groups, presets, sets and levels: config.md §4-§5. Decisions:
../PLAN.md §2.

The data is [`crates/pun-config/rules/mapping.toml`](../../crates/pun-config/rules/mapping.toml): every Pun rule and
every upstream rule, each upstream entry pointing at a Pun rule or carrying a drop or unsupported reason. It drives
`pun migrate` (configs, ignore comments), `pun explain <rule>`, and the generated "Coming from Pint / PHPStan / Rector"
docs page. `scripts/check-rule-mapping.py` checks it (§5). Counts are in §6.

Some examples in §2 and §3 use rule names from before the `<command>/<group>/<name>` rename; mapping.toml has the
current IDs.

## 1. Placement

| Command | Owns | Never | Fix |
| --- | --- | --- | --- |
| `pun format` | Layout, casing, spelling of the same construct (`array()` to `[]`, quotes, `else if` to `elseif`, `\Foo` to an import), sorting lists whose order means nothing (imports, union types, `implements`, PHPDoc tags) | Changes behaviour; removes code; adds or removes PHPDoc; rewrites one construct into another; changes declarations or member order | Always applied; safe on save |
| `pun lint` | Every other edit, plus every report one file can decide (types may make a fix safer) | Pure layout | Labelled safe, unsafe or none; `--fix` applies all |
| `pun analyse` | Reports that need type inference or other files (types, undefined symbols, inherited PHPDoc, deprecations) | Edits files | None; a report a lint or format rule can fix belongs to that rule, not to analyse |

The tests, in order; the first match wins:

1. **Risky in PHP-CS-Fixer?** Then lint, labelled unsafe. Pun keeps upstream's risky flag as is.
2. **Only layout, casing or spelling** (the format column above)? Then format. A Rector or Mago rule that does the same
   as such a fixer maps onto the format rule.
3. **Any other edit?** Lint, labelled safe (keeps behaviour) or unsafe (can change it). The 40 safe PHP-CS-Fixer
   fixers that delete or restructure code go here (commands.md §5).
4. **A report one file can decide without types?** Lint with no fix.
5. **Otherwise** it needs types or other files: analyse.

**Exception (parity):** every PHPStan core and Larastan identifier stays in analyse until 1.0, even where test 4 would
pick lint, so `pun analyse` matches `phpstan analyse` on migration day. Two carve-outs are layout and map to format:
`whitespace.bom` and `whitespace.fileEnd`. strict-rules and deprecation-rules are placed by the tests.

For Pint projects this means the risky and code-deleting fixers a pint.json turns on are lint rules; `pun migrate`
writes them under `[lint.rules]` and `pun check --fix` still writes Pint's bytes (format.md §4).

## 2. Overlaps

The mapping has 105 overlap clusters. Every cluster ends as one Pun rule in one command, unless §7's one-to-one rule
splits it.

| # | Overlap | Home | The others at migrate time |
| --- | --- | --- | --- |
| O1 | A format-safe PHP-CS-Fixer fixer and a Rector or Mago rule | The format rule, or the lint rule if the fixer is one of the 40 | Enabling the Rector or Mago rule enables it. PHP-CS-Fixer's options win; migrate prints the others it drops |
| O2 | A risky PHP-CS-Fixer fixer and anything | One lint rule, unsafe | All map to it |
| O3 | Rector, Mago and Pun with no PHP-CS-Fixer rule | One lint rule | All map to it; where strategies differ, the rule covers both |
| O4 | A syntax-only strict-rules ban and a fixer | Lint | `ignoreErrors` entries and `@phpstan-ignore` comments for the identifier move to the lint rule |
| O5 | A type-aware report and a fixer (PHPStan `method.unused` and an unused-private-method fixer) | The lint (or format) rule; analyse keeps only the identifiers no rule fixes (Pushpak, 2026-11-02) | The PHPStan class and identifiers map to the fixer, so `ignoreErrors`, `@phpstan-ignore` and `pun migrate` move to it. `pun analyse` still reports them, under the lint rule's ID, when a PHPStan class that reports them runs (its analyse rule or the lint rule is on). The lint rules see no types and miss most of them, and an error must show in lint or analyse (Pushpak, 2026-11-18) |
| O6 | A fixer whose safety depends on types | One lint rule | Labelled safe where analysis proves it, unsafe otherwise; `--fix` applies both |
| O7 | Rules that contradict each other | Both stay | `[meta] conflicts`; config loading refuses a config that enables both |
| O8 | Upstream deprecated or no-op rules | The successor | Mapped to it, or dropped with a note in migrate's output |
| O9 | Out of scope: rector-symfony, rector-doctrine, rector-downgrade-php, Rector core's and PHP-CS-Fixer's Doctrine, Symfony, Behat and JMS rules, Mago's WordPress, Psl, Symfony and Drupal rules, PHPStan's `phpstanApi.*` | None | `unsupported`: migrate names each one and skips it |

**Example: loose comparison.** PHP-CS-Fixer's `strict_comparison` (risky, every `==`) and Mago's
`identity-comparison` map to `lint/types/no-loose-comparison`. It needs no types and has a fix, so it is lint. Its fix
is `mixed`: safe where analysis proves both operands have the same type, unsafe otherwise, and `pun lint --fix`
applies both.

**Conflicts** are listed in `[meta] conflicts`: today `format/imports/single-per-statement` /
`format/imports/group`, `lint/control-flow/prefer-bare-return` / `lint/complexity/require-consistent-return`, and
`lint/style/prefer-short-ternary` / `lint/style/no-short-ternary`. No preset or set may enable both sides.

## 3. Naming style and vocabulary

The ID shape (`<command>/<group>/<name>`), the group list and "no aliases" are in config.md §4. Within that shape:

- **Format names** describe the construct (`arrays/syntax`, `imports/ordered`); `no-` only when the rule removes
  something (`whitespace/no-trailing`). See format.md §8.
- **Lint names start with a verb:** `no-` removes or bans; `prefer-X` replaces code with the better form X; `add-`
  inserts something that can be inferred (types, attributes, PHPDoc); `require-` reports, and may insert, what a policy
  says must be there. PHP-version rules are mostly `prefer-X` (`prefer-array-find`, `prefer-clamp`); the ones that fix
  code a newer PHP removed are `no-X`.
- **Configurable codemods** (rules that do nothing until configured, such as a method rename) are named after their
  edit: `rename-`, `replace-`, `remove-`, `change-`, `wrap-`. Only codemods use these prefixes, and they sit in the
  `refactor` group.
- **Analyse names** name the problem: `<word>-<thing>` (`undefined-method`, `unused-result`) or `<thing>-<problem>`
  (`argument-type`, `condition-always-true`). With groups this reads `undefined/method`, `types/argument`.
- **The name doesn't repeat the group**, is kebab-case and is unique within its group.
- **Short and familiar.** Two to four words, never the upstream mechanism (`add-return-type`, not
  `return-type-from-strict-native-call`). Where Biome, ESLint, oxlint, Ruff or typescript-eslint has a name for the same
  idea, use it (`no-lonely-if`, `no-useless-catch`, `prefer-spread`); else Mago's; else PHPStan's wording. The reader is
  a Laravel developer who has never used Rector or PHPStan. So `modernize_strpos` is `prefer-str-contains`, PHPStan's
  `method.notFound` is `undefined-method`, and Larastan's `larastan.noEnvCallsOutsideOfConfig` is
  `no-env-outside-config`.

**Vocabulary.** Each word means one thing in every command:

| Word | Means | Examples |
| --- | --- | --- |
| `unused` | declared, assigned or computed, never read | `no-unused-assignment`, `unused-private-method`, `no-unused-ignore` |
| `undefined` | referenced, declared nowhere | `undefined-method`, `undefined-variable` |
| `missing` | something required is absent (lint inserts it with `add-` or `require-`) | `missing-return-type`, `missing-argument` |
| `redundant` | no effect because something else already guarantees it | `no-redundant-nullsafe`, `redundant-null-coalesce` |
| `useless` | no effect on its own; removing it never changes behaviour | `no-useless-cast`, `no-useless-override` |
| `dead` | a branch or handler that can never run | `no-dead-catch`, `dead-catch` |
| `unreachable` | statements after `return`, `throw` or `exit` | `no-unreachable-code`, `unreachable-code` |
| `invalid` | malformed, or contradicts its declaration | `invalid-attribute`, `invalid-phpdoc` |
| `impossible` / `always-true` / `always-false` | a check whose result the types decide | `impossible-type-check`, `condition-always-false` |
| `deprecated` | uses something marked deprecated | `deprecated-method` |

`unnecessary` is not used; `redundant` covers it.

**Merged duplicates** (Pushpak, 2026-11-15). One lint rule may map to several upstream rules that do the same thing
(PHP-CS-Fixer's `logical_operators` and Rector's equivalent are both `lint/operators/no-and-or-keywords`). A keeper
lists its primary sources in `merged` (required when it has two or more) and the old Pun IDs in `replaces`. Where a
group had a PHP-CS-Fixer or `Pun/*` token fixer, that fixer wins and the other implementations were deleted, for Pint
parity. Where every implementation is native, they keep running under the keeper's ID, and the linter reports a span
once, preferring the report with a fix. Behaviour a deleted implementation had that the keeper lacks is in the keeper's
`note`. An old ID in pun.toml, `-A`/`-W`/`-D` or `pun explain` names the keeper.
docs/content/docs/migrate/merged-rules.mdx lists every merge. Families whose rules take different strategies stay
separate (strict flags, loose comparison, nullsafe, protected in final, void return, PHPUnit attributes and dedicated
asserts, PHPDoc cleanup, useless else, call_user_func). Analyse rules stay one to one with PHPStan `Rule` classes,
Mago-derived rules keep their Mago mapping, and format rules may carry several sources. PHPStan's identifiers are
`variant`s of the rule of the class that emits them (§5).

## 4. Ignore comments

| Form | Applies to |
| --- | --- |
| `$x = foo(); // @pun-ignore <id>` | the line it trails |
| `// @pun-ignore <id>` on its own line | the next line (as PHPStan's `@phpstan-ignore`) |
| `// @pun-ignore-next-line <id>` | the next line, explicitly |
| `// @pun-ignore-file <id>` before the first statement | the whole file. The only form for format rules, which can't be half-applied to a line; keeps `@php-cs-fixer-ignore`'s meaning |
| `// @pun-ignore-start <id>` ... `// @pun-ignore-end` | a region |

- One or more IDs, comma-separated: a full ID, a rule's `group/name`, or a group (`lint/security` or `security`).
  A bare `@pun-ignore` with no ID is an error. Every form is built (crates/pun-lint/src/problem/ignore.rs).
- Free text after ` -- ` is the reason: `// @pun-ignore lint/security/no-eval -- legacy plugin loader`.
- `/* */` and `/** */` forms work too, so PHPDoc-placed `@phpstan-ignore` ports one to one.
- `comments/no-unused-ignore` reports unused ignores (safe fix: delete); `comments/invalid-ignore` reports malformed
  ones.
- Migrate rewrites `@phpstan-ignore <id>` to `@pun-ignore <pun id>` through the mapping, deduplicating IDs that map to
  one rule. `@phpstan-ignore-next-line` (no ID) becomes `@pun-ignore-next-line`, and `@phpstan-ignore-line` becomes
  `@pun-ignore`, with the rules of the errors `pun analyse` reports on the line it covers (migrate runs no PHP, so Pun's
  analyser stands in for PHPStan's report). The comment stays as it is, and migrate names its line and the reason, when
  that line has no error, the Pun comment would cover another line, or one comment holds two tags; `pun analyse` still
  honours it as PHPStan does. `@php-cs-fixer-ignore <rule>` becomes `@pun-ignore-file <format id>`.

## 5. The mapping data

The comment block at the top of mapping.toml documents every field. Example entries:

```toml
[rules.lint]
"types/no-loose-comparison" = { fix = "mixed", intent = "suspicious", sets = ["code-quality"], ported = true, status = "decided", summary = "Comparisons should be strict." }

[rules.analyse]
"types/missing-return-type" = { preset = "strict", ported = true, fixed_by = ["lint/types/add-void-return-type"], status = "decided" }

[upstream.php-cs-fixer]
strict_comparison = { pun = "lint/types/no-loose-comparison", risky = true, options = "none", status = "decided" }

[upstream.phpstan]
"method.unused" = { pun = "lint/dead-code/no-unused-private-method", variant = "method.unused", level = "4", options = "none", status = "decided" }
```

- **Rules** sit under `[rules.<command>]`, keyed `<group>/<name>`. Every `pun`, `fixed_by` and `conflicts` reference
  is a full `<command>/<group>/<name>`. `[meta.groups]` holds the fixed group list with descriptions. Lint rules carry
  `sets`, analyse rules a `preset` (from the PHPStan level), format rules `presets` (`laravel`, `psr12`). `fix` is a
  label: safe, unsafe, mixed or none. There are no aliases, severities or tiers.
- **Sections:** `[upstream.<tool>]` for `php-cs-fixer`, `pint`, `pun` (the `Pun/*` Mago ports), `rector`,
  `rector-laravel`, `pest-plugin-rector`, `rector-phpunit`, `rector-symfony`, `rector-doctrine`,
  `rector-downgrade-php`, `phpstan-classes` (one entry per `Rule` class of PHPStan and its extensions, with `class`,
  `package`, `identifiers`, `level`, `gated_by`), `phpstan` (by identifier; `level` is the lowest level that emits it;
  `pattern = true` for `%s` identifiers), `phpstan-strict-rules`, `phpstan-deprecation-rules`, `larastan`,
  `mago-linter`, `mago-analyzer`; plus `[phpstan-parameters]`.
- **Upstream keys** are the exact upstream IDs. Rector and Pest entries also carry the fully qualified `class`.
  `options` says how upstream options carry over.
- **`variant`** on PHPStan-family entries keeps the original identifier when one Pun rule replaces several, so `-v`
  output can print it and migrated `ignoreErrors` keep their narrow scope.
- **Pinned upstream versions** are in `[meta] upstream`: PHP-CS-Fixer 3.95.25, Pint 1.32.1, Rector 2.8.4 (with
  rector-phpunit dev-main@22c6dae bundled), rector-laravel 2.7.3, pest-plugin-rector 5.x-dev@8c1b780, PHPStan 2.3.5,
  strict-rules 2.0.12, deprecation-rules 2.0.5, Larastan 3.14.0, Mago 1.61.0. reference/oracle.md says how to fetch
  each.

**The checker** (`scripts/check-rule-mapping.py`; its docstring lists every check) enforces §3's naming, the group
list, references, exactly one of pun / drop / unsupported per upstream entry, and conflicts. Against the upstream
inventories in `crates/pun-config/rules/upstream/*.json` it also enforces §7: every in-scope upstream rule maps to a
Pun rule, no deprecated or out-of-scope one does, each lint or analyse rule has one primary source (or `merged`, or is
Mago-derived), and a Rector-backed rule sits in a Laravel upgrade group below the floor (Laravel 12) only when its
sources come from rector-laravel's sets below `laravel-120`. An upstream
bump regenerates the inventories, so every new upstream rule needs a decision.

## 6. Counts

From mapping.toml (`scripts/check-rule-mapping.py` prints them):

| Upstream (version) | Entries | → format | → lint | → analyse | dropped | unsupported |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| PHP-CS-Fixer 3.95.25 | 300 | 163 | 115 | - | 18 (deprecated → successor) | 4 |
| Pint's own fixers | 3 | 1 | 1 | - | 1 | - |
| Pun `Pun/*` (Mago ports) | 17 | - | 17 | - | - | - |
| Rector core 2.8.4 | 523 | 15 | 269 | - | 2 | 237 |
| rector-laravel 2.7.3 | 141 | - | 94 | - | - | 47 |
| pest-plugin-rector 5.x-dev@8c1b780 | 61 | - | 59 | - | - | 2 |
| rector-phpunit (bundled) | 116 | - | 81 | - | - | 35 |
| rector-symfony / doctrine / downgrade-php | 267 | - | - | - | - | 267 |
| PHPStan-family `Rule` classes | 433 | 4 | 31 | 370 | - | 28 |
| PHPStan 2.3.5 core identifiers | 697 | 5 | 24 | 646 | 7 | 15 |
| PHPStan level parameters | 18 | - | - | 13 options + 5 switches | - | - |
| phpstan-strict-rules 2.0.12 identifiers | 44 | 1 | 17 | 26 | - | - |
| phpstan-deprecation-rules 2.0.5 identifiers | 46 | - | - | 46 | - | - |
| Larastan 3.14.0 identifiers | 20 | - | 4 | 16 | - | - |
| Mago 1.61.0 linter | 194 | 29 | 144 | - | 2 | 19 |
| Mago 1.61.0 analyzer codes | 335 | - | 8 | 192 | - | 135 |
| **Pun rules** (1,214) | 3,215 | **164** | **680** | **370** | | |

Ported (`ported = true`): 164 of 164 format rules, 679 of 680 lint rules and 365 of 371 analyse rules. In scope: 518
Rector-family rules (503 → lint, 15 → format) and 405 PHPStan-family classes (370 → analyse, 31 → lint, 4 → format).

## 7. The one-to-one mapping

Each lint and analyse rule has one primary upstream source (one Rector rule, PHP-CS-Fixer fixer, Pint fixer or
PHPStan-family `Rule` class), except merged keepers (§3) and Mago-derived rules (`origin = "mago"`). Each analyse rule
is one PHPStan class; an identifier maps to the rule of the class that emits it.

**In scope.** A Rector rule is in scope when a planned set registers it (the inventories' `in_scope`): Rector's prepared
sets (`dead-code`, `code-quality`, `coding-style`, `type-declaration` and its docblocks set, `early-return`,
`privatization`, `naming`, `instanceof`, `rector-preset`), the PHP sets from `php82` up, rector-laravel's quality sets
and its version sets from `laravel-120` up (`livewire-40` too), rector-phpunit's quality sets and `phpunit110` up, and
Pest's `coding-style`. It is out when Rector deprecated it, when only upgrade sets below PHP 8.2, Laravel 12 or
PHPUnit 11 register it, when only other sets (`named-args`, `if`, `datetime-to-carbon`, `php-polyfills`, Lumen, old
package majors) register it, or when no set does. A PHPStan class is in scope when a config registers it (O9 keeps
`phpstanApi.*` out). The inventories are crates/pun-config/rules/upstream/*.json.

The PHP sets start at `php82`, below Pun's PHP 8.3 floor (Pushpak, 2026-11-18): their rules still change code on a
PHP 8.3+ project, and a rector.php that uses them would otherwise lose them in `pun migrate`. Their rules sit in the
`php-8.2` and `php-8.3` groups.

Rector's PHP upgrade rules below `php82` and its `php-polyfills` rules are being ported too ("Upgrades below the
floor too", PLAN.md §2, 2026-11-23) into the `php-5.3` ... `php-8.1` groups. The inventories still mark them out of
scope, so the checker allows a Pun rule for them and does not yet require one. A rule whose input no longer parses or
compiles on PHP 8.3+ (`$s{0}`, an unparenthesized nested ternary) stays `unsupported` with that reason.

rector-phpunit's upgrade rules below `phpunit110` go into the `phpunit` ecosystem group, not into `phpunit-<N>`
groups: rector-phpunit has no per-version sets, only `composer-based`, which binds each rule to a phpunit/phpunit
version, and a group follows the upstream set the way `php-5.3` ... `php-8.1` follow Rector's php53 ... php81 sets. They
are in no lint set, like the other rules only `composer-based` registers, and the checker allows them, not requires them.
The two configurable core rules those sets configure (`AddParamTypeDeclarationRector`, `AddReturnTypeDeclarationRector`)
go into `refactor`, beside Rector's other configured rules.

rector-laravel's version sets below `laravel-120` go into one group per set (`laravel-5.7`, `laravel-5.8`,
`laravel-8` ... `laravel-11`; the legacy-factories migration into `laravel-8`), gated by the locked Laravel version
like `laravel-12`, so they also run on Laravel 12 and 13. Its package sets below the current majors go into
`cashier-13`, `livewire-3` and `faker-1`, the Lumen set into `lumen`, and its configurable rules into `refactor`
(assistant's call, 2026-11-24).

Rules removed for being out of scope keep their Rust code until the perf rounds end.

**Decisions** (assistant's calls where the request was silent):

- Pun's `Pun/*` ports count as Mago-derived, so a rule with one keeps its Mago mapping and its Rector sources split.
- A shared rule keeps its ID for the source closest to its name; PHP-CS-Fixer and `Pun/*` sources keep theirs.
- Strict-rules classes that are syntax only stay lint rules (O4), one rule each.
- Rector-backed refactor codemods get no set; `naming` has no rules (all of Rector's naming rules are deprecated).
- An identifier emitted by several classes points at one: core PHPStan before the extensions, then the class with the
  fewest identifiers, then the lowest level.
- An analyse rule's preset is the band of its class's level; strict-rules and deprecation-rules classes without a
  level keep the old preset, else `strict` / `recommended`.

## Borrowed from

- **Biome:** formatter and linter split, `check` as the one command, region ignores, `lint/<group>/<rule>` IDs.
- **Ruff:** one rule per idea whatever its origin, upstream codes redirected to it; `modernize` gated by target version.
- **oxlint:** `off` / `warn` / `error` per rule or category, `--deny-warnings`, `--max-warnings`, overrides per path.
- **Vite+ (`vp check`):** one config file and one `check` that runs every step in one process.
- **Rector:** prepared sets you tick, which became lint sets.
- **ESLint, typescript-eslint, Biome:** the `recommended` and `strict` names.
- **Go (`gofmt` / `vet`):** the formatter never changes behaviour and is never configured per line.

## Decisions

Step 1, config.md Build 1-2, assistant's calls where the plan is silent:

- **Groups beyond format.md §8.** Lint and analyse add the topic groups `complexity`, `dead-code`, `debug`,
  `deprecated`, `performance`, `refactor` (configurable codemods only), `security`, `style` and `undefined`, and one
  upgrade group per target (`php-8.1` ... `php-8.6`, `laravel-12`), from the rule's Rector set or `gated_by` version.
  `php-8.1` holds only Mago's `prefer-first-class-callable`. The ecosystem groups are `laravel`, `pest` and `phpunit`.
  The full list with a description each is `[meta.groups]` in mapping.toml.
- **`correctness` and `suspicious` are not groups:** they are the `intent` field (config.md §4), and each rule has a
  topic group.
- **Lint `sets`** come from the Rector sets of the upstream rules mapped to the rule (`dead-code`, `code-quality`,
  `coding-style`, `early-return`, `privatization`, `naming`, `type-declaration` as `types`, the PHP and Laravel level
  sets as `modernize`). A rule with no Rector source takes the set its kind suggests: dead and unused code →
  `dead-code`, style policy → `coding-style`, likely bugs and simplifications → `code-quality`; pedantic and opt-in
  style rules are in no set. The codemods in `refactor` are in no set.
- **Analyse `preset`** is the band of the lowest PHPStan level that emits any identifier mapped to the rule (basic 0-1,
  essential 2-3, recommended 4-5, strict 6-8, max 9-10). Rules with no PHPStan level (strict-rules, deprecation-rules,
  Larastan, Mago-only) take their old tier: `essential` → `basic`, `recommended` → `recommended`, `strict` → `strict`,
  `pedantic` → `max`.
- **The pun.toml reader** accepts a full ID or `<group>/<name>` inside its own `[<command>.rules]` table. Any other
  name is an error. When the name is an upstream key (any casing or separator), an old Pun name, or a rule of the other
  command, the error names the Pun ID: ``pun.toml: `array-syntax` is `format/arrays/syntax` in pun.toml``. A name
  several rules could mean is not guessed.
- **Lint names for the 40 fixers moved from format** (commands.md §4) follow §3's verb rule, for example
  `lint/imports/no-unused`, `lint/control-flow/prefer-bare-return`; `[upstream.php-cs-fixer]` in mapping.toml maps
  each one. `protected_to_private` merges with Mago's rule and takes its name, `lint/classes/no-protected-in-final`.
  The three configurable `general_*` fixers are `refactor` codemods.
- **Other names.** Analyse names drop the word their group carries (`argument-type` → `types/argument`,
  `invalid-ignore-comment` → `comments/invalid-ignore`). `no-excessive-complexity` is
  `complexity/no-excessive-cyclomatic` (it counts cyclomatic complexity), `require-string-style` is
  `strings/require-interpolation-style`, and the Pun-only `no-unused-ignore` is `comments/no-unused-ignore`.
