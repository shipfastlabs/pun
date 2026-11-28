# AGENTS.md

Pun is one Rust binary for the PHP toolchain. `pun format` produces the same bytes as Laravel Pint. `pun analyse` (parity with PHPStan and Larastan) is in progress.

Pun ships as the Composer package `shipfastlabs/pun` (`composer.json` at the root and the shim `composer/bin/pun`, which downloads the release binary) and through the standalone installer `docs/public/install`, served as `https://punphp.com/install`. No crate, npm package or Homebrew tap is published (`context/plan/foundations.md` §4).

Read these for more:

- `context/PLAN.md`: the source of truth. Every decision, the build order with each row's status, and the open questions. Start here.
- `context/plan/`: one plan per part, each with its Build list. `plan/rules.md` covers rule placement, naming and the upstream mapping.
- `README.md`: the behavior users see.

## Match the upstream tool

Pun's machine formats (`--format=...`), messages, and exit codes must match the pinned upstream tool: Pint, PHP-CS-Fixer, or PHPStan, except that a run error (Pun couldn't run) exits 2 where Pint exits 1 (`context/plan/output.md`). The human output is Pun's own (`context/plan/output.md`), so it does not copy upstream's screen. If you are unsure what upstream does, run it. `scripts/oracle.sh` installs the pinned tools into `.oracle/`. They run on the system PHP 8.5, with no Docker.

- Pint: `php .oracle/pint/builds/pint`
- PHP-CS-Fixer: `.oracle/php-cs-fixer`
- PHPStan: `.oracle/phpstan`

Port upstream code function by function. Use the `pcre2` crate wherever upstream calls `preg_*`. Use `pun_tokens::php_compat` for PHP string behavior, such as byte-based `strlen` and ASCII-only `strtolower`.

Corpus projects live in `corpus/`. `corpus/manifest.toml` lists them, and `scripts/corpus.sh <name>` checks one out. Pinkary is the acceptance project: `corpus/pinkary`, checked out from GitHub at the pinned commit. A local copy may also exist at `/home/forge/projects/pinkary.com`. Never modify that copy.

Run these checks in this order, cheapest first:

```sh
cargo test -p punphp-<x>
cargo test -p punphp-fixers <substring>
cargo nextest run -p punphp-analyse
cargo build -p punphp -p punphp-harness
target/debug/pun-harness format --corpus pinkary --diffs 5
scripts/cli-parity.sh "ARGS"...
scripts/fuzz-parity.sh
scripts/coverage.sh punphp-tokens
```

- Package `punphp-<x>` lives in `crates/pun-<x>` and is imported as `pun_<x>`.
- `cargo clippy --workspace --all-targets -- -D warnings` must pass. The original crates use the shared `[workspace.lints]` (all of pedantic). The six crates that port upstream PHP (pun-tokens, pun-format, pun-fixers, pun-lint, pun-phpstan, pun-analyse) carry their own `[lints]` table: the same lints plus the allows the port shape needs. Silence a single hit with `#[expect(lint, reason = "...")]`.
- `punphp-analyse`'s tests are one binary (`tests/main.rs` declares each file as a module). With `CARGO_PROFILE_DEV_OPT_LEVEL=1`, as CI sets it, it builds in about 5 minutes and runs in about 10 on 8 cores. `levels::` and `rules::` are the two slowest tests; leave them out with `-E 'not test(/^(levels|rules)::/)'` while you iterate. A new test file needs a `mod` line in `tests/main.rs`.
- CI runs the tests with cargo-nextest, one process per test. Locally, `cargo nextest run` and `cargo test` both work.
- `crates/pun-fixers/build.rs` registers every file in `src/fixers/` as a fixer, so a new fixer is one file with a `pub(crate) fn fixer()`.
- `pun-harness` compares Pint with `pun migrate` followed by `pun check --fix`. Add `--fixer <name>` to test one rule.
- `scripts/cli-parity.sh` compares the exit codes, machine formats, and resulting files of Pint and `pun` byte for byte.
- `scripts/fuzz-parity.sh` is a mutation fuzzer that compares Pun with Pint. It needs a freshly built `target/debug/pun`.
- `scripts/coverage.sh` reports line coverage for the crates you name. Without arguments, it covers every crate.

## Performance checks

Pun publishes no benchmark figures: the docs, README and homepage say it is fast, without numbers or comparisons with other tools (`context/PLAN.md` §2). Don't add timings or speedups to them, or to `context/reports/`.

`scripts/bench.py [project...]` remains an internal check for performance work. It times Pun against Pint and Mago (`BENCH=analyse` against PHPStan, `BENCH=lint` against Rector) on corpus projects; its docstring lists the setup and variables. `scripts/lint-vs-rector.py` reuses its project setup. Build `target/profiling/pun` with `cargo build --profile profiling -p punphp` first. Use the results to compare two versions of Pun, and don't publish them.

## Architecture

