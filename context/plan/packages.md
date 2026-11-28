# Plan: the package manager (`pun install`, `pun update`, `pun require`, `pun remove`)

Status: steps 2-6, 8 and 9 are built, with parts left (§5); step 7 is built for the script events; steps 1, 10 and
11 are partly or not built. Workspaces and `pun x` are built (§7). Decisions: §6. Beyond Composer: §7. Open
questions: §8.

## 1. What it is

Pun installs a project's dependencies the way Composer does, much faster. It reads composer.json, reads and writes
composer.lock, and fills `vendor/` with the same files `composer install` writes, so `composer` and `pun` work on the
same project side by side (§6). The proof is parity: on every corpus project, `pun install` and `composer install`
give byte-identical `vendor/` trees from the same lock, and `pun update` writes the lock `composer update` writes.

The everyday commands are top-level, with Composer's names and flags:

| Command | Composer | What it does |
| --- | --- | --- |
| `pun install` | `composer install` | Installs composer.lock (resolves and writes it when there is none) |
| `pun update [packages]` | `composer update` | Resolves again, writes composer.lock, installs |
| `pun require <pkg[:constraint]>` | `composer require` | Adds to composer.json, then a partial update |
| `pun remove <pkg>` | `composer remove` | Removes from composer.json, then a partial update |
| `pun composer <command>` | `composer <command>` | Every other Composer command. Built: `dump-autoload`, `run-script`, `exec`, `config`. Not built: `show`, `outdated`, `why`, `why-not`, `audit`, `validate`, `licenses`, `check-platform-reqs`, ... |

`pun composer install` and `pun install` are the same command. A `pun composer` command Pun has not built yet is a
run error (exit 2) that names it; Pun does not shell out to the Composer CLI.

## 2. Today

On pinkary, `pun install` from a lock and `pun composer dump-autoload` (every flag) write Composer 2.11.2's `vendor/`
byte for byte, modes included, and `pun update` writes Composer's exact lock. The resolver passes 158 of Composer's
231 installer fixtures (102 compared on the whole composer.lock). Install checks the lock against the platform and
against composer.json (exit 4 unless `config.allow-missing-requirements`, as Composer), skips a no-op run by a
fingerprint of its inputs and outputs, and links packages from a shared store (§7.2). The plugin host runs allowed
plugins for the script events. What is left is in §5.

## 3. The decided behaviour

- **composer.json and composer.lock hold the dependencies**, as package.json and bun.lock do for Bun. The package
  commands read composer.json and write composer.lock byte for byte as Composer 2.11.2 does; `require` and `remove`
  edit composer.json in place, keeping its formatting as Composer does. pun.toml holds only Pun's own settings:
  `[workspace]` and the tooling sections, as bunfig.toml holds only Bun's. A composer.json section in pun.toml
  (`[require]`, `[autoload]`, ...) is a config error (exit 2) that says it belongs in composer.json. `pun migrate` does
  not touch composer.json or composer.lock.
- **`vendor/` is Composer's layout:** package directories, `vendor/composer/` (`installed.json`, `installed.php`,
  `InstalledVersions.php`, `ClassLoader.php`, `autoload_*.php`, `platform_check.php`, `LICENSE`), `vendor/autoload.php`
  and the `vendor/bin` proxies, all byte-identical to Composer 2's. `ClassLoader.php` and `InstalledVersions.php` are
  Composer's own files, embedded with its MIT notice.
- **The same resolution as Composer.** The solver follows Composer's SAT solver and policy, so `pun update` picks the
  versions Composer picks, including on conflicts. Pun's conflict message is its own human output over the same
  problem set (output.md).
- **Composer's environment:** `COMPOSER_HOME`, `COMPOSER_CACHE_DIR` (the same cache, so Pun and Composer share
  downloads and metadata), `auth.json` (project and home) and `COMPOSER_AUTH`, `config.platform`, `minimum-stability`,
  `prefer-stable`, `preferred-install`, `vendor-dir`, `bin-dir`, `optimize-autoloader`, `classmap-authoritative`,
  `apcu-autoloader`, `sort-packages`, `allow-plugins`, `process-timeout`, `allow-missing-requirements`, `audit` and
  `policy` settings.
