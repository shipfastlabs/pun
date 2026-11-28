#!/usr/bin/env python3
"""Write crates/pun-cli/tests/fixtures/licenses.json, the manifest of crates/pun-cli/tests/licenses.rs: small projects
with what `composer licenses` 2.11.2 (.oracle/composer, the Composer the plugin host pins) prints and returns for them.

Each case is a set of files and the words after `composer`, run in a fresh temporary directory with a fresh HOME and
COMPOSER_HOME, COMPOSER_ROOT_VERSION=1.0.0 (unless the case's `env` leaves it out with a null) and COLUMNS=500,
without a terminal on stdin. The directory is written `{dir}` in the output. A case records `exit`, `stdout` and
`stderr`, and `message` (the lines of Composer's exception block) when Composer stops with an exception, which Pun
prints as its own run error (exit 2).

usage: scripts/export-licenses-cases.py   (needs php 8.5 and .oracle/composer from scripts/oracle.sh)
"""
import json, os, re, shutil, subprocess, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PHAR = os.path.join(ROOT, '.oracle/composer/composer.phar')
OUT = os.path.join(ROOT, 'crates/pun-cli/tests/fixtures/licenses.json')

SHA = '1234567890abcdef1234567890abcdef12345678'


def j(value):
    return json.dumps(value, indent=4) + '\n'


def package(name, version='1.0.0', **fields):
    normalized = version if version.startswith('dev-') else re.sub(r'^v', '', version) + ('.0' * (3 - version.count('.')))
    p = {'name': name, 'version': version, 'version_normalized': fields.pop('version_normalized', normalized), 'type': 'library'}
    p.update({k.replace('_', '-'): v for k, v in fields.items()})
    return p


def installed(*packages, dev=(), vendor='vendor', missing=()):
    """vendor/composer/installed.json and each package's directory (Composer drops a package whose directory isn't
    there), at its pretty name as Composer's install path has it: `vendor/Acme/Upper`, which a case-sensitive file
    system tells from `vendor/acme/upper`), but those named in `missing`."""
    files = {f'{vendor}/composer/installed.json': j({'packages': list(packages), 'dev': True, 'dev-package-names': list(dev)})}
    for p in packages:
        if p.get('type') != 'metapackage' and p['name'] not in missing:
            files[f'{vendor}/{p["name"]}/.keep'] = ''
    return files


def lock(packages=(), packages_dev=()):
    data = {'content-hash': 'x', 'packages': list(packages)}
    if packages_dev is not None:
        data['packages-dev'] = list(packages_dev)
    return j(data)


ALPHA = package('acme/alpha', '1.2.3', license=['MIT'], homepage='https://alpha.example')
BETA = package('acme/beta', '2.0.0', license=['MIT', 'Apache-2.0'],
               source={'type': 'git', 'url': 'https://github.com/acme/beta.git', 'reference': SHA},
               support={'source': 'https://github.com/acme/beta/tree/2.0.0'})
NONE = package('zeta/none', '0.1.0')
STRING = package('acme/string', '3.1.0', license='BSD-3-Clause')
DEV_GIT = package('acme/dev-git', 'dev-main', license=['GPL-2.0-or-later'],
                  source={'type': 'git', 'url': 'https://example.org/dev-git.git', 'reference': SHA})
DEV_DIST = package('acme/dev-dist', '1.x-dev', version_normalized='1.9999999.9999999.9999999-dev', license=['MIT'],
                   dist={'type': 'zip', 'url': 'https://example.org/dev-dist.zip', 'reference': 'abcdef'})
DEV_SVN = package('acme/dev-svn', 'dev-trunk', license=['MIT'],
                  source={'type': 'svn', 'url': 'https://example.org/svn', 'reference': 'trunk@12'})
ALIASED = package('acme/aliased', 'dev-main', license=['MIT'], extra={'branch-alias': {'dev-main': '2.x-dev'}},
                  source={'type': 'git', 'url': 'https://github.com/acme/aliased.git', 'reference': SHA})
UPPER = package('Acme/Upper', '1.0.0', license=['ISC'], homepage='')
WIDE = package('acme/wide', '1.0.0', license=['Propriétaire', 'Licence-日本'])
EMPTY_LICENSE = package('acme/empty-license', '1.0.0', license=[])
META = package('acme/meta', '1.0.0', type='metapackage', license=['MIT'])