- `pun-tokens` is the base crate. It holds a lexer that matches `token_get_all`, PHP-CS-Fixer's transformers, and `TokensAnalyzer`.
- `pun-format` runs the fixers (the `Fixer` trait) over one token list, in PHP-CS-Fixer's priority order. It also lints with PHP's own grammar. `scripts/gen-php-grammar.py` generates `php_grammar.rs`.
- `pun-fixers` holds one file per fixer in `src/fixers/`, and `build.rs` registers them. Its `Pun/*` rules are opt-in ports from Mago (`context/reference/mago-extras.md`).
- A fixer's tests come from three sources: upstream fixtures exported to JSON in `tests/fixtures/`, oracle output in `tests/cases/<fixer>.json` (create it with `scripts/oracle-case.php`), and `tests/metadata.json` (the priority and risky flag from Pint).
- `pun-config` reads `pun.toml` and nothing else. `rules/mapping.toml` maps Pun rule IDs to upstream names, and `scripts/check-rule-mapping.py` checks it. `migrate/` converts `pint.json`, `.php-cs-fixer.php`, `rector.php` and `phpstan.neon`. The Pint presets in `crates/pun-config/presets/` are compiled into the crate.
- Pun never runs PHP for a rule it lacks. A rule in `pun.toml` without a native port is a config error (exit 2) that names the rule. System PHP is only the test oracle, and `pun migrate` uses it to read a `.php-cs-fixer.php`.
- `pun-cli`'s `finder` module finds the files a run covers, reproducing Pint's finder and its git path selection. `pun-lint`'s `problem` module is the problem and fix type that `pun-lint` reports and `pun-cli` renders; `pun-cli` converts analyse errors into it.
- `pun-cli` builds the `pun` binary. The crate holds Pint-identical reporters, the compact output for coding agents (`agent.rs`), `--watch`, and the interactive `migrate`.
- `pun-phpstan` ports PHPStan's config, ignore, and type-string behavior. Its built-in symbols are in `data/symbols.json`. `pun-analyse` builds `pun analyse` on `pun-phpstan` and Mago's analyzer.
- `pun-harness` is local tooling. It regenerates `context/reports/scoreboard.md`.
- Release: a tag runs cargo-dist (`dist-workspace.toml`, `.github/workflows/release.yml`), which builds the four archives `composer/bin/pun` downloads (Linux musl and macOS, x86_64 and arm64). `.gitattributes` export-ignores every top-level path except the Composer package; add each new top-level path there. Every crate has `publish = false`.
- The commands divide the work like this. `format` changes layout only. `lint` applies code rules, including PHP-CS-Fixer's risky fixers. `analyse` reports type errors. `check` runs all three in one pass.
- The Mago crates are pinned to `=1.61.0`. Bump them together. Four of them (mago-analyzer, mago-codex, mago-syntax, mago-phpdoc-syntax) are vendored and patched in `third_party/`; each has a PATCHES.md, and a bump re-applies those patches.
- `crates/pun-blade` (native Blade, `[format] blade = "native"`) uses oxc's formatters from git, pinned to one rev in the root `Cargo.toml` (with a `[patch.crates-io]` for `oxc_allocator`). `oxc_formatter_css` is vendored and patched in `third_party/` with a PATCHES.md. Check a change against Prettier with prettier-plugin-blade: `cargo test -p punphp-fixers --test blade native_blade` runs the golden files.
- Harness-only environment variables: `PUN_IGNORE_PHP_CONSTRAINT=1` (for corpora that allow PHP below 8.3), `PUN_BIN`, `PUN_HARNESS_ROOT`, `PUN_PROFILE`, `PUN_CACHE_STATS` (`pun analyse` prints how many files its result cache reused and analysed, for its tests), and `PUN_GITHUB_URL` (the plain-HTTP server `pun self-update` downloads from in its tests, instead of github.com and api.github.com).
- Every test that spawns `pun` sets `PUN_AGENT=0`. Tests of agent mode are the exception.

## Build budget

1. Run `source ~/.cargo/env`.
2. Size builds to the machine: check `nproc` and free memory, and leave headroom for the subagents running beside you.
3. Prefix long commands with `timeout 900 nice`, for example `timeout 900 nice cargo test -p punphp-tokens`.
4. Build and test only the crates you changed (`-p`). Give each concurrent build its own `CARGO_TARGET_DIR`
   (`target-<name>`) so two cargo runs never wait on one lock or reuse each other's stale artifacts.
5. Delete scratch target dirs and downloads when you no longer need them. Keep `target/`. Check `df -h` before you start another one.

Corpus runs cover one fixer on one project. While you iterate, use Pinkary or laravel-framework.

## Tests

Every crate needs 100% line coverage, measured by `scripts/coverage.sh`. Delete any line that no test can reach. A rare, justified exclusion goes in `context/reports/COVERAGE.md` with its reason.