- **Repositories:** Packagist (the v2 metadata API), any Composer repository (Private Packagist, Satis), `path`,
  `package`, `vcs`/`git`/`github`/`gitlab` (through the host's API and git), and `artifact`. Not `svn`, `hg` or `fossil`.
- **The platform is the system PHP's.** Pun asks the `php` on the PATH once for its version, extensions and their
  versions, as Composer sees them, unless `config.platform` says otherwise. Scripts run on that PHP too.
- **Scripts and events run as Composer runs them:** shell commands, `@php`, `@composer`, `@putenv`, script
  references, and PHP callbacks. A callback gets a `Composer\Script\Event` from a small bundled PHP shim, enough for
  Laravel's `ComposerScripts`, Pest's and the common callbacks; the shim's API is tested against the corpus.
- **Composer plugins work.** An allowed plugin runs as it runs under Composer, through a plugin host:
  - The host is a PHP process on the system PHP running Composer's own classes (a pinned `composer.phar`, MIT,
    downloaded once with a checksum into Composer's cache, not bundled into the binary). It builds a real `Composer`
    object from the files Pun has written (`composer.json`, `composer.lock`, `vendor/composer/installed.json`),
    activates the allowed plugins as Composer does, and dispatches the events Pun sends it.
  - Pun fires every event Composer fires, in Composer's order: the script events, the package events
    (`pre-package-install` and the rest), the installer events (`pre-operations-exec`), and the plugin events
    (`pre-pool-create`, `pre-file-download`, `post-file-download`, `command`, `init`). Where a plugin changes Pun's
    work (a pool filter like laravel/vet's, a custom install path like composer/installers', a gated operation, a
    rewritten download URL), the host sends the change back and Pun applies it.
  - A plugin's commands (like Pest's `pest:dump-plugins`) run through the host as `pun composer <command>`.
  - The host starts only when an allowed plugin with no native port subscribes to an event the command fires, and
    once per command. Laravel projects need this: every Laravel corpus project allows `pestphp/pest-plugin` and
    `php-http/discovery`, and `post-autoload-dump` runs Laravel's `ComposerScripts` and `artisan package:discover`.
  - The plugins every Laravel project has get native ports that write the same files without PHP, so the common
    install starts no host: `pestphp/pest-plugin`, `php-http/discovery`, `phpstan/extension-installer`,
    `dealerdirect/phpcodesniffer-composer-installer`. A port is used only when its parity test against the plugin
    passes for that plugin version; otherwise the host runs the plugin.
  - A plugin `allow-plugins` denies, or that it does not list, is handled as Composer handles it (skipped, or the
    interactive prompt, or the non-interactive error).
- **Output:** machine formats (`--format=json` of `show`, `outdated`, `audit`, `licenses`), exit codes and the files
  Pun writes match Composer. A run error exits 2 where Composer exits 1 (output.md). The human output is Pun's own,
  with phases and timings (`Resolved 211 packages in 0.14s`), and agent mode is compact as for the other commands.
- **Speed is the point:** parallel HTTP/2 metadata and dist downloads, parallel extraction, Composer's shared cache,
  a no-op `pun install` that only checks its inputs, and classmap scanning with Pun's own lexer.

## 4. Crates

- `pun-packages`: versions and constraints, the lock, the resolver (`resolver/`), downloads through Composer's cache
  and auth (`fetch.rs`), the autoload dump, bin proxies, scripts, the native plugin ports (`plugins.rs`), the store
  (`store.rs`) and the manifest edits of `require` and `remove` (`manifest_edit.rs`). Its resolver and installer tests
  run on recorded metadata (`tests/fixtures`).
- `pun-config`: reading composer.json (`manifest.rs`) and detection on it and composer.lock (`composer.rs`).
- `pun-cli`: the commands (`install.rs`, `update.rs` with `require` and `remove`, `x.rs`, `workspace.rs`,
  `self_update.rs`) and their output.
- The plugin host (`pun-packages/src/plugin_host.rs`, `assets/plugin-host.php`) loads the pinned composer.phar
  (checked against its SHA-256), builds Composer from the project's files with scripts off, and answers JSON requests
  on a pipe: `dispatch` (one priority phase of an event) and `sync` (activate plugins installed or upgraded since).
  Pun sends an event's priority ≥ 0 phase, runs the scripts, then sends the negative phase.

## 5. Build

Each step lands with its tests green and its parity check. Install from a lock comes first: it is what CI and most
teammates run, and it needs no resolver.

1. **Oracle and corpus.** `scripts/oracle.sh` installs the pinned Composer 2.11.2 phar into `.oracle/composer`.
   `scripts/install-parity.sh <project>` runs `composer install` and `pun install` on scratch copies with an empty
   vendor and compares the trees (paths, contents, modes, symlinks). Composer's installer and autoload fixtures and
   composer/semver's tests are exported to JSON.
   *Left:* `.oracle/composer` and `scripts/install-parity.sh`.
2. **Version parsing and constraints.** Done.
3. **composer.json and composer.lock.** Models, `content-hash`, the lock writer, and `pun composer validate`. Done
   when every corpus lock round-trips byte for byte and `validate` matches Composer's messages and exit codes.
   *Left:* `pun composer validate`.
4. **`pun install` from a lock.** Done when install-parity gives identical vendor trees on pinkary,
   laravel-framework, larastan, laravel-pint and pest. Built; shown on pinkary.
