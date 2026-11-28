<?php

declare(strict_types=1);

/*
 * Composer 2.11.2's ValidatingArrayLoader (CHECK_ALL, or no checks with "constraints": false) on configs that reach each
 * of its checks, for the port in crates/pun-packages/src/validate/loader.rs. Many of them `composer validate` never
 * reaches for the root package, whose schema check comes first, but a dependency's composer.json gets this far.
 * Writes crates/pun-packages/tests/fixtures/loader.json: [{name, config, constraints, errors, warnings}], `config` being
 * the JSON the loader got decoded from (as `json_decode($json, true)`). A config Composer crashes on is left out.
 *
 *   php scripts/export-loader-cases.php
 */

$root = dirname(__DIR__);
require 'phar://'.$root.'/.oracle/composer/composer.phar/vendor/autoload.php';

use Composer\Package\Loader\ArrayLoader;
use Composer\Package\Loader\InvalidPackageException;
use Composer\Package\Loader\ValidatingArrayLoader;

/** Each case: a name and the config's JSON, merged over a valid name and version unless it sets them. */
$cases = [
    'name-missing' => '{"name": null}',
    'name-not-a-string' => '{"name": 5}',
    'name-blank' => '{"name": "  "}',
    'name-reserved' => '{"name": "con/app"}',
    'name-json' => '{"name": "acme/app.json"}',
    'name-uppercase' => '{"name": "Acme/FooBar"}',
    'name-invalid' => '{"name": "acme"}',
    'version-array' => '{"version": ["1.0"]}',
    'version-bool' => '{"version": true}',
    'version-number' => '{"version": 5}',
    'version-invalid' => '{"version": "abc"}',
    'platform-string' => '{"config": {"platform": "abc"}}',
    'platform-values' => '{"config": {"platform": {"php": "abc", "ext-a": true, "ext-b": 5, "ext-c": false, "ext-d": "1.2", "ext-e": [], "ext-f": null, "ext-g": ["x", 5], "ext-h": {"a": "b"}, "ext-i": 1.5}}}',
    'type' => '{"type": "bad type!"}',
    'type-not-a-string' => '{"type": 5}',
    'target-dir-and-extra' => '{"target-dir": 5, "extra": "x"}',
    'extra-empty' => '{"extra": []}',
    'bin-string' => '{"bin": "bin/tool"}',
    'bin-parent' => '{"bin": "../tool"}',
    'bin-unsafe' => '{"bin": "a;b"}',
    'bin-array' => '{"bin": ["ok", "../x", 5, "a|b", ["x"]]}',
    'bin-number' => '{"bin": 5}',
    'scripts-description' => '{"scripts": "x", "description": 5}',
    'description-blank' => '{"description": "  "}',
    'homepage' => '{"homepage": "ftp://example.com"}',
    'homepage-not-a-string' => '{"homepage": 5}',
    'homepage-empty' => '{"homepage": ""}',
    'keywords' => '{"keywords": ["ok", 5, ["x"], "bad!", "ünïcode ok"]}',
    'keywords-string' => '{"keywords": "x"}',
    'time-not-a-string' => '{"time": 5, "license": "Not-A-License"}',
    'time-old' => '{"time": "2001-01-01", "license": "Not-A-License"}',
    'time-old-with-time' => '{"time": "2001-01-01 10:00:00", "license": "Not-A-License"}',
    'time-future-iso' => '{"time": "2999-01-01T00:00:00Z", "license": "Not-A-License"}',
    'time-future-offset' => '{"time": "2999-01-01T10:00:00+02:00", "license": "Not-A-License"}',
    'time-old-offset' => '{"time": "2001-01-01T10:00:00.5-0130", "license": "Not-A-License"}',
    'license-invalid' => '{"license": "Not-A-License"}',
    'license-spaces' => '{"license": " MIT "}',
    'license-list' => '{"license": ["MIT", "Not-A-License", "proprietary"]}',
    'license-expressions' => '{"license": ["MIT OR Apache-2.0", "(MIT AND GPL-2.0-or-later)", "GPL-2.0+ WITH Classpath-exception-2.0", "LicenseRef-mine", "DocumentRef-a:LicenseRef-b", "NONE", "NOASSERTION", "proprietary OR MIT", "MIT OR", "MIT WITH Not-An-Exception", "(MIT"]}',
    'license-object' => '{"license": {"a": "MIT", "b": "Nope"}}',
    'license-number' => '{"license": 5}',
    'license-non-strings' => '{"license": ["MIT", 5, ["x"]]}',
    'authors-string' => '{"authors": "x"}',
    'authors' => '{"authors": [5, {"name": 5, "email": "bad", "homepage": "ftp://x", "role": []}, {}, [], {"name": "A", "email": "a@example.com", "homepage": "https://example.com"}]}',
    'authors-all-dropped' => '{"authors": [{"email": "bad"}]}',
    'support-string' => '{"support": "x"}',
    'support' => '{"support": {"email": 5, "irc": "http://x", "issues": "ftp://x", "rss": "x", "forum": "https://example.com", "chat": 5}}',
    'support-email' => '{"support": {"email": "bad"}}',
    'support-empty' => '{"support": {}}',
    'support-all-dropped' => '{"support": {"irc": "http://x"}}',
    'funding-string' => '{"funding": "x"}',
    'funding' => '{"funding": [5, {"type": 5, "url": "ftp://x"}, {}, ["x"], {"type": "github", "url": "https://example.com"}]}',
    'funding-all-dropped' => '{"funding": [{"url": "ftp://x"}]}',
    'php-ext-string' => '{"php-ext": "x"}',
    'php-ext-wrong-type' => '{"php-ext": {"extension-name": 5}}',
    'php-ext-fields' => '{"type": "php-ext", "php-ext": {"extension-name": 5, "priority": "x", "support-zts": "y", "support-nts": 1, "build-path": 5}}',
    'php-ext-build-path-null' => '{"type": "php-ext-zend", "php-ext": {"build-path": null, "extension-name": "a"}}',
    'php-ext-download-number' => '{"type": "php-ext", "php-ext": {"download-url-method": 5}}',
    'php-ext-download-empty' => '{"type": "php-ext", "php-ext": {"download-url-method": []}}',
    'php-ext-download-list' => '{"type": "php-ext", "php-ext": {"download-url-method": [5, "bad", "pre-packaged-source"]}}',
    'php-ext-download-string' => '{"type": "php-ext", "php-ext": {"download-url-method": "bad"}}',
    'php-ext-os-both' => '{"type": "php-ext", "php-ext": {"os-families": ["linux"], "os-families-exclude": ["bsd"]}}',
    'php-ext-os-string' => '{"type": "php-ext", "php-ext": {"os-families": "x"}}',
    'php-ext-os-empty' => '{"type": "php-ext", "php-ext": {"os-families-exclude": []}}',
    'php-ext-os-list' => '{"type": "php-ext", "php-ext": {"os-families": [5, "bad", "linux"]}}',
    'php-ext-os-all-dropped' => '{"type": "php-ext", "php-ext": {"os-families": ["bad"]}}',
    'php-ext-configure-string' => '{"type": "php-ext", "php-ext": {"configure-options": "x"}}',
    'php-ext-configure' => '{"type": "php-ext", "php-ext": {"configure-options": [5, {}, {"name": 5}, {"name": "x", "needs-value": "y", "description": 5}, {"name": "y", "needs-value": true, "description": "d"}]}}',
    'php-ext-configure-all-dropped' => '{"type": "php-ext", "php-ext": {"configure-options": [{"description": "d"}]}}',
    'php-ext-empty-after' => '{"type": "php-ext", "php-ext": {"priority": "x"}}',
    'links-string' => '{"require": "x"}',
    'links' => '{"name": "acme/app", "require": {"acme/app": "^1", "Acme/Upper": "^1", "lib-icu+x": "^1", "acme/a": 5, "acme/b": "self.version", "acme/c": "nope", "acme/d": "*", "acme/e": ">=1", "acme/f": "dev-main", "acme/g": "1.0.0", "acme/h": "==1.0", "acme/i": ">2 <1", "php": "*", "acme/j": "^1 || >=2", "acme/k": "<0.0.0.0-dev", "acme/l": "1.0.0@dev", "acme/m": "1.0.0 || dev-x"}}',
    'links-no-checks' => '{"require": {"acme/d": "*", "acme/g": "1.0.0", "acme/i": ">2 <1"}}',
    'links-dev-and-conflict' => '{"require-dev": {"acme/d": "*", "acme/g": "1.0.0", "acme/i": ">2 <1"}, "conflict": {"acme/r": "1.0", "acme/s": "1.0"}, "replace": {"acme/r": "1.0", "acme/t": "1.0"}, "provide": {"acme/p": "1.0"}}',
    'links-conflict-not-a-string' => '{"conflict": {"acme/e": 1, "acme/g": "1.0"}, "replace": {"acme/e": "*", "acme/g": "*"}}',
    'links-self-in-replace' => '{"name": "acme/app", "replace": {"ACME/APP": "1.0"}}',
    'suggest' => '{"suggest": {"acme/a": 5, "acme/b": "why"}}',
    'suggest-string' => '{"suggest": "x"}',
    'minimum-stability-number' => '{"minimum-stability": 5}',
    'minimum-stability-RC' => '{"minimum-stability": "RC"}',
    'minimum-stability-rc' => '{"minimum-stability": "rc"}',
    'minimum-stability-Beta' => '{"minimum-stability": "Beta"}',
    'minimum-stability-bad' => '{"minimum-stability": "nope"}',
    'autoload-string' => '{"autoload": "x"}',
    'autoload' => '{"autoload": {"psr-4": {"Acme": "src", "": "lib", "Ok\\\\": "ok"}, "unknown": [], "psr-0": {"": "x"}, "classmap": ["a"]}}',
    'target-dir-psr4' => '{"target-dir": "x", "autoload": {"psr-4": {"Acme\\\\": "src"}}}',
    'source-string' => '{"source": "x"}',
    'source-empty' => '{"source": {}}',
    'source-missing' => '{"source": {"type": "git"}}',
    'source-types' => '{"source": {"type": 5, "url": 5, "reference": []}}',
    'source-dashes' => '{"source": {"type": "git", "url": " -u", "reference": "-r"}}',
    'source-int-reference' => '{"source": {"type": "git", "url": "https://example.com", "reference": 5}}',
    'source-perforce' => '{"source": {"type": "perforce", "url": "rsh:x", "reference": "r"}}',
    'source-perforce-ok' => '{"source": {"type": "perforce", "url": "ssl:host:1666", "reference": "r"}}',
    'source-perforce-bad' => '{"source": {"type": "perforce", "url": "host name", "reference": "r"}}',
    'dist' => '{"dist": {"url": 5, "type": 5, "reference": " -x"}}',
    'dist-missing' => '{"dist": {"reference": "r"}}',
    'include-path' => '{"include-path": ["a", 5, []], "transport-options": "x"}',
    'branch-alias-string' => '{"extra": {"branch-alias": "x"}}',
    'branch-alias' => '{"extra": {"branch-alias": {"dev-main": 5, "dev-a": "2.0", "dev-b": "next-dev", "dev-1.x": "2.0.x-dev", "dev-2.x": "2.1.x-dev", "3.x-dev": "3.1.x-dev", "dev-c": "1.0-dev"}}}',
];

