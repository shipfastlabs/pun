#!/usr/bin/env python3
"""Write crates/pun-cli/tests/fixtures/validate.json, the manifest of crates/pun-cli/tests/validate.rs: small projects
with what `composer validate` 2.11.2 (.oracle/composer, the Composer the plugin host pins) prints and returns for them.

Each case is a set of files and the arguments after `validate`. Composer runs on the system PHP in a fresh temporary
directory, with a fresh HOME and COMPOSER_HOME (no global config), COMPOSER_ROOT_VERSION=1.0.0 (no "could not detect
the root package version" line outside git) and COLUMNS=500 (no wrapped exception blocks). A case records `exit`,
`stdout` and `stderr`; when Composer stops with an exception (exit 1 and its "In <file> line <n>:" block), it also
records `message`, the block's lines, which Pun prints as its own run error (exit 2). When Composer crashes on input it
doesn't expect (a PHP error with its trace), it records `crash`, the error's first line, instead.

Cases marked `lock` get a composer.lock that Composer writes itself (`update --no-install` against inline `package`
repositories, so no network), before the case's `after_lock` edits to composer.json or the lock.

usage: scripts/export-validate-cases.py   (needs php 8.5 and .oracle/composer from scripts/oracle.sh)
"""
import json, os, re, shutil, subprocess, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PHAR = os.path.join(ROOT, '.oracle/composer/composer.phar')
OUT = os.path.join(ROOT, 'crates/pun-cli/tests/fixtures/validate.json')


def j(value):
    return json.dumps(value, indent=4) + '\n'


# A valid base: every check passes, so each case changes only what it tests.
BASE = {'name': 'acme/app', 'description': 'An app', 'license': 'MIT', 'require': {'php': '^8.3'}}


def base(**fields):
    m = dict(BASE)
    for k, v in fields.items():
        k = k.replace('_', '-')
        if v is None:
            m.pop(k, None)
        else:
            m[k] = v
    return m


# Inline repositories for the lock cases: no packagist, one package.
REPOS = [{'packagist.org': False}, {'type': 'package', 'package': [
    {'name': 'acme/lib', 'version': '1.2.0', 'dist': {'type': 'zip', 'url': 'https://example.com/lib.zip'}},
    {'name': 'acme/tool', 'version': '2.0.0', 'dist': {'type': 'zip', 'url': 'https://example.com/tool.zip'}},
]}]
LOCKED = base(require={'php': '^8.3', 'acme/lib': '^1.0'}, repositories=REPOS)

INSTALLED = {'packages': [{'name': 'acme/dep', 'version': '1.0.0', 'version_normalized': '1.0.0.0', 'type': 'library',
                           'install-path': '../acme/dep'}], 'dev': True, 'dev-package-names': []}

