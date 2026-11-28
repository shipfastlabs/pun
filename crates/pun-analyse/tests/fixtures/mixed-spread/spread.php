<?php

declare(strict_types=1);

namespace MixedSpread;

/** @param array<string, array<int, string>> $presets */
function countPresets(array $presets): int
{
    return count($presets);
}

function presets(mixed $integration, mixed $optional): int
{
    return countPresets([...$integration, ...$optional]);
}

function reactVersion(mixed $dependencies, mixed $devDependencies): mixed
{
    $node = [...$devDependencies ?? [], ...$dependencies ?? []];

    return $node['react'] ?? null;
}

/**
 * @param list<int> $list
 * @param list<string> $names
 */
function lists(array $list, array $names): void
{
    \PHPStan\dumpType([...$list]);
    \PHPStan\dumpType([...$list, ...$names]);
}
