#!/usr/bin/env python3
"""Write crates/pun-cli/tests/fixtures/config.json, the manifest of crates/pun-cli/tests/config.rs: small projects with
what `composer config` 2.11.2 (.oracle/composer, the Composer the plugin host pins) prints and returns for them, and the
files it leaves behind.

Each case is a set of files in the project (`files`) and in COMPOSER_HOME (`home_files`), and the words after
`composer`, run in a fresh temporary directory with a fresh HOME and COMPOSER_HOME, COMPOSER_ROOT_VERSION=1.0.0 and
COLUMNS=500 (unless the case's `env` says otherwise; a null leaves a variable out), without a terminal on stdin. The
project's directory is written `{dir}` and COMPOSER_HOME `{home}` in the output and the files. A case records `exit`,
`stdout` and `stderr`, `after` and `home_after` (every file of the project and of COMPOSER_HOME afterwards, byte for
byte), and `message` (the lines of Composer's exception block) when Composer stops with an exception, which Pun prints
as its own run error (exit 2).

usage: scripts/export-config-cases.py   (needs php 8.5 and .oracle/composer from scripts/oracle.sh)
"""
import json, os, re, shutil, subprocess, tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PHAR = os.path.join(ROOT, '.oracle/composer/composer.phar')
OUT = os.path.join(ROOT, 'crates/pun-cli/tests/fixtures/config.json')


def j(value, indent=4):
    return json.dumps(value, indent=indent) + '\n'


ROOT_JSON = {'name': 'acme/app', 'description': 'The application', 'require': {'php': '>=8.1'},
             'config': {'sort-packages': True, 'vendor-dir': 'lib/vendor', 'allow-plugins': {'acme/plugin': True}},
             'extra': {'laravel': {'dont-discover': []}}}
PROJECT = {'composer.json': j(ROOT_JSON)}
TWO_SPACES = {'composer.json': j(ROOT_JSON, indent=2)}
TABS = {'composer.json': json.dumps(ROOT_JSON, indent='\t') + '\n'}
NO_NEWLINE = {'composer.json': json.dumps({'name': 'acme/app'}, indent=4)}
BARE = {'composer.json': '{}\n'}
WITH_REPOS = {'composer.json': j({'name': 'acme/app', 'repositories': [{'type': 'vcs', 'url': 'https://example.org/one.git'}]})}
GLOBAL = {'config.json': j({'config': {'process-timeout': 900, 'github-protocols': ['https']}})}