CASES = [
    # ValidateCommand: results, exit codes and options.
    ('valid', {'composer.json': j(BASE)}, []),
    ('valid-file-argument', {'sub/composer.json': j(BASE)}, ['sub/composer.json']),
    ('missing-file', {}, []),
    ('missing-file-argument', {'composer.json': j(BASE)}, ['nope.json']),
    ('warnings', {'composer.json': j(base(license=None))}, []),
    ('warnings-strict', {'composer.json': j(base(license=None))}, ['--strict']),
    ('errors', {'composer.json': j(base(replace={'acme/a': '1.0'}, conflict={'acme/a': '<1.0'}))}, []),
    ('errors-and-warnings', {'composer.json': j(base(license=None, replace={'acme/a': '1.0'}, conflict={'acme/a': '<1.0'}))}, []),
    ('errors-and-publish-errors', {'composer.json': j(base(description=None, replace={'acme/a': '1.0'}, conflict={'acme/a': '<1.0'}))}, []),
    ('warnings-no-check-publish', {'composer.json': j(base(description=None, license=None))}, ['--no-check-publish']),
    ('publish-errors', {'composer.json': j(base(description=None))}, []),
    ('publish-errors-no-check-publish', {'composer.json': j(base(description=None))}, ['--no-check-publish']),
    ('publish-errors-and-warnings', {'composer.json': j(base(description=None, license=None))}, []),
    ('version-field', {'composer.json': j(base(version='1.0.0'))}, []),
    ('version-field-no-check-version', {'composer.json': j(base(version='1.0.0'))}, ['--no-check-version']),
    ('constraints', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '*', 'acme/b': '1.2.3', 'acme/c': '>=1.0', 'acme/d': '==2.0.0'}))}, []),
    ('constraints-no-check-all', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '*', 'acme/b': '1.2.3'}))}, ['--no-check-all']),
    ('constraints-strict', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '*'}))}, ['--strict']),
    # Composer stops before its report when composer.json doesn't load.
    ('parse-error-trailing-comma', {'composer.json': '{"name": "acme/app",}\n'}, []),
    ('parse-error-missing-brace', {'composer.json': '{"name": "acme/app"\n'}, []),
    ('parse-error-single-quotes', {'composer.json': "{'name': 'acme/app'}\n"}, []),
    ('parse-error-empty', {'composer.json': ''}, []),
    ('not-an-object', {'composer.json': '[]\n'}, []),
    ('schema-lax-name', {'composer.json': j(base(name='Acme/App'))}, []),
    ('schema-lax-type', {'composer.json': j(base(description=5))}, []),
    ('schema-unknown-property', {'composer.json': j(base(unknown_key=1))}, []),
    ('schema-lax-enum', {'composer.json': j(base(minimum_stability='nightly'))}, []),
    ('schema-lax-email', {'composer.json': j(base(authors=[{'name': 'A', 'email': 'not-an-email'}]))}, []),
    ('schema-lax-several', {'composer.json': j(base(description=5, minimum_stability='nightly', keywords='x'))}, []),
    ('schema-lax-nested', {'composer.json': j(base(autoload={'psr-4': {'Acme\\': 5}}))}, []),
    # The schema's other paths: formats, anyOf/oneOf/not, patternProperties, numbers, empty containers.
    ('schema-uri-format', {'composer.json': j(base(support={'issues': 'not a uri', 'source': 'https://example.com/a{b'}))}, []),
    ('schema-email-format', {'composer.json': j(base(support={'email': 'ünicode@example.com'}, authors=[{'name': 'A', 'email': 'a@example'}]))}, []),
    ('schema-require-empty-array', {'composer.json': j(base(require=[]))}, []),
    ('schema-require-empty-object', {'composer.json': j(base(require={}))}, []),
    ('schema-require-number', {'composer.json': j(base(require={'acme/a': 5}))}, []),
    ('schema-root-number', {'composer.json': '5\n'}, []),
    ('schema-root-null', {'composer.json': 'null\n'}, []),
    ('schema-root-string', {'composer.json': '"x"\n'}, []),
    ('schema-license-number', {'composer.json': j(base(license=5))}, []),
    ('schema-license-mixed-array', {'composer.json': j(base(license=['MIT', 5]))}, []),
    ('schema-policy-reserved', {'composer.json': j(base(config={'policy': {'ignore-x': {}, 'package': {}}}))}, []),
    ('schema-process-timeout', {'composer.json': j(base(config={'process-timeout': 'soon', 'platform': {'php': True}}))}, []),
    ('schema-repositories', {'composer.json': j(base(repositories=[{'type': 'composer'}, {'type': 'vcs', 'url': 5}, {'type': 'nope', 'url': 'x'}, 'not-an-object']))}, []),
    ('schema-repositories-object', {'composer.json': j(base(repositories={'a': {'type': 'path'}, 'packagist.org': False}))}, []),
    ('schema-authors-extra', {'composer.json': j(base(authors=[{'name': 'A', 'age': 3}, 'B']))}, []),
    ('schema-autoload', {'composer.json': j(base(autoload={'psr-4': {'Acme\\': ['src/', 5]}, 'classmap': 'src/', 'unknown': []}))}, []),
    ('schema-scripts', {'composer.json': j(base(scripts={'a': 5, 'b': ['x', 6], 'c': True}))}, []),
    ('schema-bin-and-type', {'composer.json': j(base(bin=5, type=['library']))}, []),
    ('schema-dollar-schema-strict', {'composer.json': j(dict(BASE, **{'$schema': 'https://getcomposer.org/schema.json'}))}, []),
    ('schema-float-and-bool', {'composer.json': j(base(config={'process-timeout': 1.5, 'optimize-autoloader': 'yes', 'sort-packages': 1}))}, []),
    ('schema-funding', {'composer.json': j(base(funding=[{'type': 'github'}, {'url': 'not a uri'}, 5]))}, []),
    ('schema-archive', {'composer.json': j(base(archive={'exclude': 'x', 'name': 5}))}, []),
    ('schema-strict-several', {'composer.json': j({'name': 'acme/app', 'extra-root': 1, 'license': 'MIT', 'other': 2})}, []),
    ('schema-minimum', {'composer.json': j(base(type='php-ext', php_ext={'priority': 5}))}, []),
    ('schema-maximum-numeric-string', {'composer.json': j(base(type='php-ext', php_ext={'priority': '120'}))}, []),
    ('schema-policy-period', {'composer.json': j(base(config={'policy': {'mine': {'cooldown': {'period': -1}}}}))}, []),
    ('schema-null-values', {'composer.json': j(base(description=None, homepage=None, type='php-ext', php_ext={'build-path': 5}) | {'description': None})}, []),
    ('schema-dependency-errors', {'composer.json': j(BASE), 'vendor/composer/installed.json': j(INSTALLED), 'vendor/acme/dep/composer.json': j({'name': 'acme/dep', 'description': 5, 'require': [], 'extra': 'x'})}, ['-A']),
    ('parse-error-unterminated-string', {'composer.json': '{"name": "acme/app}\n'}, []),
    ('parse-error-invalid-escape', {'composer.json': '{"name": "acme\\x/app"}\n'}, []),
    ('parse-error-later-line', {'composer.json': '{\n    "name": "acme/app",\n    "description": "An app"\n    "license": "MIT"\n}\n'}, []),
    ('parse-error-trailing-garbage', {'composer.json': '{"name": "acme/app"} x\n'}, []),
    ('parse-error-bom', {'composer.json': '\ufeff' + j(BASE)}, []),
    ('parse-error-comment', {'composer.json': '{\n    // a comment\n    "name": "acme/app"\n}\n'}, []),
    # ConfigValidator.
    # jsonlint reports the first duplicate it reduces, at the line its value ends on, and misses one whose earlier
    # value is null (it checks with isset()).
    ('duplicate-key-multiline-value', {'composer.json': '{\n"name": "acme/app",\n"description": "d",\n"license": "MIT",\n"extra": {"a": 1},\n"extra": {\n"b": 2\n}\n,"type": "library"\n}\n'}, []),
    ('duplicate-key-split', {'composer.json': '{\n"name": "acme/app",\n"description": "d",\n"license": "MIT",\n"type": "library",\n"type"\n:\n"project"\n}\n'}, []),
    ('duplicate-key-after-null', {'composer.json': '{\n"name": "acme/app",\n"description": "d",\n"license": "MIT",\n"homepage": null,\n"homepage": "https://example.com"\n}\n'}, []),
    ('duplicate-key-inner-first', {'composer.json': '{\n"name": "acme/app",\n"description": "d",\n"license": "MIT",\n"extra": {"a": 1},\n"extra": {\n"b": 1,\n"b": 2\n}\n}\n'}, []),
    # More of jsonlint's errors.
    ('parse-error-array', {'composer.json': '{"name": "acme/app", "keywords": ["a", "b",]}\n'}, []),
    ('parse-error-array-missing-comma', {'composer.json': '{"keywords": ["a" "b"]}\n'}, []),
    ('parse-error-missing-colon', {'composer.json': '{"name" "acme/app"}\n'}, []),
    ('parse-error-block-comment', {'composer.json': '{/* x */"name": "acme/app"}\n'}, []),
    ('parse-error-close-comment', {'composer.json': '{"name": "acme/app" */}\n'}, []),
    ('parse-error-literal', {'composer.json': '{"name": tru}\n'}, []),
    ('parse-error-leading-zero', {'composer.json': '{"a": 01}\n'}, []),
    ('parse-error-trailing-point', {'composer.json': '{"a": 1.}\n'}, []),
    ('parse-error-leading-point', {'composer.json': '{"a": .5}\n'}, []),
    ('parse-error-plus', {'composer.json': '{"a": +1}\n'}, []),
    ('parse-error-exponent', {'composer.json': '{"a": 1e}\n'}, []),
    ('parse-error-control-character', {'composer.json': '{"a": "x\ty"}\n'}, []),
    ('parse-error-short-unicode', {'composer.json': '{"a": "\\u12"}\n'}, []),
    ('parse-error-backslash-end', {'composer.json': '{"a": "x\\"}\n'}, []),
    ('parse-error-at-start', {'composer.json': 'x\n'}, []),
    ('parse-error-second-byte', {'composer.json': '{x}\n'}, []),
    ('parse-error-long-line', {'composer.json': '{"description": "a very long description that goes on and on", "name" "acme/app"}\n'}, []),
    ('parse-error-crlf', {'composer.json': '{\r\n    "name": "acme/app",\r\n    "license" "MIT"\r\n}\r\n'}, []),
    ('parse-error-tabs', {'composer.json': '{\n\t"name": "acme/app"\n\t"license": "MIT"\n}\n'}, []),
    ('parse-error-unicode-context', {'composer.json': '{"description": "ünïcödé text before", "name" "acme/app"}\n'}, []),
    ('parse-error-nested', {'composer.json': '{"require": {"php": "^8.3",}, "name": "acme/app"}\n'}, []),
    ('parse-error-second-value', {'composer.json': '{"name": "acme/app"}\n{}\n'}, []),
    ('parse-error-only-whitespace', {'composer.json': '  \n  \n'}, []),
    ('not-utf8', {'composer.json': {'hex': '7b226e616d65223a202261636d652fff61707022207d0a'}}, []),
    ('parse-error-dependency', {'composer.json': j(BASE), 'vendor/composer/installed.json': j(INSTALLED), 'vendor/acme/dep/composer.json': '{"name": "acme/dep",}\n'}, ['-A']),
    ('parse-error-unterminated-at-end', {'composer.json': '{"name": "'}, []),
    ('lone-surrogate', {'composer.json': '{"name": "acme/app", "description": "\\ud800", "license": "MIT"}\n'}, []),
    ('deep-nesting', {'composer.json': '{"name": "acme/app", "description": "d", "license": "MIT", "extra": ' + '{"a": ' * 140 + '1' + '}' * 140 + '}\n'}, []),
    # PHP's json_decode takes 511 nested arrays and objects and refuses 512, the root object counting.
    ('nesting-511', {'composer.json': '{"name": "acme/app", "description": "d", "license": "MIT", "extra": ' + '[' * 510 + '1' + ']' * 510 + '}\n'}, []),
    ('nesting-512', {'composer.json': '{"name": "acme/app", "description": "d", "license": "MIT", "extra": ' + '[' * 511 + '1' + ']' * 511 + '}\n'}, []),
    ('composer-env', {'custom.json': j(base(license=None))}, [], {'env': {'COMPOSER': 'custom.json'}}),
    ('duplicate-key-nested', {'composer.json': '{\n    "name": "acme/app",\n    "description": "An app",\n    "license": "MIT",\n    "require": {\n        "php": "^8.3",\n        "php": "^8.4"\n    }\n}\n'}, []),
    ('duplicate-key', {'composer.json': '{\n    "name": "acme/app",\n    "description": "An app",\n    "license": "MIT",\n    "license": "MIT"\n}\n'}, []),
    ('license-proprietary', {'composer.json': j(base(license='proprietary'))}, []),
    ('license-array', {'composer.json': j(base(license=['MIT', 'proprietary']))}, []),
    ('license-empty-array', {'composer.json': j(base(license=[]))}, []),
    ('license-gpl-plus', {'composer.json': j(base(license='GPL-2.0+'))}, []),
    ('license-gpl', {'composer.json': j(base(license='LGPL-3.0'))}, []),
    ('license-deprecated-other', {'composer.json': j(base(license='eCos-2.0'))}, []),
    ('license-unknown', {'composer.json': j(base(license='Not-A-License'))}, []),
    ('composer-installer', {'composer.json': j(base(type='composer-installer'))}, []),
    ('require-and-require-dev-one', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '^1.0'}, require_dev={'acme/a': '^1.0'}))}, []),
    ('require-and-require-dev-two', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '^1.0', 'acme/b': '^1.0'}, require_dev={'acme/a': '^1.0', 'acme/b': '^1.0'}))}, []),
    ('provide-and-replace', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '^1.0'}, require_dev={'acme/b': '^1.0'}, provide={'acme/a': '1.0'}, replace={'acme/b': '1.0'}))}, []),
    ('commit-ref', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': 'dev-main#abc123'}))}, []),
    ('scripts-descriptions-and-aliases', {'composer.json': j(base(scripts={'test': 'phpunit'}, scripts_descriptions={'test': 'Tests', 'lint': 'Lint'}, scripts_aliases={'lint': ['l']}))}, []),
    ('autoload-empty-prefix', {'composer.json': j(base(autoload={'psr-0': {'': 'src/'}, 'psr-4': {'': 'src/'}}))}, []),
    # ValidatingArrayLoader.
    ('authors', {'composer.json': j(base(authors=[{'name': 'A', 'email': 'a@example.com', 'homepage': 'ftp://x'}, {'name': 'B', 'homepage': 'https://example.com'}]))}, []),
    ('support', {'composer.json': j(base(support={'irc': 'http://x', 'issues': 'ftp://x', 'source': 'https://example.com', 'chat': 'ftp://y'}))}, []),
    ('php-ext', {'composer.json': j(base(type='php-ext', php_ext={'extension-name': 'acme', 'priority': 'high', 'support-zts': 'yes', 'download-url-method': [], 'os-families': ['windows', 'plan9'], 'configure-options': [{'name': 'with-x', 'needs-value': 'no'}, {'description': 'x'}]}))}, []),
    ('php-ext-wrong-type', {'composer.json': j(base(php_ext={'extension-name': 'acme'}))}, []),
    ('php-ext-os-families-both', {'composer.json': j(base(type='php-ext', php_ext={'os-families': ['linux'], 'os-families-exclude': ['windows']}))}, []),
    ('funding', {'composer.json': j(base(funding=[{'type': 'github', 'url': 'ftp://x'}]))}, []),
    ('homepage', {'composer.json': j(base(homepage='ftp://example.com'))}, []),
    ('keywords', {'composer.json': j(base(keywords=['ok', 'bad!keyword']))}, []),
    ('link-self', {'composer.json': j(base(require={'php': '^8.3', 'acme/app': '^1.0'}))}, []),
    ('link-invalid-name', {'composer.json': j(base(require={'php': '^8.3', 'acme/a b': '^1.0'}))}, []),
    ('link-invalid-constraint', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': 'not a constraint'}))}, []),
    # RootPackageLoader, loading the root before the report: each of these stops the command.
    ('root-name-reserved', {'composer.json': j(base(name='acme/nul'))}, []),
    ('root-name-json', {'composer.json': j(base(name='acme/app.json'))}, []),
    ('root-invalid-version', {'composer.json': j(base(version='not a version'))}, []),
    ('link-invalid-constraint-conflict', {'composer.json': j(base(conflict={'acme/a': 'nope'}))}, []),
    ('link-invalid-constraint-require-dev', {'composer.json': j(base(require_dev={'Acme/A': '>=1 <'}))}, []),
    ('link-self-require-dev', {'composer.json': j(base(require_dev={'acme/app': '^1.0'}))}, []),
    ('link-self-replace', {'composer.json': j(base(replace={'acme/app': '1.0'}))}, []),
    ('link-uppercase-name', {'composer.json': j(base(require={'php': '^8.3', 'Acme/A': '^1.0'}))}, []),
    ('link-reserved-name-in-replace', {'composer.json': j(base(replace={'acme/con': '1.0'}))}, []),
    # RootPackageLoader::extractAliases: each side of "X as Y" in require and require-dev must be an exact version.
    ('alias-target-invalid', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '1.0 as foo'}))}, []),
    ('alias-source-range', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '>=1.0 as 2.0'}))}, []),
    ('alias-source-branch', {'composer.json': j(base(require_dev={'acme/a': 'main as 1.0'}))}, []),
    ('alias-valid', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': 'dev-main as 1.0.x-dev', 'acme/b': '1.0 as 2.0'}))}, []),
    ('alias-in-or', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '^2.0 || 1.0 as 1.5'}))}, []),
    ('alias-with-reference', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': 'dev-main#abc123 as 1.0'}))}, []),
    ('link-impossible', {'composer.json': j(base(require={'php': '^8.3', 'acme/a': '>2.0 <1.0'}))}, []),
    ('link-conflict-replaced', {'composer.json': j(base(replace={'acme/a': '1.0'}, conflict={'acme/a': '<1.0'}))}, []),
    ('autoload-psr4-separator', {'composer.json': j(base(autoload={'psr-4': {'Acme': 'src/'}}))}, []),
    ('target-dir-and-psr4', {'composer.json': j(base(target_dir='Acme', autoload={'psr-4': {'Acme\\': 'src/'}}))}, []),
    ('branch-alias', {'composer.json': j(base(extra={'branch-alias': {'dev-main': '2.0', 'dev-next': 'next-dev', 'dev-old': '1.x-dev'}}))}, []),
    ('bin', {'composer.json': j(base(bin=['bin/tool', '../escape']))}, []),
    ('time', {'composer.json': j(base(time='not a time'))}, []),
    ('config-platform', {'composer.json': j(base(config={'platform': {'php': 'not a version'}}))}, []),
    # The lock.
    ('lock-fresh', {'composer.json': j(LOCKED)}, [], {'lock': True}),
    ('lock-stale', {'composer.json': j(LOCKED)}, [], {'lock': True, 'after_lock': {'composer.json': j(dict(LOCKED, require={'php': '^8.3', 'acme/lib': '^1.0', 'acme/tool': '^2.0'}))}}),
    ('lock-stale-no-check-lock', {'composer.json': j(LOCKED)}, ['--no-check-lock'], {'lock': True, 'after_lock': {'composer.json': j(dict(LOCKED, require={'php': '^8.3', 'acme/lib': '^1.0', 'acme/tool': '^2.0'}))}}),
    ('lock-stale-config-lock-false', {'composer.json': j(LOCKED)}, [], {'lock': True, 'after_lock': {'composer.json': j(dict(LOCKED, require={'php': '^8.3', 'acme/lib': '^1.0', 'acme/tool': '^2.0'}, config={'lock': False}))}}),
    ('lock-stale-config-lock-false-check-lock', {'composer.json': j(LOCKED)}, ['--check-lock'], {'lock': True, 'after_lock': {'composer.json': j(dict(LOCKED, require={'php': '^8.3', 'acme/lib': '^1.0', 'acme/tool': '^2.0'}, config={'lock': False}))}}),
    ('lock-config-false-and-warnings', {'composer.json': j(base(license=None, config={'lock': False})), 'composer.lock': j({'content-hash': 'x', 'packages': [], 'packages-dev': []})}, []),
    ('lock-without-packages-dev', {'composer.json': j(BASE), 'composer.lock': j({'content-hash': 'x', 'packages': []})}, []),
    # An old lock with only `hash`, the md5 of composer.json's text; a lock without `packages` isn't one.
    ('lock-legacy-hash-fresh', {'composer.json': j(BASE), 'composer.lock': j({'hash': __import__('hashlib').md5(j(BASE).encode()).hexdigest(), 'packages': [], 'packages-dev': []})}, []),
    ('lock-legacy-hash-stale', {'composer.json': j(BASE), 'composer.lock': j({'hash': 'abc', 'packages': [], 'packages-dev': []})}, []),
    # Locker reads `packages` and `packages-dev` with isset(): null is absent.
    ('lock-packages-null', {'composer.json': j(BASE), 'composer.lock': j({'content-hash': 'x', 'packages': None})}, []),
    ('lock-packages-dev-null', {'composer.json': j(BASE), 'composer.lock': j({'content-hash': 'x', 'packages': [], 'packages-dev': None})}, []),
    # Factory::getLockFile: a composer file not named *.json has "<file>.lock".
    ('file-without-json-extension', {'config': j(BASE), 'config.lock': j({'content-hash': 'x', 'packages': [], 'packages-dev': []})}, ['config']),
    ('lock-not-object', {'composer.json': j(BASE), 'composer.lock': '[]\n'}, []),
    ('lock-without-packages', {'composer.json': j(BASE), 'composer.lock': j({'content-hash': 'abc'})}, []),
    ('lock-invalid-json', {'composer.json': j(BASE), 'composer.lock': '{"content-hash": \n'}, []),
    ('lock-missing-requirement', {'composer.json': j(LOCKED)}, [], {'lock': True, 'drop_locked': 'acme/lib'}),
    ('lock-and-warnings', {'composer.json': j(LOCKED)}, [], {'lock': True, 'after_lock': {'composer.json': j(dict(LOCKED, require={'php': '^8.3', 'acme/lib': '^1.0', 'acme/tool': '*'}))}}),
    # --with-dependencies: each installed package's composer.json, by its pretty name.
    ('with-dependencies', {'composer.json': j(BASE), 'vendor/composer/installed.json': j(INSTALLED), 'vendor/acme/dep/composer.json': j({'name': 'acme/dep', 'require': {'acme/x': '*'}})}, ['--with-dependencies']),
    # Composer validates vendor/<name>, not the package's install-path, and skips what isn't there.
    ('with-dependencies-vendor-path', {'composer.json': j(BASE), 'vendor/composer/installed.json': j({'packages': [{'name': 'acme/dep', 'version': '1.0.0', 'type': 'library', 'install-path': '../elsewhere/dep'}, {'name': 'acme/meta', 'version': '1.0.0', 'type': 'metapackage'}, {'name': 'acme/gone', 'version': '1.0.0', 'type': 'library'}], 'dev': True, 'dev-package-names': []}), 'vendor/elsewhere/dep/composer.json': j({'name': 'acme/dep', 'description': 'd', 'license': 'MIT', 'version': '1.0.0'}), 'vendor/acme/dep/composer.json': j({'name': 'acme/dep', 'description': 'd', 'license': 'MIT', 'keywords': ['bad!']})}, ['-A']),
    # A dependency isn't loaded as the root is, so its report gets this far with a name the schema refuses.
    ('with-dependencies-uppercase-name', {'composer.json': j(BASE), 'vendor/composer/installed.json': j(INSTALLED), 'vendor/acme/dep/composer.json': j({'name': 'Acme/FooBAR-BazQux', 'description': 'd', 'license': 'MIT'})}, ['-A']),
    # `-d`: Composer changes to the directory, so a dependency's path is absolute and physical.
    ('with-dependencies-working-dir', {'sub/composer.json': j(BASE), 'sub/vendor/composer/installed.json': j(INSTALLED), 'sub/vendor/acme/dep/composer.json': '{\n    "name": "acme/dep",\n    "description": "d",\n    "license": "MIT",\n    "license": "MIT"\n}\n'}, ['-d', 'sub', '-A']),
    ('working-dir-missing', {'composer.json': j(BASE)}, ['-d', 'nope']),
    # The local repository holds an aliased package and its alias, which validate checks once each.
    ('with-dependencies-branch-alias', {'composer.json': j(BASE), 'vendor/composer/installed.json': j({'packages': [{'name': 'acme/dep', 'version': 'dev-master', 'version_normalized': 'dev-master', 'type': 'library', 'extra': {'branch-alias': {'dev-master': '1.x-dev'}}}], 'dev': True, 'dev-package-names': []}), 'vendor/acme/dep/composer.json': j({'name': 'acme/dep', 'description': 'd', 'license': 'MIT', 'keywords': ['bad!']})}, ['-A']),
    ('with-dependencies-default-branch', {'composer.json': j(BASE), 'vendor/composer/installed.json': j({'packages': [{'name': 'acme/dep', 'version': 'dev-main', 'version_normalized': 'dev-main', 'type': 'library', 'default-branch': True}, {'name': 'acme/other', 'version': 'dev-1.x', 'version_normalized': '1.9999999.9999999.9999999-dev', 'type': 'library', 'default-branch': True}], 'dev': True, 'dev-package-names': []}), 'vendor/acme/dep/composer.json': j({'name': 'acme/dep', 'description': 'd', 'license': 'MIT'}), 'vendor/acme/other/composer.json': j({'name': 'acme/other', 'description': 'd', 'license': 'MIT'})}, ['-A']),
    ('with-dependencies-list-duplicate-key', {'composer.json': j(BASE), 'vendor/composer/installed.json': j(INSTALLED), 'vendor/acme/dep/composer.json': '[{"a": 1, "a": 2}]\n'}, ['-A']),
    ('with-dependencies-strict', {'composer.json': j(BASE), 'vendor/composer/installed.json': j(INSTALLED), 'vendor/acme/dep/composer.json': j({'name': 'acme/dep', 'description': 'd', 'license': 'MIT', 'require': {'acme/x': '*'}})}, ['-A', '--strict']),
]