Take each expected value from a source other than the code under test: an upstream fixture, the real tool's output, `token_get_all`, or a value you derived by hand. To cover a branch that upstream's fixtures miss, write an input, run the oracle on it, and paste the oracle's output as the expectation.

Keep a test only if it fails on a plausible wrong implementation. If you cannot name that bug, delete the test. These tests fail the bar:

- A function compared with itself or with a copy of its own logic.
- A snapshot of the current output.
- A test with no assertion.
- A test that asserts only "does not panic" when the output is checkable.
- A mock that only verifies calls.
- A test of a private helper that the public API already covers.

## How work runs

One lead agent drives the build and works on `main`. It spawns subagents for independent work and merges what
they return.

- **Always work on the `main` branch.** Commit on main and push to `origin/main`. Do not create long-lived
  branches, `helper/*` branches or pull requests. Rewrite history or force-push only when Pushpak asks.
- At the start of a session, read `context/PLAN.md` (§0 "Resume here" first), then run `git log -15`,
  `git status` and `git fetch origin`. Pick the next row from §0's priorities.
- Before you start a row, set its status in `context/PLAN.md` §3 to `doing`. When it lands, delete the row and
  update §0's counts.
- **Subagents.** Spawn as many as the work and the machine allow, one per independent slice (a rule group, a
  crate, a bug). Give each a slice no other subagent touches, the files it may edit, and the check that proves it
  done. A subagent that edits files runs in its own git worktree (`isolation: "worktree"`, or
  `git worktree add ../pun-<name> main`) with its own `CARGO_TARGET_DIR`, commits there, and the lead merges the
  result into main, runs the tests and pushes. Remove the worktree after the merge. Read-only subagents (search,
  review, oracle runs) need no worktree.
- Shared files (`context/PLAN.md`, `crates/pun-config/rules/mapping.toml`, each crate's `rules/mod.rs`): subagents
  change only their own lines; the lead resolves conflicts at merge time by keeping both sides.
- Never `git stash`, `git reset --hard` or `git clean` in the main checkout while subagents run. Commit with
  explicit paths: `git commit -m "..." -- path1 path2`.
- When a decision is genuinely Pushpak's, add a `needs Pushpak` row in `context/PLAN.md` §3, take the option
  closest to upstream, and keep going.

## Commits

Write each commit message as a Conventional Commit: `type(scope): subject`. Use a lowercase subject and no trailing period. The types are feat, fix, docs, chore, test, perf, refactor, ci, style, and build.

The only author is Pushpak Chhajed <pushpak1300@gmail.com>. Do not add `Co-authored-by` trailers.

## Releases

Release only when Pushpak asks. Pushing a `vX.Y.Z` tag runs `.github/workflows/release.yml` (cargo-dist), which builds the four archives, attests them, and publishes the GitHub release that `pun self-update`, `docs/public/install` and `composer/bin/pun` download. Packagist reads the same tag as the Composer package's version. A suffix such as `-beta.1` makes it a pre-release.

1. Check that CI passed on the commit you release.
2. Set the version in two places: `version` under `[workspace.package]` in `Cargo.toml`, and `PUN_VERSION` in `composer/bin/pun` (`tests/composer.rs` checks that they match). Run `cargo build -p punphp` so `Cargo.lock` follows, and check `target/debug/pun --version`.
3. Commit the three files as `chore(release): version X.Y.Z` and push it to `origin/main`.
4. Tag that commit with a lightweight tag, `git tag vX.Y.Z`, and push only the tag: `git push origin vX.Y.Z`.
5. Watch the Release workflow (`gh run watch`). When it finishes, `gh release view vX.Y.Z` lists the four `punphp-<target>.tar.gz` archives, their `.sha256` files, `sha256.sum` and `dist-manifest.json`.
6. Check the installed binary: `curl -fsSL https://punphp.com/install | sh -s -- vX.Y.Z`, then `pun --version`.

Never move or delete a published tag. If a release is broken, fix it on main and release the next patch version.

The shipfastlabs organization runs only GitHub Actions pinned to a full commit SHA, so a workflow that uses `@v4` fails at "Set up job". Pin each action as `owner/action@<sha> # <tag>`. `release.yml` is generated by cargo-dist: set its actions' SHAs under `[dist.github-action-commits]` in `dist-workspace.toml`, run `dist generate --mode ci`, and check it with `dist generate --check`.

## Docs site

`docs/` is also the public docs site, built with [Blume](https://useblume.dev). To run it, use `bun install && bun run dev` inside `docs/`.

- `docs/content/docs/*.mdx`: the pages.
- `docs/pages/index.astro`: the homepage.
- `docs/theme.css`: the brand tokens. They mirror `docs/design/tokens.json`.
- Internal notes (plan, decisions, tasks, reference) live in `context/`, not `docs/`.

When a change alters a flag, a rule, or behavior, update the matching page under `docs/content/docs/`. The `blume` and `blume-update-docs` skills in `.claude/skills/` cover the tooling.
