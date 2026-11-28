#!/usr/bin/env python3
"""Write crates/pun-cli/tests/fixtures/check_platform_reqs.json, the manifest of crates/pun-cli/tests/check_platform_reqs.rs:
small projects with what `composer check-platform-reqs` 2.11.2 (.oracle/composer, the Composer the plugin host pins)
prints and returns for them, and the platform it checked them against.

Composer checks the real platform of the PHP it runs on. So the manifest records that platform once, as Pun's own probe
(the PROBE script of crates/pun-packages/src/resolver/platform.rs) run on the same `php`, without its `ini` and
`inputs` (local paths Pun only needs for hints and its cache). The test gives Pun that probe through PHP_BINARY, a
script that prints it, so it checks the same platform on any machine.

Each case is a set of files and the arguments after `check-platform-reqs`, run in a fresh temporary directory with a
fresh HOME and COMPOSER_HOME, COMPOSER_ROOT_VERSION=1.0.0 and COLUMNS=500, as scripts/export-validate-cases.py does; a case's `env` adds to that
environment, and a null in it leaves the variable out (the test gives Pun the same environment). A
case records `exit`, `stdout` and `stderr`, and `message` (the lines of Composer's exception block) when Composer stops
with an exception, which Pun prints as its own run error (exit 2).

usage: scripts/export-platform-reqs-cases.py   (needs php 8.5 and .oracle/composer from scripts/oracle.sh)
"""
import json, os, re, shutil, subprocess, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PHAR = os.path.join(ROOT, '.oracle/composer/composer.phar')
OUT = os.path.join(ROOT, 'crates/pun-cli/tests/fixtures/check_platform_reqs.json')
PLATFORM_RS = os.path.join(ROOT, 'crates/pun-packages/src/resolver/platform.rs')


def j(value):
    return json.dumps(value, indent=4) + '\n'


ROOT_JSON = {'name': 'acme/app', 'require': {'php': '>=8.1'}}


def root(**fields):
    m = dict(ROOT_JSON)
    for k, v in fields.items():
        m[k.replace('_', '-')] = v
    return m


def package(name, **fields):
    p = {'name': name, 'version': '1.0.0', 'version_normalized': '1.0.0.0', 'type': 'library'}
    p.update({k.replace('_', '-'): v for k, v in fields.items()})
    return p


def installed(*packages, dev=(), vendor='vendor'):
    """vendor/composer/installed.json and each package's directory: Composer drops a package whose install path isn't
    there (`Factory::purgePackages`)."""
    files = {f'{vendor}/composer/installed.json': j({'packages': list(packages), 'dev': True, 'dev-package-names': list(dev)})}
    for p in packages:
        if p.get('type') != 'metapackage':
            files[f'{vendor}/{p["name"]}/.keep'] = ''
    return files


def lock(packages=(), packages_dev=()):
    data = {'content-hash': 'x', 'packages': list(packages)}
    if packages_dev is not None:
        data['packages-dev'] = list(packages_dev)
    return j(data)


OK = package('acme/ok', require={'php': '>=8.0', 'ext-json': '*', 'ext-mbstring': '*', 'lib-pcre': '>=8', 'acme/other': '^1.0'})
API = package('acme/api', require={'ext-ctype': '*', 'composer-plugin-api': '^2.0', 'composer-runtime-api': '^2.2', 'composer': '^2.0', 'php-64bit': '*', 'php-ipv6': '*'})
OLD = package('acme/old', require={'php': '<8.0'})
MISSING = package('acme/missing', require={'ext-nope': '*', 'lib-nope': '^1.0', 'php-zts': '*', 'php-debug': '*'})
POLYFILL = package('acme/polyfill', provide={'ext-fake': '1.2.0'})
WANTS_FAKE = package('acme/wants-fake', require={'ext-fake': '^1.0'})
WANTS_FAKE2 = package('acme/wants-fake2', require={'ext-fake': '^2.0'})
REPLACER = package('acme/replacer', replace={'ext-other-fake': '3.0.0'})
WANTS_OTHER = package('acme/wants-other', require={'ext-other-fake': '^3.0'})
MBSTRING_POLYFILL = package('acme/mbstring-polyfill', provide={'ext-mbstring': '*'})
DEV = package('acme/dev', require={'ext-nope': '*'})