5. **`pun composer dump-autoload`.** Done when Composer's autoload fixtures pass and the corpus `vendor/composer/` is
   identical in all modes. Built; shown on pinkary with every flag.
6. **Scripts and events.** Done when pinkary's `post-autoload-dump` runs and `php artisan package:discover` writes the
   same `bootstrap/cache/packages.php` after `pun install` as after `composer install`. Built, `pun composer
   run-script` and `pun composer exec` included.
7. **Plugins.** The host with the script, package and installer events and plugin commands; then the native ports,
   each tested against the real plugin. Done when every corpus project and a plugin fixture set (pest-plugin,
   php-http/discovery, extension-installer, composer/installers, phpcodesniffer-composer-installer, laravel/vet,
   symfony/flex, a custom installer, a plugin command) give the same files as Composer.
   *Left:* the package, installer, pool and download events, custom installer paths, plugin commands, the
   allow-plugins prompt, and the native ports of extension-installer and phpcodesniffer-composer-installer.
8. **Metadata and the solver.** Packagist v2 and Composer repositories with the shared cache and conditional
   requests; the pool, the solver and the problem reports. Done when Composer's installer fixtures pass. The
   `pre-pool-create`, `pre-file-download` and `post-file-download` events reach the plugin host here.
9. **`pun update`, `pun require`, `pun remove`.** Full and partial updates (`--with-dependencies`,
   `--with-all-dependencies`, `--prefer-lowest`, `--minimal-changes`). Done when `pun update` writes the same
   composer.lock as Composer on every corpus project from its composer.json, and on 20 recorded partial updates.
   *Left of 8 and 9:* the other corpus projects, the 20 recorded partial updates, `vcs`, `git` and `artifact`
   repositories, and the root `filter.summary-url`.
10. **The long tail of `pun composer`:** `show`, `outdated`, `why`, `why-not`, `audit`, `licenses`, `check-platform-reqs`,
    `config`, `init`, `create-project`, `global`, `status`, `reinstall`, `bump`, in that order, each with its machine
    format matched. Built: `config`, matching Composer 2.11.2 on the cases of
    `crates/pun-cli/tests/fixtures/config.json`. The rest is not built.
11. **Docs.** A docs section for the package manager (commands, compatibility, plugins, what is not supported).
    *Today:* command pages under docs/content/docs/commands/ and a workspaces guide exist. No benchmark figures are
    published (PLAN.md §2, 2026-11-23).

## 6. Decisions

- Top-level verbs `install`, `update`, `require`, `remove`; `pun composer <command>` for the rest (Pushpak).
- `composer.json` and `composer.lock` are the source of truth, and every Composer feature they carry is supported
  (Pushpak). There is no pun.lock; pun.toml keeps only `[workspace]` and Pun's own settings, after Bun's package.json
  / bunfig.toml split. Pun-only install settings that Composer has no key for go in pun.toml, not `extra.pun`, when
  they are built (§7).