CASES = [
    # Reading a setting: the project's, else the global one, else the default.
    ('get-default', PROJECT, ['config', 'process-timeout']),
    ('get-project', PROJECT, ['config', 'vendor-dir']),
    ('get-absolute', PROJECT, ['config', 'vendor-dir', '--absolute']),
    ('get-bin-dir', PROJECT, ['config', 'bin-dir']),
    ('get-global', PROJECT, ['config', 'process-timeout'], {}, GLOBAL),
    ('get-global-option', PROJECT, ['config', '--global', 'process-timeout'], {}, GLOBAL),
    ('get-boolean', PROJECT, ['config', 'sort-packages']),
    ('get-array', PROJECT, ['config', 'github-protocols'], {}, GLOBAL),
    ('get-object', PROJECT, ['config', 'allow-plugins']),
    ('get-nested', PROJECT, ['config', 'allow-plugins.acme/plugin']),
    ('get-source', PROJECT, ['config', 'vendor-dir', '--source']),
    ('get-extra', PROJECT, ['config', 'extra']),
    ('get-extra-key', PROJECT, ['config', 'extra.laravel']),
    ('get-property', PROJECT, ['config', 'name']),
    ('get-repositories', WITH_REPOS, ['config', 'repositories']),
    ('get-undefined', PROJECT, ['config', 'nope']),
    ('get-undefined-extra', PROJECT, ['config', 'extra.nope']),
    ('get-env', PROJECT, ['config', 'process-timeout'], {'COMPOSER_PROCESS_TIMEOUT': '42'}),
    # The list of every setting.
    ('list', PROJECT, ['config', '--list']),
    ('list-source', PROJECT, ['config', '-l', '--source'], {}, GLOBAL),
    ('list-global', PROJECT, ['config', '--list', '--global'], {}, GLOBAL),
    # Setting one: config keys, with their validators and normalizers.
    ('set-integer', PROJECT, ['config', 'process-timeout', '600']),
    ('set-boolean', PROJECT, ['config', 'secure-http', 'false']),
    ('set-boolean-true', PROJECT, ['config', 'optimize-autoloader', 'true']),
    ('set-string', PROJECT, ['config', 'bin-dir', 'tools']),
    ('set-array', PROJECT, ['config', 'github-protocols', 'https', 'ssh']),
    ('set-allow-plugins', PROJECT, ['config', '--no-plugins', 'allow-plugins.acme/other', 'true']),
    ('set-allow-plugins-false', PROJECT, ['config', 'allow-plugins.acme/plugin', 'false']),
    ('set-preferred-install', PROJECT, ['config', 'preferred-install.acme/*', 'source']),
    ('set-platform', PROJECT, ['config', 'platform.php', '8.2.0']),
    ('set-platform-false', PROJECT, ['config', 'platform.ext-foo', 'false']),
    ('set-sort-packages-two-spaces', TWO_SPACES, ['config', 'process-timeout', '600']),
    ('set-tabs', TABS, ['config', 'process-timeout', '600']),
    ('set-no-newline', NO_NEWLINE, ['config', 'process-timeout', '600']),
    ('set-bare', BARE, ['config', 'process-timeout', '600']),
    ('set-global', PROJECT, ['config', '--global', 'process-timeout', '1200']),
    ('set-global-existing', PROJECT, ['config', '-g', 'secure-http', 'false'], {}, GLOBAL),
    ('set-file', {**PROJECT, 'other.json': j({'name': 'acme/other'})}, ['config', '--file', 'other.json', 'process-timeout', '7']),
    ('set-composer-env', {**PROJECT, 'custom.json': j({'name': 'acme/custom'})}, ['config', 'process-timeout', '8'], {'COMPOSER': 'custom.json'}),
    # Root properties.
    ('set-name', PROJECT, ['config', 'name', 'acme/renamed']),
    ('set-description', PROJECT, ['config', 'description', 'A new description']),
    ('set-minimum-stability', PROJECT, ['config', 'minimum-stability', 'dev']),
    ('set-prefer-stable', PROJECT, ['config', 'prefer-stable', 'true']),
    ('set-keywords', PROJECT, ['config', 'keywords', 'one', 'two']),
    ('set-license', PROJECT, ['config', 'license', 'MIT']),
    ('set-type', PROJECT, ['config', 'type', 'project']),
    ('unset-description', PROJECT, ['config', '--unset', 'description']),
    ('set-property-global', PROJECT, ['config', '--global', 'name', 'acme/x']),
    # extra.
    ('set-extra', PROJECT, ['config', 'extra.foo.bar', 'baz']),
    ('set-extra-json', PROJECT, ['config', '--json', 'extra.list', '[1, "two", {"three": 3}]']),
    ('set-extra-merge', PROJECT, ['config', '--json', '--merge', 'extra.laravel', '{"providers": ["App\\\\Provider"]}']),
    ('set-extra-merge-array', {'composer.json': j({'name': 'acme/app', 'extra': {'list': [1]}})}, ['config', '--json', '--merge', 'extra.list', '[2]']),
    ('set-extra-merge-mixed', {'composer.json': j({'name': 'acme/app', 'extra': {'list': [1]}})}, ['config', '--json', '--merge', 'extra.list', '{"a": 2}']),
    ('set-extra-invalid-json', PROJECT, ['config', '--json', 'extra.list', '[1,']),
    ('unset-extra', PROJECT, ['config', '--unset', 'extra.laravel']),
    # Repositories.
    ('set-repository', PROJECT, ['config', 'repositories.foo', 'vcs', 'https://example.org/foo.git']),
    ('set-repository-append', WITH_REPOS, ['config', '--append', 'repositories.foo', 'vcs', 'https://example.org/foo.git']),
    ('set-repository-prepend', WITH_REPOS, ['config', 'repositories.foo', 'composer', 'https://repo.example.org']),
    ('set-repository-json', PROJECT, ['config', 'repo.foo', '{"type": "path", "url": "../foo"}']),
    ('set-repository-packagist-off', PROJECT, ['config', 'repositories.packagist.org', 'false']),
    ('set-repository-one-value', PROJECT, ['config', 'repositories.foo', 'vcs']),
    ('unset-repository', WITH_REPOS, ['config', '--unset', 'repositories.0']),
    # Auth.
    ('set-github-oauth', PROJECT, ['config', 'github-oauth.github.com', 'ghp_token']),
    ('set-github-oauth-global', PROJECT, ['config', '-g', 'github-oauth.github.com', 'ghp_token']),
    ('set-http-basic', PROJECT, ['config', 'http-basic.repo.example.org', 'user', 'secret']),
    ('set-http-basic-one', PROJECT, ['config', 'http-basic.repo.example.org', 'user']),
    ('set-bearer', PROJECT, ['config', 'bearer.repo.example.org', 'token']),
    ('unset-github-oauth', {**PROJECT, 'auth.json': j({'github-oauth': {'github.com': 'ghp_token'}})}, ['config', '--unset', 'github-oauth.github.com']),
    # Unsetting and errors.
    ('unset', PROJECT, ['config', '--unset', 'vendor-dir']),
    ('unset-missing', PROJECT, ['config', '--unset', 'process-timeout']),
    ('unset-with-value', PROJECT, ['config', '--unset', 'process-timeout', '5']),
    ('set-invalid-integer', PROJECT, ['config', 'process-timeout', 'abc']),
    ('set-invalid-choice', PROJECT, ['config', 'preferred-install', 'sometimes']),
    ('set-too-many', PROJECT, ['config', 'process-timeout', '1', '2']),
    ('set-unknown', PROJECT, ['config', 'nope', 'value']),
    # Policies.
    ('policy-block', PROJECT, ['config', 'policy.advisories.block', 'false']),
    ('policy-audit-invalid', PROJECT, ['config', 'policy.advisories.audit', 'maybe']),
    ('policy-cooldown-period', PROJECT, ['config', 'policy.cooldown.period', '7 days']),
    ('policy-cooldown-period-seconds', PROJECT, ['config', 'policy.cooldown.period', '3600']),
    ('policy-cooldown-period-null', PROJECT, ['config', 'policy.cooldown.period', 'null']),
    ('policy-cooldown-period-invalid', PROJECT, ['config', 'policy.cooldown.period', 'soon']),
    ('policy-ignore', PROJECT, ['config', 'policy.advisories.ignore', 'PKSA-1', 'PKSA-2']),
    ('policy-ignore-json-merge', {'composer.json': j({'name': 'acme/app', 'config': {'policy': {'abandoned': {'ignore': ['a/b']}}}})}, ['config', '--json', '--merge', 'policy.abandoned.ignore', '["c/d"]']),
    ('policy-ignore-merge-mixed', {'composer.json': j({'name': 'acme/app', 'config': {'policy': {'abandoned': {'ignore': ['a/b']}}}})}, ['config', '--json', '--merge', 'policy.abandoned.ignore', '{"c/d": "why"}']),
    ('policy-ignore-json-scalar', PROJECT, ['config', '--json', 'policy.abandoned.ignore', '"x"']),
    ('policy-custom-ignore', PROJECT, ['config', 'policy.mine.ignore', 'a/b']),
    ('policy-custom-reserved', PROJECT, ['config', 'policy.ignore-me.ignore', 'a/b']),
    ('policy-unreachable-scopes', PROJECT, ['config', 'policy.ignore-unreachable', 'audit', 'install']),
    ('policy-unreachable-invalid', PROJECT, ['config', 'policy.ignore-unreachable', 'audit', 'nope']),
    ('policy-unreachable-json', PROJECT, ['config', '--json', 'policy.ignore-unreachable', '["update"]']),
    ('policy-unreachable-json-invalid', PROJECT, ['config', '--json', 'policy.ignore-unreachable', '["nope"]']),
    ('policy-unreachable-json-scalar', PROJECT, ['config', '--json', 'policy.ignore-unreachable', 'true']),
    ('policy-unreachable-boolean', PROJECT, ['config', 'policy.ignore-unreachable', 'true']),
    ('policy-list', PROJECT, ['config', 'policy.mine', 'true']),
    ('policy-list-invalid', PROJECT, ['config', 'policy.mine', 'maybe']),
    ('policy-list-reserved', PROJECT, ['config', 'policy.packages', 'true']),
    ('policy-custom-block', PROJECT, ['config', 'policy.mine.block', 'true']),
    ('policy-custom-block-invalid', PROJECT, ['config', 'policy.mine.block', 'x']),
    ('policy-custom-scope', PROJECT, ['config', 'policy.mine.block-scope', 'update']),
    ('policy-custom-scope-invalid', PROJECT, ['config', 'policy.mine.block-scope', 'x']),
    ('policy-custom-audit', PROJECT, ['config', 'policy.mine.audit', 'report']),
    ('policy-custom-audit-invalid', PROJECT, ['config', 'policy.mine.audit', 'x']),
    ('policy-custom-block-reserved', PROJECT, ['config', 'policy.security.block', 'true']),
    ('policy-sources', PROJECT, ['config', 'policy.mine.sources', 'x']),
    ('policy-unset', {'composer.json': j({'name': 'acme/app', 'config': {'policy': {'abandoned': {'ignore': ['a/b'], 'block': True}}}})}, ['config', '--unset', 'policy.abandoned.ignore']),
    ('policy-unset-last', {'composer.json': j({'name': 'acme/app', 'config': {'policy': {'abandoned': {'block': True}}}})}, ['config', '--unset', 'policy.abandoned.block']),
    ('policy-unset-missing', PROJECT, ['config', '--unset', 'policy.abandoned.block']),
    ('policy-get', {'composer.json': j({'name': 'acme/app', 'config': {'policy': {'abandoned': {'block': True}}}})}, ['config', 'policy']),
    # More settings and their validators.
    ('set-size', PROJECT, ['config', 'cache-files-maxsize', '1GiB']),
    ('set-size-invalid', PROJECT, ['config', 'cache-files-maxsize', 'lots']),
    ('set-store-auths', PROJECT, ['config', 'store-auths', 'prompt']),
    ('set-discard-changes', PROJECT, ['config', 'discard-changes', 'stash']),
    ('set-bump', PROJECT, ['config', 'bump-after-update', 'dev']),
    ('set-platform-check', PROJECT, ['config', 'platform-check', 'false']),
    ('set-suffix-null', PROJECT, ['config', 'autoloader-suffix', 'null']),
    # -d, before or after the command's name: Composer reads and writes the composer.json there, and resolves paths there.
    ('working-dir-get', {'sub/composer.json': j({'name': 'acme/sub'})}, ['-d', 'sub', 'config', 'name']),
    ('working-dir-set-after', {'sub/composer.json': j({'name': 'acme/sub'})}, ['config', '-d', 'sub', 'description', 'Hi']),
    ('working-dir-absolute', {'sub/composer.json': j({'name': 'acme/sub'})}, ['--working-dir=sub', 'config', '--absolute', 'vendor-dir']),
    ('set-cafile', {**PROJECT, 'ca.pem': 'x\n'}, ['config', 'cafile', 'ca.pem']),
    ('set-cafile-missing', PROJECT, ['config', 'cafile', 'nope.pem']),
    ('set-capath', {**PROJECT, 'certs/.keep': ''}, ['config', 'capath', 'certs']),
    ('set-capath-null', PROJECT, ['config', 'capath', 'null']),
    ('set-protocols-invalid', PROJECT, ['config', 'github-protocols', 'https', 'ftp']),
    ('set-domains', PROJECT, ['config', 'gitlab-domains', 'gitlab.com', 'git.example.org']),
    ('set-severities', PROJECT, ['config', 'audit.ignore-severity', 'low']),
    ('set-severities-invalid', PROJECT, ['config', 'audit.ignore-severity', 'tiny']),
    ('set-audit-abandoned', PROJECT, ['config', 'audit.abandoned', 'report']),
    ('set-audit-ignore', PROJECT, ['config', 'audit.ignore', 'CVE-1', 'CVE-2']),
    ('set-audit-ignore-json-merge', {'composer.json': j({'name': 'acme/app', 'config': {'audit': {'ignore': {'CVE-1': 'why'}}}})}, ['config', '-j', '-m', 'audit.ignore', '{"CVE-2": "because"}']),
    ('unset-audit-ignore', {'composer.json': j({'name': 'acme/app', 'config': {'audit': {'ignore': ['CVE-1']}}})}, ['config', '--unset', 'audit.ignore']),
    ('unset-audit', {'composer.json': j({'name': 'acme/app', 'config': {'audit': {'abandoned': 'report'}}})}, ['config', '--unset', 'audit']),
    ('set-disable-tls', PROJECT, ['config', 'disable-tls', 'true']),
    ('set-disable-tls-off', {'composer.json': j({'name': 'acme/app', 'config': {'disable-tls': True}})}, ['config', 'disable-tls', 'false']),
    ('unset-disable-tls', {'composer.json': j({'name': 'acme/app', 'config': {'disable-tls': True}})}, ['config', '--unset', 'disable-tls']),
    ('set-stability-invalid', PROJECT, ['config', 'minimum-stability', 'unstable']),
    ('set-stability-rc', PROJECT, ['config', 'minimum-stability', 'rc']),
    ('unset-preferred-install', {'composer.json': j({'name': 'acme/app', 'config': {'preferred-install': {'acme/*': 'source'}}})}, ['config', '--unset', 'preferred-install.acme/*']),
    ('set-preferred-install-invalid', PROJECT, ['config', 'preferred-install.acme/*', 'sometimes']),
    ('unset-allow-plugins', PROJECT, ['config', '--unset', 'allow-plugins.acme/plugin']),
    ('set-allow-plugins-invalid', PROJECT, ['config', 'allow-plugins.acme/x', 'maybe']),
    ('unset-platform', {'composer.json': j({'name': 'acme/app', 'config': {'platform': {'php': '8.1.0'}}})}, ['config', '--unset', 'platform.php']),
    ('unset-platform-all', {'composer.json': j({'name': 'acme/app', 'config': {'platform': {'php': '8.1.0'}}})}, ['config', '--unset', 'platform']),
    ('unset-keywords', {'composer.json': j({'name': 'acme/app', 'keywords': ['a']})}, ['config', '--unset', 'keywords']),
    ('set-keywords-global', PROJECT, ['config', '-g', 'keywords', 'a']),
    ('set-extra-global', PROJECT, ['config', '-g', 'extra.a', 'b']),
    # suggest, scripts, and properties without a setting.
    ('set-suggest', PROJECT, ['config', 'suggest.acme/zed', 'For', 'the', 'zed']),
    ('unset-suggest', {'composer.json': j({'name': 'acme/app', 'suggest': {'acme/zed': 'x', 'acme/yak': 'y'}})}, ['config', '--unset', 'suggest.acme/zed']),
    ('unset-suggest-all', {'composer.json': j({'name': 'acme/app', 'suggest': {'acme/zed': 'x'}})}, ['config', '--unset', 'suggest']),
    ('set-script', PROJECT, ['config', 'scripts.test', 'phpunit']),
    ('set-script-list', PROJECT, ['config', 'scripts.check', '@test', '@lint']),
    ('unset-script', {'composer.json': j({'name': 'acme/app', 'scripts': {'test': 'phpunit', 'lint': 'pint'}})}, ['config', '--unset', 'scripts.test']),
    ('unset-property', {'composer.json': j({'name': 'acme/app', 'homepage': 'https://x.example'})}, ['config', '--unset', 'homepage']),
    ('get-default-property', PROJECT, ['config', 'minimum-stability']),
    ('get-default-property-array', PROJECT, ['config', 'keywords', '--source']),
    ('get-empty-object-setting', PROJECT, ['config', 'platform']),
    ('get-dotted-unknown', PROJECT, ['config', 'cache.nope']),
    ('get-repository-missing', PROJECT, ['config', 'repositories.nope']),
    ('get-repository', WITH_REPOS, ['config', 'repositories.0']),
    ('get-suggest-key', {'composer.json': j({'name': 'acme/app', 'suggest': {'acme/zed': 'why'}})}, ['config', 'suggest.acme/zed']),
    # More auth.
    ('set-bitbucket', PROJECT, ['config', 'bitbucket-oauth.bitbucket.org', 'key', 'secret']),
    ('set-bitbucket-one', PROJECT, ['config', 'bitbucket-oauth.bitbucket.org', 'key']),
    ('set-gitlab-token', PROJECT, ['config', 'gitlab-token.gitlab.example.org', 'token']),
    ('set-gitlab-token-pair', PROJECT, ['config', 'gitlab-token.gitlab.example.org', 'user', 'token']),
    ('set-github-oauth-two', PROJECT, ['config', 'github-oauth.github.com', 'a', 'b']),
    ('set-custom-headers', PROJECT, ['config', 'custom-headers.repo.example.org', 'X-One: 1', 'X-Two: 2']),
    ('set-custom-headers-invalid', PROJECT, ['config', 'custom-headers.repo.example.org', 'nonsense']),
    ('set-forgejo', PROJECT, ['config', 'forgejo-token.codeberg.org', 'user', 'token']),
    ('set-forgejo-one', PROJECT, ['config', 'forgejo-token.codeberg.org', 'user']),
    ('set-auth-in-composer-json', {'composer.json': j({'name': 'acme/app', 'config': {'github-oauth': {'github.com': 'old'}}})}, ['config', 'github-oauth.github.com', 'new']),
    # Repositories as an object, true, and unset.
    ('set-repository-object-form', {'composer.json': j({'name': 'acme/app', 'repositories': {'one': {'type': 'vcs', 'url': 'https://example.org/one.git'}, 'packagist.org': False}})}, ['config', 'repositories.two', 'vcs', 'https://example.org/two.git']),
    ('set-repository-true', PROJECT, ['config', 'repositories.foo', 'true']),
    ('unset-repository-named', {'composer.json': j({'name': 'acme/app', 'repositories': [{'name': 'foo', 'type': 'vcs', 'url': 'https://example.org/foo.git'}]})}, ['config', '--unset', 'repositories.foo']),
    ('unset-repository-object-form', {'composer.json': j({'name': 'acme/app', 'repositories': {'foo': {'type': 'vcs', 'url': 'u'}}})}, ['config', '--unset', 'repo.foo']),
    ('unset-repository-disabled', {'composer.json': j({'name': 'acme/app', 'repositories': [{'packagist.org': False}]})}, ['config', '--unset', 'repositories.packagist.org']),
    ('set-repository-disabled-again', {'composer.json': j({'name': 'acme/app', 'repositories': [{'packagist.org': False}]})}, ['config', 'repositories.packagist.org', 'false']),
    ('set-repository-replace', {'composer.json': j({'name': 'acme/app', 'repositories': [{'name': 'foo', 'type': 'vcs', 'url': 'old'}]})}, ['config', 'repositories.foo', 'vcs', 'new']),
    # When the manipulator can't: a node that isn't an object, rewritten whole.
    ('fallback-config', {'composer.json': j({'name': 'acme/app', 'config': []})}, ['config', 'process-timeout', '9']),
    ('fallback-extra', {'composer.json': j({'name': 'acme/app', 'extra': []})}, ['config', 'extra.a.b', 'c']),
    ('fallback-suggest', {'composer.json': j({'name': 'acme/app', 'suggest': []})}, ['config', 'suggest.acme/x', 'why']),
    ('fallback-scripts', {'composer.json': j({'name': 'acme/app', 'scripts': []})}, ['config', 'scripts.test', 'x']),
    ('fallback-policy', {'composer.json': j({'name': 'acme/app', 'config': []})}, ['config', 'policy.abandoned.block', 'true']),
    ('fallback-repositories', {'composer.json': j({'name': 'acme/app', 'repositories': 'nope'})}, ['config', 'repositories.foo', 'vcs', 'u']),
    ('fallback-auth', {**PROJECT, 'auth.json': j({'github-oauth': []})}, ['config', 'github-oauth.github.com', 't']),
    ('fallback-platform', {'composer.json': j({'name': 'acme/app', 'config': []})}, ['config', 'platform.php', '8.1.0']),
    ('fallback-repositories-bool', {'composer.json': j({'name': 'acme/app', 'repositories': True})}, ['config', 'repositories.foo', 'vcs', 'u']),
    ('fallback-repositories-int', {'composer.json': j({'name': 'acme/app', 'repositories': 5})}, ['config', 'repositories.foo', 'vcs', 'u']),
    ('fallback-repositories-float', {'composer.json': j({'name': 'acme/app', 'repositories': 1.5})}, ['config', 'repositories.foo', 'vcs', 'u']),
    ('fallback-unset-extra', {'composer.json': j({'name': 'acme/app', 'extra': ['a', 'b']})}, ['config', '--unset', 'extra.0']),
    ('fallback-unset-extra-object', {'composer.json': j({'name': 'acme/app', 'extra': {'a': ['x', 'y']}})}, ['config', '--unset', 'extra.a.0']),
    # The global config and the environment merged into what Composer reads.
    ('merge-global', PROJECT, ['config', '--list', '--source'], {'COMPOSER_AUTH': '{"github-oauth": {"github.com": "t"}}'},
     {'config.json': j({'config': {'preferred-install': {'acme/*': 'source'}, 'gitlab-domains': ['gitlab.com', 'git.example.org'], 'audit': {'ignore': ['CVE-1']}, 'policy': {'abandoned': {'ignore': ['x/y']}, 'mine': False}, 'allow-plugins': {'z/z': False}, 'github-oauth': {'example.org': 'u'}}, 'repositories': [{'packagist': False}, {'type': 'composer', 'url': 'https://repo.example.org'}]}),
      'auth.json': j({'http-basic': {'repo.example.org': {'username': 'u', 'password': 'p'}}})}),
    ('merge-project-over-global', {'composer.json': j({'name': 'acme/app', 'config': {'preferred-install': 'source', 'audit': {'ignore': ['CVE-2']}, 'policy': {'abandoned': {'ignore': ['a/b']}}}, 'repositories': {'packagist.org': False, 'mine': {'type': 'composer', 'url': 'https://packagist.org'}}})}, ['config', '--list'], {},
     {'config.json': j({'config': {'preferred-install': {'acme/*': 'source'}, 'audit': {'ignore': ['CVE-1']}, 'policy': {'abandoned': {'ignore': ['x/y']}}}})}),
    ('get-env-settings', PROJECT, ['config', '--list'], {'COMPOSER_VENDOR_DIR': 'v', 'COMPOSER_HTACCESS_PROTECT': '0', 'COMPOSER_BIN_COMPAT': 'full', 'COMPOSER_AUDIT_ABANDONED': 'ignore', 'COMPOSER_PROCESS_TIMEOUT': '7'}),
    ('get-home-path', PROJECT, ['config', 'archive-dir', '--absolute']),
    ('get-cache-files-ttl', {'composer.json': j({'name': 'acme/app', 'config': {'cache-files-ttl': 5, 'cache-files-maxsize': '2k', 'secure-http': False}})}, ['config', '--list']),
    ('get-global-base', PROJECT, ['config', '-g', 'vendor-dir', '--absolute'], {}, GLOBAL),
    ('get-env-booleans', PROJECT, ['config', '--list'], {'COMPOSER_DISCARD_CHANGES': 'stash', 'COMPOSER_POLICY': '0', 'COMPOSER_SECURITY_BLOCKING_ABANDONED': 'on', 'COMPOSER_CACHE_READ_ONLY': '1'}),
    ('get-env-policy-on', {'composer.json': j({'name': 'acme/app', 'config': {'policy': False, 'discard-changes': 'stash'}})}, ['config', 'policy'], {'COMPOSER_POLICY': 'true', 'COMPOSER_DISCARD_CHANGES': 'false'}),
    ('get-env-discard-invalid', PROJECT, ['config', '--list'], {'COMPOSER_DISCARD_CHANGES': 'maybe'}),
    ('get-env-bool-invalid', PROJECT, ['config', '--list'], {'COMPOSER_POLICY': 'maybe'}),
    ('get-env-audit-invalid', PROJECT, ['config', '--list'], {'COMPOSER_AUDIT_ABANDONED': 'maybe'}),
    ('get-bin-compat-invalid', PROJECT, ['config', '--list'], {'COMPOSER_BIN_COMPAT': 'weird'}),
    ('get-discard-invalid', {'composer.json': j({'name': 'acme/app', 'config': {'discard-changes': 'maybe'}})}, ['config', '--list']),
    ('get-maxsize-invalid', {'composer.json': j({'name': 'acme/app', 'config': {'cache-files-maxsize': 'lots'}})}, ['config', '--list']),
    ('get-maxsize-g', {'composer.json': j({'name': 'acme/app', 'config': {'cache-files-maxsize': '1.5G'}})}, ['config', 'cache-files-maxsize']),
    ('get-protocols-http', {'composer.json': j({'name': 'acme/app', 'config': {'github-protocols': ['http', 'https']}})}, ['config', '--list']),
    ('get-protocols-git-first', {'composer.json': j({'name': 'acme/app', 'config': {'github-protocols': ['git', 'https']}})}, ['config', 'github-protocols']),
    ('get-home-tilde', {'composer.json': j({'name': 'acme/app', 'config': {'vendor-dir': '~/v', 'archive-dir': '$HOME/a'}})}, ['config', '--list']),
    ('get-tls-disabled', {'composer.json': j({'name': 'acme/app', 'config': {'disable-tls': True}})}, ['config', 'secure-http']),
    ('file-and-global', PROJECT, ['config', '-g', '--file', 'x.json', 'process-timeout']),
    ('no-key', PROJECT, ['config']),
    ('no-composer-json', {}, ['config', 'process-timeout', '600']),
    ('no-composer-json-get', {}, ['config', 'process-timeout']),
]