ROOT_SELF = {'name': 'acme/app', 'provide': {'ext-fake': 'self.version'}, 'require': {'ext-fake': '^1.0'}}

PLUGIN_PHP = '''<?php

namespace Acme\\Installer;

use Composer\\Composer;
use Composer\\Installer\\LibraryInstaller;
use Composer\\IO\\IOInterface;
use Composer\\Package\\PackageInterface;
use Composer\\Plugin\\PluginInterface;

class Plugin implements PluginInterface
{
    public function activate(Composer $composer, IOInterface $io): void
    {
        $composer->getInstallationManager()->addInstaller(new ModuleInstaller($io, $composer));
    }

    public function deactivate(Composer $composer, IOInterface $io): void {}

    public function uninstall(Composer $composer, IOInterface $io): void {}
}

class ModuleInstaller extends LibraryInstaller
{
    public function supports(string $packageType): bool
    {
        return $packageType === 'acme-module';
    }

    public function getInstallPath(PackageInterface $package): string
    {
        return 'modules/'.explode('/', $package->getName())[1];
    }
}
'''


def plugin_project(module_dir, needs=False, name='acme/installer', **plugin_fields):
    """An installer plugin (acme/installer) that installs `acme-module` packages to modules/<name>, and such a module:
    with `module_dir` in modules/ only, else in vendor/ only, where it would be without the plugin."""
    fields = {'type': 'composer-plugin', 'require': {'composer-plugin-api': '^2.0'},
              'autoload': {'psr-4': {'Acme\\Installer\\': 'src/'}}, 'extra': {'class': 'Acme\\Installer\\Plugin'},
              'install_path': f'../{name}', **plugin_fields}
    plugin = package(name, **fields)
    module = package('acme/module', type='acme-module', require={'php': '<8.0'}, install_path='../../modules/module')
    gone = package('acme/gone', require={'ext-nope': '*'}, install_path='../acme/gone')
    packages = [plugin, module, gone]
    if needs:
        # A package that requires the plugin, for a root that requires it.
        packages.append(package('acme/needs-installer', require={name: '*'}, install_path='../acme/needs-installer'))
    files = {'vendor/composer/installed.json': j({'packages': packages, 'dev': True, 'dev-package-names': []}),
             f'vendor/{name}/src/Plugin.php': PLUGIN_PHP}
    files['modules/module/.keep' if module_dir else 'vendor/acme/module/.keep'] = ''
    if needs:
        files['vendor/acme/needs-installer/.keep'] = ''
    return files


NEW_LOCK = j({'content-hash': 'x', 'packages': [OLD], 'packages-dev': [], 'plugin-api-version': '2.6.0'})

