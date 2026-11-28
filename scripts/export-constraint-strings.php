<?php

declare(strict_types=1);

/*
 * `(string) VersionParser::parseConstraints($constraint)` of composer/semver as Composer 2.11.2 bundles it: the form
 * `check-platform-reqs` prints a requirement's constraint in, for crate pun-packages's `constraint::composer_string`.
 * Writes crates/pun-packages/tests/fixtures/constraint-strings.json: {constraint: string}.
 *
 *   php scripts/export-constraint-strings.php
 */

$root = dirname(__DIR__);
require 'phar://'.$root.'/.oracle/composer/composer.phar/vendor/autoload.php';

$constraints = [
    '*', '*@dev', 'x', 'X', '1.0', '1.0.0', 'v1.0.0', '=1.0', '==1.0', '!=1.0', '<>1.0', '<1.0', '<=1.0', '>1.0', '>=1.0',
    '1.0.0-beta', '1.0.0-beta.2', '1.0.0-RC1', '1.0.0-alpha3', '1.0.0-patch1', '1.0.0+build.5', '1.0-dev', '1.0.x-dev',
    'dev-main', 'dev-main#abc123', 'dev-feature/x', '^1.0', '^1.2.3', '^0.1', '^0.0.1', '^0', '^1.0@dev', '^1.0@beta',
    '~1', '~1.2', '~1.2.3', '~1.2.3.4', '~0.3', '1.*', '1.2.*', '1.2.3.*', '0.*', '1 - 2', '1.0 - 2.0.1', '1.0.0 - 2.0',
    '>=1.0 <2.0', '>=1.0,<2.0', '>=1.0, <2.0', '>1.0 <=2.0', '>=1.0 <2.0 || >=2.0 <3.0', '^1.0 || ^2.0', '^1.0 || ^2.0 || ^3.0',
    '^1.0 || ^3.0', '^1.0 | ^2.0', '1.0 || 2.0', '>=8.1', '<8.0', '^8.1', '>=7.4 <8.4', '*  ||  ^1.0', '^1.0 || *',
    '>=1.0 <2.0 || >=3.0 <4.0', '~1.0 || ~2.0', '1.0.0 as 2.0.0', '^1.0 as 1.5', 'dev-main as 1.0.x-dev', '>=1.0@dev',
    '1.0.0@stable', '~1.2@RC', '  ^1.0  ', '^1.0,^1.1', '>=0.0.0', '<0.0.0', '9999999-dev', 'dev-1.x', '1.x',
];

$parser = new Composer\Semver\VersionParser();
$out = [];
foreach ($constraints as $constraint) {
    $out[$constraint] = (string) $parser->parseConstraints($constraint);
}
file_put_contents(
    $root.'/crates/pun-packages/tests/fixtures/constraint-strings.json',
    json_encode($out, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE | JSON_THROW_ON_ERROR)."\n",
);
printf("%d constraints\n", count($out));