def composer(cwd, args, home, extra_env=None):
    env = {'PATH': os.environ['PATH'], 'HOME': home, 'COMPOSER_HOME': os.path.join(home, '.composer'),
           'COMPOSER_CACHE_DIR': os.path.join(home, '.cache'), 'COMPOSER_ROOT_VERSION': '1.0.0', 'COLUMNS': '500',
           **(extra_env or {})}
    env = {k: v for k, v in env.items() if v is not None}
    return subprocess.run(['php', PHAR, *args], cwd=cwd, env=env, capture_output=True, text=True, stdin=subprocess.DEVNULL,
                          timeout=60)


def exception_message(stderr):
    """The lines of Composer's exception block (`In X line N:` and the message), or of Symfony's own error, without the
    command's usage."""
    lines = stderr.splitlines()
    start = next((i for i, l in enumerate(lines) if re.match(r'^In \S+ line \d+:$', l)), None)
    if start is None:
        if re.search(r'\n\nconfig \[', stderr):
            return [l.strip() for l in stderr.split('\n\n')[0].splitlines() if l.strip()]
        return None
    message = []
    for line in lines[start + 1:]:
        if line.startswith('config [') or line.startswith('Exception trace:'):
            break
        text = line[2:].rstrip() if line.startswith('  ') else line.rstrip()
        if re.match(r'^In \S+ line \d+:$', text):
            continue
        if text and not re.fullmatch(r'\[[\w\\]+\]', text):
            message.append(text)
    return message