$out = [];
foreach ($cases as $name => $json) {
    $config = json_decode($json, true, flags: JSON_THROW_ON_ERROR);
    if (!array_key_exists('name', $config)) {
        $config = ['name' => 'acme/pkg'] + $config;
    } elseif ($config['name'] === null) {
        unset($config['name']);
    }
    if (!array_key_exists('version', $config)) {
        $config['version'] = '1.0.0';
    }
    foreach ([true, false] as $constraints) {
        if (!$constraints && !str_starts_with($name, 'links-no-checks')) {
            continue;
        }
        $loader = new ValidatingArrayLoader(new ArrayLoader(), true, null, $constraints ? ValidatingArrayLoader::CHECK_ALL : 0);
        $errors = [];
        try {
            $loader->load($config);
        } catch (InvalidPackageException $e) {
            $errors = $e->getErrors();
        } catch (\Throwable $e) {
            fprintf(STDERR, "%s: Composer crashes (%s), left out\n", $name, explode("\n", $e->getMessage())[0]);
            continue;
        }
        $out[] = [
            'name' => $name,
            'config' => json_encode($config, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_THROW_ON_ERROR),
            'constraints' => $constraints,
            'errors' => $errors,
            'warnings' => $loader->getWarnings(),
        ];
    }
}
file_put_contents(
    $root.'/crates/pun-packages/tests/fixtures/loader.json',
    json_encode($out, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_THROW_ON_ERROR)."\n",
);
printf("%d loader cases\n", count($out));