def composer(cwd, args, home, extra_env=None):
    env = {'PATH': os.environ['PATH'], 'HOME': home, 'COMPOSER_HOME': os.path.join(home, '.composer'),
           'COMPOSER_CACHE_DIR': os.path.join(home, '.cache'), 'COMPOSER_ROOT_VERSION': '1.0.0', 'COLUMNS': '500',
           **(extra_env or {})}
    return subprocess.run(['php', PHAR, '--no-interaction', *args], cwd=cwd, env=env, capture_output=True, text=True)


def write(dir, files):
    for path, content in files.items():
        full = os.path.join(dir, path)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        # A file that isn't UTF-8 is given as {"hex": "..."}.
        data = bytes.fromhex(content['hex']) if isinstance(content, dict) else content.encode()
        with open(full, 'wb') as f:
            f.write(data)


def exception_message(stderr):
    """The lines of Symfony's exception block ("In <file> line <n>:", then the padded message), or None."""
    lines = stderr.split('\n')
    start = next((i for i, l in enumerate(lines) if re.match(r'^In \S+ line \d+:$', l)), None)
    if start is None:
        return None
    message = []
    for line in lines[start + 1:]:
        if line.startswith('validate ['):
            break
        # Symfony indents each line by two spaces and pads it on the right to the block's width.
        text = line[2:].rstrip() if line.startswith('  ') else line.rstrip()
        if re.match(r'^In \S+ line \d+:$', text):
            continue
        if text and not re.fullmatch(r'\[[\w\\]+\]', text):
            message.append(text)
    return message