ROOT_JSON = {'name': 'acme/app', 'license': 'MIT'}
EVERYTHING = [BETA, NONE, ALPHA, STRING, DEV_GIT, DEV_DIST, DEV_SVN, ALIASED, UPPER, WIDE, EMPTY_LICENSE, META]


def project(manifest=None, packages=EVERYTHING, **kw):
    return {'composer.json': j(ROOT_JSON if manifest is None else manifest), **installed(*packages, **kw)}


CASES = [
    # The three formats over every kind of package.
    ('text', project(), ['licenses']),
    ('json', project(), ['licenses', '--format=json']),
    ('summary', project(), ['licenses', '--format', 'summary']),
    ('format-short', project(packages=[ALPHA]), ['licenses', '-f', 'json']),
    ('format-invalid', project(packages=[ALPHA]), ['licenses', '--format=xml']),
    # With colour: links and Composer's styles.
    ('text-ansi', project(), ['licenses', '--ansi']),
    ('summary-ansi', project(packages=[ALPHA, BETA]), ['licenses', '-f', 'summary', '--ansi']),
    ('json-ansi', project(packages=[ALPHA]), ['licenses', '-f', 'json', '--ansi']),
    # The root: no name, no license, several, the version as Composer finds it.
    ('root-bare', project(manifest={}, packages=[ALPHA]), ['licenses']),
    ('root-bare-json', project(manifest={}, packages=[ALPHA]), ['licenses', '-f', 'json']),
    ('root-licenses', project(manifest={'name': 'acme/app', 'license': ['MIT', 'GPL-3.0-only'], 'version': '2.0.0'}, packages=[ALPHA]), ['licenses']),
    ('root-version-guessed', project(packages=[ALPHA]), ['licenses'], {'COMPOSER_ROOT_VERSION': None}),
    ('root-version-dev', project(packages=[ALPHA]), ['licenses', '-f', 'json'], {'COMPOSER_ROOT_VERSION': '1.2-dev'}),
    # A root in git: the version git gives, and its commit (the case's `git` makes a repository of its files).
    ('root-version-git', {**project(packages=[ALPHA]), 'git': True}, ['licenses'], {'COMPOSER_ROOT_VERSION': None}),
    # Nothing installed.
    ('empty', {'composer.json': j(ROOT_JSON)}, ['licenses']),
    ('empty-json', {'composer.json': j(ROOT_JSON)}, ['licenses', '-f', 'json']),
    ('empty-summary', {'composer.json': j(ROOT_JSON)}, ['licenses', '-f', 'summary']),
    # A package whose directory is gone isn't installed.
    ('package-directory-missing', project(packages=[ALPHA, BETA], missing=['acme/beta']), ['licenses']),
    # Summary order: by count, ties in the order the packages come.
    ('summary-ties', project(packages=[
        package('a/one', license=['Zlib']), package('b/two', license=['MIT']), package('c/three', license=['Zlib', 'MIT']),
        package('d/four', license=['Apache-2.0']), package('e/five'), package('f/six', license=['Apache-2.0'])]), ['licenses', '-f', 'summary']),
    # --no-dev: the packages the root requires, and theirs, by name, provide or replace.
    ('no-dev', project(manifest={**ROOT_JSON, 'require': {'acme/alpha': '*', 'virtual/thing': '*'}, 'require-dev': {'acme/beta': '*'}}, packages=[
        package('acme/alpha', license=['MIT'], require={'acme/string': '*'}), STRING, BETA,
        package('acme/provider', license=['ISC'], provide={'virtual/thing': '1.0'}), package('acme/unused', license=['MIT'])],
        dev=['acme/beta']), ['licenses', '--no-dev']),
    ('no-dev-json', project(manifest={**ROOT_JSON, 'require': {'acme/alpha': '*'}}, packages=[package('acme/alpha', license=['MIT']), BETA]), ['licenses', '--no-dev', '-f', 'json']),
    # --locked: the lock's packages, with packages-dev unless --no-dev.
    ('locked', {'composer.json': j(ROOT_JSON), 'composer.lock': lock([BETA, ALPHA], [NONE, DEV_GIT]), **installed(STRING)}, ['licenses', '--locked']),
    ('locked-no-dev', {'composer.json': j(ROOT_JSON), 'composer.lock': lock([BETA, ALPHA], [NONE])}, ['licenses', '--locked', '--no-dev']),
    ('locked-json', {'composer.json': j(ROOT_JSON), 'composer.lock': lock([ALIASED], [])}, ['licenses', '--locked', '-f', 'json']),
    ('locked-alias', {'composer.json': j(ROOT_JSON), 'composer.lock': lock([ALIASED, ALPHA], [])}, ['licenses', '--locked']),
    ('locked-alias-summary', {'composer.json': j(ROOT_JSON), 'composer.lock': lock([ALIASED, ALPHA], [])}, ['licenses', '--locked', '-f', 'summary']),
    ('locked-invalid-lock', {'composer.json': j({**ROOT_JSON, 'config': {'allow-plugins': {'acme/x': True}}}), 'composer.lock': '{"packages": [\n'}, ['licenses', '--locked']),
    ('locked-lock-without-packages', {'composer.json': j(ROOT_JSON), 'composer.lock': '{}\n'}, ['licenses', '--locked']),
    ('locked-without-lock', {'composer.json': j(ROOT_JSON)}, ['licenses', '--locked']),
    ('locked-without-packages-dev', {'composer.json': j(ROOT_JSON), 'composer.lock': lock([ALPHA], None)}, ['licenses', '--locked']),
    ('locked-without-packages-dev-no-dev', {'composer.json': j(ROOT_JSON), 'composer.lock': lock([ALPHA], None)}, ['licenses', '--locked', '--no-dev']),
    # Composer's global options.
    ('quiet', project(packages=[ALPHA]), ['licenses', '-q']),
    ('quiet-json', project(packages=[ALPHA]), ['licenses', '--quiet', '-f', 'json']),
    ('verbose', project(packages=[ALPHA]), ['licenses', '-v']),
    ('no-ops', project(packages=[ALPHA]), ['licenses', '--no-plugins', '--no-scripts', '--no-cache', '-f', 'summary']),
    # No composer.json.
    ('no-composer-json', {}, ['licenses']),
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
        if line.startswith('licenses [') or line.startswith('Exception trace:'):
            break
        text = line[2:].rstrip() if line.startswith('  ') else line.rstrip()
        if re.match(r'^In \S+ line \d+:$', text):
            continue
        if text and not re.fullmatch(r'\[[\w\\]+\]', text):
            message.append(text)
    return message


# The commit a case's `git` makes: always the same, so its hash is too (crates/pun-cli/tests/licenses.rs makes it so).
GIT_ENV = {'GIT_AUTHOR_NAME': 'a', 'GIT_AUTHOR_EMAIL': 'a@b', 'GIT_COMMITTER_NAME': 'a', 'GIT_COMMITTER_EMAIL': 'a@b',
           'GIT_AUTHOR_DATE': '2026-01-01T00:00:00Z', 'GIT_COMMITTER_DATE': '2026-01-01T00:00:00Z'}


def git_commit(work):
    env = {'PATH': os.environ['PATH'], 'HOME': work, **GIT_ENV}
    for args in (['init', '-q', '-b', 'main'], ['add', '-A'], ['-c', 'commit.gpgsign=false', 'commit', '-q', '-m', 'one']):
        subprocess.run(['git', *args], cwd=work, env=env, check=True)


def record(name, files, args, extra_env=None):
    home = tempfile.mkdtemp(prefix='pun-licenses-home-')
    work = tempfile.mkdtemp(prefix='pun-licenses-')
    files = dict(files)
    git = files.pop('git', False)
    try:
        for path, content in files.items():
            file = os.path.join(work, path)
            os.makedirs(os.path.dirname(file), exist_ok=True)
            with open(file, 'w') as f:
                f.write(content)
        if git:
            git_commit(work)
        r = composer(work, args, home, extra_env)
        here = os.path.realpath(work)
        case = {'name': name, 'files': files, 'args': args, 'exit': r.returncode,
                'stdout': r.stdout.replace(here, '{dir}').replace(work, '{dir}'),
                'stderr': r.stderr.replace(here, '{dir}').replace(work, '{dir}')}
        if extra_env:
            case['env'] = extra_env
        if git:
            case['git'] = True
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


def main():
    version = subprocess.run(['php', PHAR, '--version'], capture_output=True, text=True).stdout
    assert 'Composer version 2.11.2 ' in version, version
    cases = [record(*c) for c in CASES]
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, 'w') as f:
        json.dump({'cases': cases}, f, indent=1, ensure_ascii=False)
        f.write('\n')
    print(f'{OUT}: {len(cases)} cases')


if __name__ == '__main__':
    main()
