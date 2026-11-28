<?php

declare(strict_types=1);

/*
 * The data `composer validate` 2.11.2 checks a composer.json against, from .oracle/composer/composer.phar:
 * - crates/pun-packages/assets/spdx.json: the SPDX license list (composer/spdx-licenses), as
 *   {licenses: {identifier: deprecated}, exceptions: [identifier]}, in the bundled files' order;
 * - crates/pun-packages/assets/composer-schema.json: Composer's JSON schema (res/composer-schema.json), as shipped.
 *
 *   php scripts/export-composer-data.php
 */

$root = dirname(__DIR__);
$composer = 'phar://'.$root.'/.oracle/composer/composer.phar/';
$phar = $composer.'vendor/composer/spdx-licenses/res/';
$licenses = json_decode(file_get_contents($phar.'spdx-licenses.json'), true, flags: JSON_THROW_ON_ERROR);
$exceptions = json_decode(file_get_contents($phar.'spdx-exceptions.json'), true, flags: JSON_THROW_ON_ERROR);

$out = [
    'licenses' => array_map(static fn (array $license): bool => $license[2], $licenses),
    'exceptions' => array_keys($exceptions),
];
file_put_contents(
    $root.'/crates/pun-packages/assets/spdx.json',
    json_encode($out, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_THROW_ON_ERROR)."\n",
);
printf("%d licenses (%d deprecated), %d exceptions\n", count($licenses), count(array_filter($out['licenses'])), count($exceptions));

$schema = file_get_contents($composer.'res/composer-schema.json');
json_decode($schema, flags: JSON_THROW_ON_ERROR);
file_put_contents($root.'/crates/pun-packages/assets/composer-schema.json', $schema);
printf("composer-schema.json: %d bytes\n", strlen($schema));