- Follow Composer's solver, not PubGrub, so the lock matches Composer's (assistant's call, AGENTS.md "Match the
  upstream tool").
- The solver runs on the pool after Composer's pool optimisation, without compacting constraints first (a group can
  only be smaller, so a version or two more stays). When that pool has no solution, the solver runs again on the
  whole pool, so the problems name every version that was loaded (assistant's call).
- Metadata files are revalidated on every resolve, as Composer does, although Packagist sends `max-age=900`: a
  release tagged a minute ago must be resolvable (assistant's call).
- Composer plugins must work (Pushpak): every allowed plugin runs through the plugin host on Composer's own classes;
  native ports only speed up the common ones.
- No phast code is copied (Pushpak). phast (github.com/phast-sh/phast) is a speed benchmark and a reference for ideas
  only; its own manifest, lock and autoloader are not what Pun ships. Composer's own files that `vendor/` must contain
  (`ClassLoader.php`, `InstalledVersions.php`) and the phar the host runs keep Composer's MIT notice.
- `@composer` in a script runs the `composer` on the PATH until `pun composer` has the command (assistant's call).
- `pun self-update` replaces a standalone install with the latest release, checked as the installer checks it; a copy
  Composer installed is updated by Composer (Pushpak).
- Composer 2.11.2 is the pinned oracle, and the plugin host runs that same phar; Pun follows Composer's minor releases
  as it follows PHPStan's (Pushpak).
- A fresh clone needs no Composer: the standalone installer (`curl -fsSL https://punphp.com/install | sh`,
  foundations.md §4) gives a global `pun` that runs `pun install` (Pushpak). `pun install` in a project that requires
  `shipfastlabs/pun` rewrites the `vendor/shipfastlabs/pun` shim but not the binary it runs, which the shim keeps beside it; no
  test covers it yet.

## 7. Beyond Composer: what pnpm and Cargo add

Every feature here keeps `vendor/` what Composer writes, and an unused feature changes nothing Pun writes. Settings
that Composer has no key for live in pun.toml (§6), as Bun keeps them in bunfig.toml's `[install]`: the
`extra.pun.*` names below become pun.toml keys when they are built. Each one is built after the step it depends on.

1. **Workspaces (monorepos), as Cargo and pnpm have them.** Built, except where noted.
   - The root `pun.toml` opts in with `[workspace] members`, a list of globs (`["packages/*", "apps/*"]`) (Pushpak).
     Each member is a directory with its own composer.json, which needs a `name`. Pun adds a `path` repository per
     glob and requires each member at `*@dev` in memory; nothing is written to the root composer.json. Not built:
     `pun workspace init`.
   - One composer.lock and one `vendor/` at the root, as Cargo keeps one `Cargo.lock` and one `target/`. Members
     are installed as path packages (symlinked) and every member's `require` joins one resolution, so the whole
     repository gets one version of each package.
   - Every command finds the workspace root by walking up from the current directory, so `pun install` works in any
     member.
   - `-p <member>` (Cargo's flag; pnpm calls it `--filter`) picks members by name or glob: `pun require -p acme/billing
     guzzlehttp/guzzle` edits that member's manifest and updates the root lock; in a member, the member is the default.
     Not built: without a constraint, `require` should use the constraint other members already have for that
     package (it writes the newest `^major.minor` today).
   - Not built: `pun composer run-script --workspace <script>` runs a script in every member that defines it, in
     dependency order, with independent members in parallel. `pun check`, `format`, `lint` and `analyse` take
     `--workspace` too and use each member's own `pun.toml`.
   - Not built: `pun composer validate --workspace` reports what Cargo's `workspace.dependencies` would have
     prevented: two members requiring different constraints for one package, and a member requiring another member at
     a version it does not have.
2. **A shared file store, as pnpm has.** Built (`store.rs`). Each dist archive is extracted once into
   `$PUN_CACHE_DIR/store/v1/`, keyed by its type, URL, reference, checksum and the umask, and `vendor/` gets
   hardlinks to it (pnpm's default; Pushpak), so the second project that needs a package version installs it without
   a download, an extraction or more disk. The bytes and modes are Composer's, so install parity holds.
   - Editing a hardlinked file in `vendor/` in place edits the store and every project's copy; the store notices and
     extracts the package again for later installs. `--link-mode=clone` (FICLONE on btrfs and XFS, `clonefile` on
     APFS, copies elsewhere) and `copy` avoid that, through the flag or `PUN_LINK_MODE` (uv's names).
   - The classes scanned in a stored package are cached (`$PUN_CACHE_DIR/classes/v1/`), so the class map rescans only
     the root's paths, path repositories and source installs.
   - The last generated autoloader of each vendor directory is kept (`$PUN_CACHE_DIR/autoload/v1/`, keyed by the
     options, composer.json, the packages' metadata and paths, and the stamps of the root's autoload paths and of
     every package not from the store), so a warm install or a repeated `dump-autoload` writes it without a scan.
   - Not built: a pun.toml key for the link mode.
3. **`--locked` and `--offline`, as Cargo has them.** Not built. `pun install --locked` fails when there is no
   composer.lock or its `content-hash` does not match composer.json (Composer only warns). It is the default when
   `CI` is set, as pnpm's `--frozen-lockfile` is. `--offline` resolves and installs from the cache only and names the
   first package it cannot find there.
4. **`pun x`, as `pnpm dlx` and `cargo install` have it.** Built, also as `punx`. `pun x laravel/installer new app`
   resolves the package into a cached scratch project (its own composer.json and composer.lock) and runs its binary
   without touching the current project. `pun x pkg:^1.2` (or `pkg@1.2`) pins a version, short aliases name common
   tools (`laravel`, `pint`, `phpstan`, `pest`, ...), and `punx list` and `punx clean [package...]` show and remove
   the cached installs.
5. **A minimum release age, as pnpm has it.** Not built. `extra.pun.minimum-release-age` (minutes) makes the resolver
   ignore versions published more recently, against compromised releases that are pulled within hours. Off by
   default, because with it `pun update` can pick older versions than `composer update`.
6. **Patches, as `pnpm patch` and Cargo's `[patch]` have them.** Not built. `pun patch <package>` copies the package
   to a scratch directory to edit, and `pun patch --commit <dir>` writes a unified diff to `patches/` and registers it
   under `extra.patches` in the format `cweagans/composer-patches` reads. The same patch then applies under Composer
   with that plugin, and Pun applies it natively.

## 8. Open questions (needs Pushpak)

1. **Which §7 features land before 1.0.** Proposed: `--locked` and `--offline` now, and the rest after 1.0
   (workspaces and `pun x` are built).