def record(name, files, args, options):
    home = tempfile.mkdtemp(prefix='pun-validate-home-')
    work = tempfile.mkdtemp(prefix='pun-validate-')
    try:
        write(work, files)
        if options.get('lock'):
            r = composer(work, ['update', '--no-install', '--no-plugins', '--no-scripts'], home)
            assert r.returncode == 0, (name, r.stderr)
            files = dict(files)
            lock = json.load(open(os.path.join(work, 'composer.lock')))
            if 'drop_locked' in options:
                lock['packages'] = [p for p in lock['packages'] if p['name'] != options['drop_locked']]
            files['composer.lock'] = json.dumps(lock, indent=4) + '\n'
            files.update(options.get('after_lock', {}))
            write(work, files)
        r = composer(work, ['validate', *args], home, options.get('env'))
        # The project's own directory, which a dependency's path starts with, as `{dir}`.
        here = lambda text: text.replace(os.path.realpath(work), '{dir}')
        case = {'name': name, 'files': files, 'args': args, 'exit': r.returncode, 'stdout': here(r.stdout), 'stderr': here(r.stderr)}
        if options.get('env'):
            case['env'] = options['env']
        if 'Exception trace:' in r.stderr:
            # Composer itself crashed (a PHP TypeError and the like): tests/validate.rs says what Pun does instead.
            # Its first line, without where it was called from, and no trace: those name local paths.
            case['crash'] = exception_message(r.stderr)[0].split(', called in ')[0]
            case['stderr'] = ''
        elif r.returncode == 1 and (message := exception_message(r.stderr)) is not None:
            case['message'] = message
        return case
    finally:
        shutil.rmtree(home)
        shutil.rmtree(work)


def main():
    version = subprocess.run(['php', PHAR, '--version'], capture_output=True, text=True).stdout
    assert 'Composer version 2.11.2 ' in version, version
    out = []
    for case in CASES:
        name, files, args = case[:3]
        out.append(record(name, files, args, case[3] if len(case) > 3 else {}))
    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, 'w') as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
        f.write('\n')
    print(f'{OUT}: {len(out)} cases')


if __name__ == '__main__':
    main()