CASES = [
    ('vendor-success', {'composer.json': j(root()), **installed(OK, API)}, []),
    ('vendor-failed', {'composer.json': j(root()), **installed(OLD)}, []),
    ('vendor-missing', {'composer.json': j(root()), **installed(MISSING)}, []),
    ('vendor-all', {'composer.json': j(root()), **installed(OK, OLD, MISSING)}, []),
    ('provided', {'composer.json': j(root()), **installed(POLYFILL, WANTS_FAKE)}, []),
    ('provided-failed', {'composer.json': j(root()), **installed(POLYFILL, WANTS_FAKE, WANTS_FAKE2)}, []),
    ('replaced', {'composer.json': j(root()), **installed(REPLACER, WANTS_OTHER)}, []),
    ('provided-and-real', {'composer.json': j(root()), **installed(MBSTRING_POLYFILL, OK)}, []),
    ('root-only', {'composer.json': j(root(require={'php': '>=8.1', 'ext-json': '*'}, require_dev={'ext-nope': '*'})), **installed(package('acme/plain'))}, []),
    ('root-only-no-dev', {'composer.json': j(root(require={'php': '>=8.1', 'ext-json': '*'}, require_dev={'ext-nope': '*'})), **installed(package('acme/plain'))}, ['--no-dev']),
    ('dev-packages', {'composer.json': j(root()), **installed(OK, DEV, dev=['acme/dev'])}, []),
    ('dev-packages-no-dev', {'composer.json': j(root()), **installed(OK, DEV, dev=['acme/dev'])}, ['--no-dev']),
    ('names-and-case', {'composer.json': j(root(require={'php': '>=8.1', 'ext-PDO': '*', 'ext-zend-opcache': '*', 'ext-Zend-OPcache': '*'})), **installed(package('acme/plain'))}, []),
    ('config-platform-ignored', {'composer.json': j(root(config={'platform': {'php': '7.0.0', 'ext-nope': '1.0'}})), **installed(OK, MISSING)}, []),
    ('vendor-dir-config', {'composer.json': j(root(config={'vendor-dir': 'lib'})), **installed(OLD, vendor='lib')}, []),
    # The lock: with --lock, or when vendor/ has nothing installed.
    ('lock', {'composer.json': j(root()), 'composer.lock': lock([OLD], [DEV]), **installed(OK)}, ['--lock']),
    ('lock-no-dev', {'composer.json': j(root()), 'composer.lock': lock([OLD], [DEV])}, ['--lock', '--no-dev']),
    ('no-vendor', {'composer.json': j(root()), 'composer.lock': lock([OK], [DEV])}, []),
    ('no-vendor-no-dev', {'composer.json': j(root()), 'composer.lock': lock([OK], [DEV])}, ['--no-dev']),
    ('empty-vendor', {'composer.json': j(root()), 'composer.lock': lock([OLD]), **installed()}, []),
    ('no-vendor-no-lock', {'composer.json': j(root())}, []),
    ('lock-without-packages-dev', {'composer.json': j(root()), 'composer.lock': lock([OK], None)}, ['--lock']),
    ('lock-without-packages-dev-no-dev', {'composer.json': j(root()), 'composer.lock': lock([OK], None)}, ['--lock', '--no-dev']),
    # --format=json.
    ('json-success', {'composer.json': j(root()), **installed(OK, API)}, ['--format=json']),
    ('json-all', {'composer.json': j(root()), **installed(OK, OLD, MISSING, POLYFILL, WANTS_FAKE, WANTS_FAKE2)}, ['--format', 'json']),
    ('json-short', {'composer.json': j(root()), **installed(OLD)}, ['-f', 'json']),
    ('json-empty', {'composer.json': j({'name': 'acme/app'}), **installed(package('acme/plain'))}, ['--format=json']),
    ('format-text', {'composer.json': j(root()), **installed(OLD)}, ['--format=text']),
    ('format-invalid', {'composer.json': j(root()), **installed(OLD)}, ['--format=xml']),
    # Branch aliases: the local repository and the lock hold an aliased package and its alias.
    ('alias-provider', {'composer.json': j(root()), **installed(package('acme/poly', version='dev-main', version_normalized='dev-main', extra={'branch-alias': {'dev-main': '2.x-dev'}}, provide={'ext-fake': 'self.version'}), package('acme/wants', require={'ext-fake': '^9.0'}))}, []),
    ('alias-failed-twice', {'composer.json': j(root()), **installed(package('acme/old-dev', version='dev-main', version_normalized='dev-main', default_branch=True, require={'php': '<8.0'}))}, []),
    ('two-providers-fail', {'composer.json': j(root()), **installed(package('acme/p1', provide={'ext-fake': '1.0.0'}), package('acme/p2', replace={'ext-fake': '1.5.0'}), WANTS_FAKE2)}, []),
    ('root-provides', {'composer.json': j(root(provide={'ext-fake': '2.1.0'}, require={'php': '>=8.1', 'ext-fake': '^2.0'})), **installed(package('acme/plain'))}, []),
    ('lock-root-alias', {'composer.json': j(root()), 'composer.lock': j({'content-hash': 'x', 'packages': [package('acme/lib', version='dev-main', version_normalized='dev-main', require={'php': '<8.0'})], 'packages-dev': [], 'aliases': [{'package': 'acme/lib', 'version': 'dev-main', 'alias': '1.0.0', 'alias_normalized': '1.0.0.0'}]})}, ['--lock']),
    ('working-dir', {'sub/composer.json': j(root()), **{'sub/' + k: v for k, v in installed(OLD).items()}}, ['-d', 'sub']),
    ('text-empty', {'composer.json': j({'name': 'acme/app'}), **installed(package('acme/plain'))}, []),
    ('no-dev-lock-fallback-dev-names', {'composer.json': j(root()), 'composer.lock': lock([OK], [DEV]), 'vendor/composer/installed.json': j({'packages': [OK], 'dev': True, 'dev-package-names': ['acme/ok']})}, ['--no-dev']),
    # Files that don't read as Composer expects them.
    ('installed-invalid-json', {'composer.json': j(root()), 'vendor/composer/installed.json': '{"packages": [\n', 'vendor/acme/old/.keep': '', 'composer.lock': lock([OLD], [])}, []),
    ('installed-list', {'composer.json': j(root()), 'vendor/composer/installed.json': j([OLD]), 'vendor/acme/old/.keep': ''}, []),
    ('installed-not-a-list', {'composer.json': j(root()), 'vendor/composer/installed.json': j({'packages': 5}), 'composer.lock': lock([OLD], [])}, []),
    ('lock-invalid-json', {'composer.json': j(root()), 'composer.lock': '{"packages": [\n'}, ['--lock']),
    # What lock-not-an-object gives without Composer's crash: a lock with no packages.
    ('lock-empty', {'composer.json': j(root()), 'composer.lock': lock([], [])}, ['--lock']),
    ('lock-not-an-object', {'composer.json': j(root()), 'composer.lock': '[]\n'}, ['--lock']),
    ('composer-json-array', {'composer.json': '[]\n'}, []),
    ('composer-json-null', {'composer.json': 'null\n'}, []),
    ('composer-json-true', {'composer.json': 'true\n'}, []),
    ('composer-json-integer', {'composer.json': '5\n'}, []),
    ('composer-json-double', {'composer.json': '1.5\n'}, []),
    ('composer-json-string', {'composer.json': '"x"\n'}, []),
    ('installed-packages-object', {'composer.json': j(root()), 'vendor/composer/installed.json': j({'packages': {'old': OLD}, 'dev': True, 'dev-package-names': []}), 'vendor/acme/old/.keep': ''}, []),
    ('composer-json-schema', {'composer.json': j({'name': 'acme/app', 'require': {'php': 5}})}, []),
    ('no-composer-json', {}, []),
    # The root's version, which a `self.version` link of the root's shows: COMPOSER_ROOT_VERSION, else its `version`,
    # else (outside git) 1.0.0+no-version-set.
    ('root-version-guessed', {'composer.json': j(ROOT_SELF), **installed(package('acme/plain'))}, [], {'COMPOSER_ROOT_VERSION': None}),
    ('root-version-manifest', {'composer.json': j({**ROOT_SELF, 'version': '1.4.0'}), **installed(package('acme/plain'))}, [], {'COMPOSER_ROOT_VERSION': None}),
    ('root-version-env', {'composer.json': j({**ROOT_SELF, 'version': '1.4.0'}), **installed(package('acme/plain'))}, [], {'COMPOSER_ROOT_VERSION': '1.2-dev'}),
    # An installer plugin puts its packages elsewhere: Composer loads the plugins before it drops the packages that
    # aren't installed, so the plugin's install path counts, which installed.json records as `install-path`.
    ('installer-plugin', {'composer.json': j(root(config={'allow-plugins': {'acme/installer': True}})), **plugin_project(module_dir=True)}, []),
    ('installer-plugin-module-gone', {'composer.json': j(root(config={'allow-plugins': {'acme/installer': True}})), **plugin_project(module_dir=False)}, []),
    # COMPOSER names the composer file, and its lock follows it.
    ('composer-env', {'custom.json': j(root()), 'custom.lock': lock([OLD], []), 'composer.json': j({'name': 'acme/other'})}, ['--lock'], {'COMPOSER': 'custom.json'}),
    ('composer-env-missing', {'composer.json': j(root())}, [], {'COMPOSER': 'nope.json'}),
    ('invalid-json', {'composer.json': '{"name": "acme/app",}\n'}, []),
    ('working-dir-missing', {'composer.json': j(root())}, ['-d', 'nope']),
    # A package whose directory is gone isn't installed, nor checked.
    ('package-directory-missing', {'composer.json': j(root()), **{k: v for k, v in installed(OK, OLD).items() if k != 'vendor/acme/old/.keep'}}, []),
    ('metapackage', {'composer.json': j(root()), **installed(package('acme/meta', type='metapackage', require={'ext-nope': '*'}))}, []),
    # Composer's global options, after the command's name or before it (`before`). -vv and -vvv add Composer's
    # debug lines (with local paths and times, and a cache collection now and then), which Pun doesn't print.
    ('global-no-interaction', {'composer.json': j(root()), **installed(OLD)}, ['-n']),
    ('global-quiet', {'composer.json': j(root()), **installed(OLD)}, ['-q']),
    ('global-quiet-json', {'composer.json': j(root()), **installed(OLD)}, ['--quiet', '--format=json']),
    ('global-verbose', {'composer.json': j(root()), **installed(OLD)}, ['-v']),
    ('global-ansi', {'composer.json': j(root()), **installed(OK, OLD, MISSING)}, ['--ansi']),
    ('global-ansi-provided', {'composer.json': j(root()), **installed(POLYFILL, WANTS_FAKE, WANTS_FAKE2, OLD)}, ['--ansi']),
    ('global-ansi-no-vendor', {'composer.json': j(root()), 'composer.lock': lock([OK], [DEV])}, ['--ansi']),
    ('global-quiet-error', {'composer.json': j(root()), 'composer.lock': '{"packages": [\n'}, ['--lock', '-q']),
    ('global-ansi-json', {'composer.json': j(root()), **installed(OLD)}, ['--ansi', '--format=json']),
    ('global-no-ansi', {'composer.json': j(root()), **installed(OLD)}, ['--no-ansi']),
    ('global-ansi-and-no-ansi', {'composer.json': j(root()), **installed(OLD)}, ['--no-ansi', '--ansi']),
    ('global-no-scripts', {'composer.json': j(root()), **installed(OLD)}, ['--no-scripts']),
    ('global-no-cache', {'composer.json': j(root()), **installed(OLD)}, ['--no-cache']),
    ('global-before', {'sub/composer.json': j(root()), **{'sub/' + k: v for k, v in installed(OLD).items()}}, ['--no-dev'], None, ['-d', 'sub', '-n']),
    ('global-no-plugins', {'composer.json': j(root(config={'allow-plugins': {'acme/installer': True}})), **plugin_project(module_dir=True)}, ['--no-plugins']),
    # A plugin allow-plugins doesn't allow isn't loaded (and, without interaction, Composer says so).
    ('plugin-not-allowed', {'composer.json': j(root()), **plugin_project(module_dir=True)}, []),
    ('plugin-not-allowed-ansi', {'composer.json': j(root()), **plugin_project(module_dir=True)}, ['--ansi']),
    # A plugin the root requires (through another package) loads only when allowed, and is skipped without a word.
    ('plugin-required-not-allowed', {'composer.json': j(root(require={'php': '>=8.1', 'acme/needs-installer': '*'})), **plugin_project(module_dir=True, needs=True)}, []),
    ('plugin-required-allowed', {'composer.json': j(root(require={'php': '>=8.1', 'acme/needs-installer': '*'}, config={'allow-plugins': {'acme/*': True}})), **plugin_project(module_dir=True, needs=True)}, []),
    ('plugin-allow-all', {'composer.json': j(root(config={'allow-plugins': True})), **plugin_project(module_dir=True)}, []),
    # --lock checks the lock, but Composer still reads the vendor dir and loads its plugins first.
    ('lock-plugin-not-allowed', {'composer.json': j(root()), 'composer.lock': NEW_LOCK, **plugin_project(module_dir=True)}, ['--lock']),
    # Without allow-plugins, Composer reads the lock for its plugin-api-version, before it checks anything.
    ('vendor-lock-invalid-json', {'composer.json': j(root()), 'composer.lock': '{"packages": [\n', **installed(OLD)}, []),
    ('vendor-lock-invalid-json-allow-plugins', {'composer.json': j(root(config={'allow-plugins': {'acme/x': True}})), 'composer.lock': '{"packages": [\n', **installed(OLD)}, []),
    ('vendor-lock-invalid-json-no-plugins', {'composer.json': j(root()), 'composer.lock': '{"packages": [\n', **installed(OLD)}, ['--no-plugins']),
    ('lock-installed-invalid-json', {'composer.json': j(root()), 'composer.lock': lock([OLD], []), 'vendor/composer/installed.json': '{"packages": [\n'}, ['--lock']),
    # A lock from before allow-plugins (plugin-api-version below 2.2.0, or none) and no allow-plugins: Composer stops.
    ('plugin-old-lock', {'composer.json': j(root()), 'composer.lock': lock([OLD], []), **plugin_project(module_dir=True)}, []),
    ('plugin-new-lock', {'composer.json': j(root()), 'composer.lock': NEW_LOCK, **plugin_project(module_dir=True)}, []),
    # What registerPackage checks of a plugin it would load.
    ('plugin-without-api', {'composer.json': j(root(config={'allow-plugins': True})), **plugin_project(module_dir=True, require={})}, []),
    ('plugin-exact-api', {'composer.json': j(root(config={'allow-plugins': True})), **plugin_project(module_dir=True, require={'composer-plugin-api': '2.11.0'})}, []),
    ('plugin-other-api', {'composer.json': j(root(config={'allow-plugins': True})), **plugin_project(module_dir=True, require={'composer-plugin-api': '^1.0'})}, []),
    # A required plugin allow-plugins doesn't name, which Composer skips without a word when it is optional.
    ('plugin-optional', {'composer.json': j(root(require={'php': '>=8.1', 'acme/needs-installer': '*'})), **plugin_project(module_dir=True, needs=True, extra={'class': 'Acme\\Installer\\Plugin', 'plugin-optional': True})}, []),
    ('plugin-package-versions-deprecated', {'composer.json': j(root(require={'php': '>=8.1', 'acme/needs-installer': '*'})), **plugin_project(module_dir=True, needs=True, name='composer/package-versions-deprecated')}, []),
    ('plugin-denied', {'composer.json': j(root(config={'allow-plugins': {'acme/installer': False}})), **plugin_project(module_dir=True)}, []),
    ('global-no-plugins-vendor', {'composer.json': j(root(config={'allow-plugins': {'acme/installer': True}})), **plugin_project(module_dir=False)}, ['--no-plugins']),
]


