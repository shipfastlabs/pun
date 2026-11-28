#!/usr/bin/env python3
"""Write crates/pun-cli/tests/fixtures/suggests.json, the manifest of crates/pun-cli/tests/suggests.rs: small projects
with what `composer suggests` 2.11.2 (.oracle/composer, the Composer the plugin host pins) prints and returns for them,
and the platform it ran on.

Composer leaves out a suggestion the platform already has (`ext-*` and the rest), so the manifest records that
platform once, as Pun's own probe (the PROBE script of crates/pun-packages/src/resolver/platform.rs) run on the same
`php`, without its `ini` and `inputs` (local paths). The test gives Pun that probe through PHP_BINARY, a script that
prints it.

Each case is a set of files and the words after `composer`, run in a fresh temporary directory with a fresh HOME and
COMPOSER_HOME, COMPOSER_ROOT_VERSION=1.0.0 (unless the case's `env` leaves it out with a null) and COLUMNS=500,
without a terminal on stdin. A case records `exit`,
`stdout` and `stderr`, and `message` (the lines of Composer's exception block) when Composer stops with an exception,
which Pun prints as its own run error (exit 2).

usage: scripts/export-suggests-cases.py   (needs php 8.5 and .oracle/composer from scripts/oracle.sh)
"""
import json, os, re, shutil, subprocess, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PHAR = os.path.join(ROOT, '.oracle/composer/composer.phar')
OUT = os.path.join(ROOT, 'crates/pun-cli/tests/fixtures/suggests.json')
PLATFORM_RS = os.path.join(ROOT, 'crates/pun-packages/src/resolver/platform.rs')


def j(value):
    return json.dumps(value, indent=4) + '\n'


def package(name, version='1.0.0', **fields):
    p = {'name': name, 'version': version, 'version_normalized': version + '.0', 'type': 'library'}
    p.update({k.replace('_', '-'): v for k, v in fields.items()})
    return p


def installed(*packages, dev=()):
    """vendor/composer/installed.json and each package's directory (Composer drops a package whose directory isn't
    there), at its pretty name as Composer's install path has it: `vendor/Acme/Upper`, which a case-sensitive file
    system tells from `vendor/acme/upper`)."""
    files = {'vendor/composer/installed.json': j({'packages': list(packages), 'dev': True, 'dev-package-names': list(dev)})}
    for p in packages:
        files[f'vendor/{p["name"]}/.keep'] = ''
    return files


def lock(packages=(), packages_dev=(), **fields):
    data = {'content-hash': 'x', 'packages': list(packages), 'packages-dev': list(packages_dev), **fields}
    return j(data)


# The root requires alpha and beta (dev); alpha requires gamma, which is a transitive dependency.
ALPHA = package('acme/alpha', require={'acme/gamma': '*'}, suggest={
    'acme/zed': 'For the zed feature', 'ext-gd': 'For images (installed here)', 'ext-nope': 'Not installed',
    'acme/beta': 'Installed, so left out', 'acme/alpha': 'Itself', 'virtual/thing': 'Provided by delta',
    'acme/empty': '', 'Acme/Mixed': 'Mixed case target'})
BETA = package('acme/beta', suggest={'acme/zed': 'Beta wants zed too', 'acme/yak': 'Line one\nline two\twith a tab <info>tag</info> and \\ backslash'})
GAMMA = package('acme/gamma', suggest={'acme/transitive': 'Only with --all'})
DELTA = package('acme/delta', provide={'virtual/thing': '1.0'})
UPPER = package('Acme/Upper', suggest={'acme/from-upper': 'Pretty name with capitals'})
ROOT_JSON = {'name': 'acme/app', 'require': {'acme/alpha': '*', 'acme/delta': '*', 'acme/upper': '*'},
             'require-dev': {'acme/beta': '*'}, 'suggest': {'acme/root-wish': 'The root suggests too', 'ext-gd': 'Installed'}}
PACKAGES = [ALPHA, BETA, GAMMA, DELTA, UPPER]


def vendor_project(manifest=None, packages=PACKAGES, **kw):
    return {'composer.json': j(ROOT_JSON if manifest is None else manifest), **installed(*packages, **kw)}


