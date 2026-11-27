<?php

namespace App;

final class Item
{
    /** @var list<string> */
    public array $names = [];

    public ?string $label = null;
}

function nullsafeEmpty(?Item $item, ?int $limit = null): int
{
    if (empty($item?->names)) {
        return 0;
    }

    \PHPStan\dumpType($item);
    \PHPStan\dumpType($item->names);
    \PHPStan\dumpType($limit);

    return 1;
}

function nullsafeNotIsset(?Item $item, int $count): int
{
    if (!isset($item?->label)) {
        return 0;
    }

    \PHPStan\dumpType($item->label);
    \PHPStan\dumpType($count);

    return 1;
}

function plainEmpty(?Item $item, int $count): int
{
    if ($item === null || empty($item->names)) {
        return 0;
    }

    \PHPStan\dumpType($item->names);
    \PHPStan\dumpType($count);

    return 1;
}