def probe_script():
    source = open(PLATFORM_RS).read()
    return re.search(r'const PROBE: &str = r#"(.*?)"#;', source, re.S).group(1)


def composer(cwd, args, home, extra_env=None):
    env = {'PATH': os.environ['PATH'], 'HOME': home, 'COMPOSER_HOME': os.path.join(home, '.composer'),
           'COMPOSER_CACHE_DIR': os.path.join(home, '.cache'), 'COMPOSER_ROOT_VERSION': '1.0.0', 'COLUMNS': '500',
           **(extra_env or {})}
    env = {k: v for k, v in env.items() if v is not None}
    return subprocess.run(['php', PHAR, '--no-interaction', *args], cwd=cwd, env=env, capture_output=True, text=True)


def write(dir, files):
    for path, content in files.items():
        full = os.path.join(dir, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, 'w') as f:
            f.write(content)


def exception_message(stderr):
    """The lines of Symfony's exception block ("In <file> line <n>:", then the padded message), or None."""
    lines = stderr.split('\n')
    start = next((i for i, l in enumerate(lines) if re.match(r'^In \S+ line \d+:$', l)), None)
    if start is None:
        return None
    message = []
    for line in lines[start + 1:]:
        if line.startswith('check-platform-reqs [') or line.startswith('Exception trace:'):
            break
        text = line[2:].rstrip() if line.startswith('  ') else line.rstrip()
        if re.match(r'^In \S+ line \d+:$', text):
            continue
        if text and not re.fullmatch(r'\[[\w\\]+\]', text):
            message.append(text)
    return message


