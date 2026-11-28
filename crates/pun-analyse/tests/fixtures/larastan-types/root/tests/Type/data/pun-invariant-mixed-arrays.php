<?php

declare(strict_types=1);

namespace PunInvariantMixedArrays;

use Illuminate\Support\Collection;

/**
 * @param array<int|string, mixed> $items
 * @return Collection<int|string, array<mixed>>
 */
function items(array $items): Collection
{
    return collect($items)->map(fn (mixed $item): array => (array) $item);
}

/**
 * @param list<mixed> $items
 * @return Collection<int, array<string>>
 */
function strings(array $items): Collection
{
    return collect($items)->map(fn (mixed $item): array => (array) $item);
}