def tree(base, replacements):
    """Every file under `base` (but caches), by its path, with the temporary directories written as placeholders."""
    out = {}
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = sorted(d for d in dirnames if d != 'cache')
        for name in sorted(filenames):
            path = os.path.join(dirpath, name)
            text = open(path, encoding='utf-8').read()
            for old, new in replacements:
                text = text.replace(old, new)
            out[os.path.relpath(path, base)] = text
    return out


def record(name, files, args, extra_env=None, home_files=None):
    home = tempfile.mkdtemp(prefix='pun-config-home-')
    work = tempfile.mkdtemp(prefix='pun-config-')
    composer_home = os.path.join(home, '.composer')
    try:
        for base, contents in ((work, files), (composer_home, home_files or {})):
            for path, content in contents.items():
                file = os.path.join(base, path)
                os.makedirs(os.path.dirname(file), exist_ok=True)
                with open(file, 'w') as f:
                    f.write(content)
        r = composer(work, args, home, extra_env)
        replacements = [(os.path.realpath(composer_home), '{home}'), (composer_home, '{home}'),
                        (os.path.realpath(work), '{dir}'), (work, '{dir}'),
                        (os.path.realpath(home), '{user}'), (home, '{user}')]

        def placeholders(text):
            for old, new in replacements:
                text = text.replace(old, new)
            return text

        case = {'name': name, 'files': files, 'home_files': home_files or {}, 'args': args, 'exit': r.returncode,
                'stdout': placeholders(r.stdout), 'stderr': placeholders(r.stderr),
                'after': tree(work, replacements), 'home_after': tree(composer_home, replacements)}
        if extra_env:
            case['env'] = extra_env
        message = exception_message(case['stderr'])
        if 'Composer could not find' in case['stderr']:
            # Composer prints this error itself, outside an exception block; it is still a run error.
            message = [line for line in case['stderr'].splitlines() if line]
        if message is not None and r.returncode != 0:
            # The test reads only these lines of a run error.
            case['message'] = message
            case['stderr'] = ''
        dumped = json.dumps(case)
        assert '/var/folders' not in dumped and home not in dumped, (name, [l for l in dumped.split('\\n') if '/var/folders' in l][:3])
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