def record(name, files, args, extra_env=None, before=()):
    home = tempfile.mkdtemp(prefix='pun-platform-home-')
    work = tempfile.mkdtemp(prefix='pun-platform-')
    try:
        write(work, files)
        r = composer(work, [*before, 'check-platform-reqs', *args], home, extra_env)
        stderr = r.stderr.replace(os.path.realpath(work), '{dir}')
        case = {'name': name, 'files': files, 'args': args, 'exit': r.returncode, 'stdout': r.stdout, 'stderr': stderr}
        if extra_env:
            case['env'] = extra_env
        if before:
            case['before'] = list(before)
        message = exception_message(stderr)
        if 'Composer could not find' in stderr:
            # Composer prints this error itself, outside an exception block; it is still a run error.
            message = [line for line in stderr.splitlines() if line]
        if message is not None:
            # The test reads only these lines of a run error; the rest (a LogicException's trace) names local paths.
            case['message'] = message
            case['stderr'] = ''
        return case
    finally:
        shutil.rmtree(home)
        shutil.rmtree(work)


def main():
    version = subprocess.run(['php', PHAR, '--version'], capture_output=True, text=True).stdout
    assert 'Composer version 2.11.2 ' in version, version
    probed = subprocess.run(['php', '-d', 'display_errors=stderr', '-r', probe_script()], capture_output=True, text=True, check=True)
    probe = json.loads(probed.stdout)
    probe['ini'] = []
    probe['inputs'] = []
    cases = [record(*case) for case in CASES]
    # Every platform package Composer sees on this PHP, each version and each lib-* included: the root requires them
    # all, so the table shows what Pun's probe makes of the same PHP.
    home = tempfile.mkdtemp(prefix='pun-platform-home-')
    work = tempfile.mkdtemp(prefix='pun-platform-')
    try:
        write(work, {'composer.json': j({'name': 'acme/app'})})
        shown = json.loads(composer(work, ['show', '--platform', '--format=json'], home).stdout)
    finally:
        shutil.rmtree(home)
        shutil.rmtree(work)
    names = sorted(p['name'] for p in shown['platform'])
    cases.append(record('every-platform-package', {'composer.json': j({'name': 'acme/app', 'require': {n: '*' for n in names}}), **installed(package('acme/plain'))}, []))
    cases.append(record('every-platform-package-json', {'composer.json': j({'name': 'acme/app', 'require': {n: '*' for n in names}}), **installed(package('acme/plain'))}, ['--format=json']))
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, 'w') as f:
        json.dump({'probe': probe, 'cases': cases}, f, indent=1, ensure_ascii=False)
        f.write('\n')
    print(f'{OUT}: PHP {probe["php"]}, {len(cases)} cases')


if __name__ == '__main__':
    main()