def lock_project(manifest=None, packages=(ALPHA, GAMMA, DELTA, UPPER), packages_dev=(BETA,), **lock_fields):
    return {'composer.json': j(ROOT_JSON if manifest is None else manifest), 'composer.lock': lock(packages, packages_dev, **lock_fields)}


CASES = [
    # From the vendor dir (no lock): by package, the default, with the root's dependencies only.
    ('vendor', vendor_project(), ['suggests']),
    ('vendor-by-package', vendor_project(), ['suggests', '--by-package']),
    ('vendor-by-suggestion', vendor_project(), ['suggests', '--by-suggestion']),
    ('vendor-both', vendor_project(), ['suggests', '--by-suggestion', '--by-package']),
    ('vendor-list', vendor_project(), ['suggests', '--list']),
    ('vendor-list-and-by-package', vendor_project(), ['suggests', '--list', '--by-package']),
    ('vendor-all', vendor_project(), ['suggests', '--all']),
    ('vendor-all-short', vendor_project(), ['suggests', '-a', '--by-suggestion']),
    ('vendor-ansi', vendor_project(), ['suggests', '--by-suggestion', '--by-package', '--ansi']),
    ('vendor-list-ansi', vendor_project(), ['suggests', '--list', '--ansi']),
    ('vendor-filter-missing-ansi', vendor_project(), ['suggests', 'acme/nope', '--ansi']),
    # Without a lock, --no-dev changes nothing.
    ('vendor-no-dev', vendor_project(), ['suggests', '--no-dev']),
    # Named packages: their suggestions only, all of them, by name in any case.
    ('vendor-filter', vendor_project(), ['suggests', 'acme/gamma', 'ACME/BETA']),
    ('vendor-filter-root', vendor_project(), ['suggests', 'acme/app']),
    ('vendor-filter-missing', vendor_project(), ['suggests', 'acme/nope']),
    ('vendor-filter-missing-two', vendor_project(), ['suggests', 'acme/nope', 'acme/alpha', 'acme/other']),
    ('vendor-filter-platform', vendor_project(), ['suggests', 'php']),
    # config.platform counts as installed.
    ('vendor-config-platform', vendor_project(manifest={**ROOT_JSON, 'config': {'platform': {'ext-nope': '1.0'}}}), ['suggests']),
    # Nothing to suggest.
    ('vendor-none', vendor_project(manifest={'name': 'acme/app', 'require': {'acme/delta': '*'}}, packages=[DELTA]), ['suggests']),
    ('vendor-none-list', vendor_project(manifest={'name': 'acme/app'}, packages=[]), ['suggests', '--list']),
    ('no-vendor-no-lock', {'composer.json': j(ROOT_JSON)}, ['suggests']),
    # From the lock: with packages-dev unless --no-dev, and its platform overrides.
    ('lock', lock_project(), ['suggests']),
    ('lock-all', lock_project(), ['suggests', '--all', '--by-suggestion']),
    ('lock-no-dev', lock_project(), ['suggests', '--no-dev']),
    ('lock-no-dev-filter-dev', lock_project(), ['suggests', '--no-dev', 'acme/beta']),
    ('lock-no-dev-filter-both', lock_project(), ['suggests', '--no-dev', 'acme/beta', 'acme/nope', 'acme/alpha']),
    ('lock-no-dev-filter-two-dev', lock_project(packages_dev=(BETA, package('acme/dev2'))), ['suggests', '--no-dev', 'acme/beta', 'acme/dev2']),
    ('lock-platform-overrides', lock_project(**{'platform-overrides': {'ext-nope': '2.0'}}), ['suggests']),
    ('lock-over-vendor', {**lock_project(packages=(GAMMA,), packages_dev=()), **installed(ALPHA)}, ['suggests', '--all']),
    # A lock that isn't JSON (read as the lock, not for allow-plugins, which is set).
    ('lock-invalid-json', {'composer.json': j({**ROOT_JSON, 'config': {'allow-plugins': {'acme/x': True}}}), 'composer.lock': '{"packages": [\n'}, ['suggests']),
    # Composer's warning about a root version it can't find.
    ('root-version-guessed', vendor_project(), ['suggests'], {'COMPOSER_ROOT_VERSION': None}),
    # Composer's global options.
    ('quiet', vendor_project(), ['suggests', '-q']),
    ('quiet-missing', vendor_project(), ['suggests', '-q', 'acme/nope']),
    ('verbose', vendor_project(), ['suggests', '-v', '--list']),
    ('no-ops', vendor_project(), ['suggests', '--no-plugins', '--no-scripts', '--no-cache', '--list']),
    # No composer.json.
    ('no-composer-json', {}, ['suggests']),
]


