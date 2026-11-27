<?php

declare(strict_types=1);

namespace ArrayFindNarrowing;

use function PHPStan\dumpType;

final class Rows
{
    /**
     * @param  list<mixed>  $rows
     * @return array<mixed>
     */
    public function row(array $rows): array
    {
        return array_find($rows, static fn (mixed $row): bool => is_array($row)) ?? [];
    }

    /**
     * @param  list<int|string>  $values
     * @param  non-empty-list<int|string>  $some
     * @param  array{1, 'a', null}  $shape
     */
    public function dumps(array $values, array $some, array $shape): void
    {
        dumpType(array_find($values, static fn (int|string $v): bool => is_int($v)));
        dumpType(array_find($values, static function (int|string $v): bool {
            return is_string($v);
        }));
        dumpType(array_find($some, static fn (int|string $v): bool => is_int($v)));
        dumpType(array_find($values, static fn (int|string $v): bool => $v > 1));
        dumpType(array_find($values, 'is_int'));
        dumpType(array_find($shape, static fn (mixed $v): bool => $v !== null));
        dumpType(array_find($values, static fn (int|string $v, int $k): bool => $k > 0 && is_string($v)));
    }
}
