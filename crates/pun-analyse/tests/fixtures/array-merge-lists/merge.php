<?php

declare(strict_types=1);

namespace App;

use function PHPStan\dumpType;

final class Groups
{
    /**
     * @param  array<string, list<string>>  $groups
     * @return list<string>
     */
    public function flat(array $groups): array
    {
        return array_merge(...array_values($groups));
    }
}

/**
 * @param array<string, list<string>> $groups
 * @param list<list<int>> $lists
 * @param list<string> $a
 * @param list<int> $b
 * @param array<string, int> $m
 * @param non-empty-list<string> $ne
 * @param array<int, string> $ia
 */
function merges(array $groups, array $lists, array $a, array $b, array $m, array $ne, array $ia): void
{
    dumpType(array_merge(...array_values($groups)));
    dumpType(array_merge(...$groups));
    dumpType(array_merge(...$lists));
    dumpType(array_merge($a, $b));
    dumpType(array_merge($a, $m));
    dumpType(array_merge($ne, $b));
    dumpType(array_merge($ia, $b));
    dumpType(array_merge(...[$a, $b]));
    dumpType(array_merge([], ...$lists));
}