def composer(cwd, args, home, extra_env=None):
    env = {'PATH': os.environ['PATH'], 'HOME': home, 'COMPOSER_HOME': os.path.join(home, '.composer'),
           'COMPOSER_CACHE_DIR': os.path.join(home, '.cache'), 'COMPOSER_ROOT_VERSION': '1.0.0', 'COLUMNS': '500',
           **(extra_env or {})}
    env = {k: v for k, v in env.items() if v is not None}
    return subprocess.run(['php', PHAR, *args], cwd=cwd, env=env, capture_output=True, text=True, stdin=subprocess.DEVNULL,
                          timeout=60)


def exception_message(stderr):
    """The lines of Composer's exception block (`In X line N:` and the message), without the command's usage."""
    lines = stderr.splitlines()
    start = next((i for i, l in enumerate(lines) if re.match(r'^In \S+ line \d+:$', l)), None)
    if start is None:
        return None
    message = []
    for line in lines[start + 1:]:
        if line.startswith('suggests [') or line.startswith('Exception trace:'):
            break
        text = line[2:].rstrip() if line.startswith('  ') else line.rstrip()
        if re.match(r'^In \S+ line \d+:$', text):
            continue
        if text and not re.fullmatch(r'\[[\w\\]+\]', text):
            message.append(text)
    return message


def record(name, files, args, extra_env=None):
    home = tempfile.mkdtemp(prefix='pun-suggests-home-')
    work = tempfile.mkdtemp(prefix='pun-suggests-')
    try:
        for path, content in files.items():
            file = os.path.join(work, path)
            os.makedirs(os.path.dirname(file), exist_ok=True)
            with open(file, 'w') as f:
                f.write(content)
        r = composer(work, args, home, extra_env)
        here = os.path.realpath(work)
        case = {'name': name, 'files': files, 'args': args, 'exit': r.returncode,
                'stdout': r.stdout.replace(here, '{dir}').replace(work, '{dir}'),
                'stderr': r.stderr.replace(here, '{dir}').replace(work, '{dir}')}
        if extra_env:
            case['env'] = extra_env
        message = exception_message(case['stderr'])
        if 'Composer could not find' in case['stderr']:
            # Composer prints this error itself, outside an exception block; it is still a run error.
            message = [line for line in case['stderr'].splitlines() if line]
        if message is not None:
            # The test reads only these lines of a run error.
            case['message'] = message
            case['stderr'] = ''
        assert '/var/folders' not in json.dumps(case) and home not in json.dumps(case), (name, case)
        return case
    finally:
        shutil.rmtree(home)
        shutil.rmtree(work)


def probe_script():
    source = open(PLATFORM_RS).read()
    return re.search(r'const PROBE: &str = r#"(.*?)"#;', source, re.S).group(1)


def main():
    version = subprocess.run(['php', PHAR, '--version'], capture_output=True, text=True).stdout
    assert 'Composer version 2.11.2 ' in version, version
    probed = subprocess.run(['php', '-d', 'display_errors=stderr', '-r', probe_script()], capture_output=True, text=True, check=True)
    probe = json.loads(probed.stdout)
    probe['ini'] = []
    probe['inputs'] = []
    cases = [record(*c) for c in CASES]
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, 'w') as f:
        json.dump({'probe': probe, 'cases': cases}, f, indent=1, ensure_ascii=False)
        f.write('\n')
    print(f'{OUT}: PHP {probe["php"]}, {len(cases)} cases')


if __name__ == '__main__':
    main()
