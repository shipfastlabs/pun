<?php

declare(strict_types=1);

namespace PunCollectionFilterOffsets;

use Illuminate\Support\Collection;

use function PHPStan\Testing\assertType;

/** @param Collection<int, array{id: int, group: array{id: int}|null}> $items */
function countGrouped(Collection $items): int
{
    assertType('Illuminate\Support\Collection<int, array{id: int, group: array{id: int}}>', $items->filter(fn (array $item): bool => $item['group'] !== null));
    assertType('Illuminate\Support\Collection<int, array{id: int, group: null}>', $items->filter(fn (array $item): bool => $item['group'] === null));
    assertType('Illuminate\Support\Collection<int, array{id: int, group: array{id: int}}>', $items->filter(fn (array $item): bool => is_array($item['group'])));
    assertType('Illuminate\Support\Collection<int, array{id: int<min, -1>|int<1, max>, group: array{id: int}|null}>', $items->filter(fn (array $item): bool => $item['id'] !== 0));

    return $items
        ->filter(fn (array $item): bool => $item['group'] !== null)
        ->groupBy(fn (array $item): int => $item['group']['id'])
        ->map(fn (Collection $grouped): int => countWithGroup($grouped))
        ->count();
}

/** @param Collection<int, array{id: int, group: array{id: int}}> $items */
function countWithGroup(Collection $items): int
{
    return $items->count();
}
