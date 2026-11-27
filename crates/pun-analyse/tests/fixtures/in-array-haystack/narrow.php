<?php

declare(strict_types=1);

namespace App;

use function PHPStan\dumpType;

final class Images
{
    /** @param list<\GdImage|false> $parts */
    public function width(array $parts): int
    {
        if (in_array(false, $parts, true)) {
            return 0;
        }

        return array_sum(array_map(imagesx(...), $parts));
    }

    /** @param list<int|false> $at */
    public function allPositive(array $at): bool
    {
        return ! in_array(false, $at, true) && array_all($at, static fn (int $index): bool => $index > 0);
    }
}

/**
 * @param array<string, string|null> $map
 * @param list<'a'|'b'> $letters
 * @param list<int|false> $loose
 * @param list<int|false> $kept
 * @param array<string, int> $ids
 */
function narrowing(array $map, array $letters, array $loose, array $kept, array $ids, int $id): void
{
    if (! in_array(null, $map, true)) {
        dumpType($map);
    }
    if (! in_array('a', $letters, true)) {
        dumpType($letters);
    }
    if (! in_array(false, $loose)) {
        dumpType($loose);
    }
    if (in_array(false, $kept, true)) {
        dumpType($kept);
    }
    if (in_array($id, $ids, true)) {
        dumpType($ids);
    }
}
