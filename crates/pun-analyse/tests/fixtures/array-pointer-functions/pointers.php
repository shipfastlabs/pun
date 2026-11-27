<?php

declare(strict_types=1);

namespace ArrayPointerFunctions;

use function PHPStan\dumpType;

final class Orders
{
    /**
     * @param  list<string>  $columns
     * @return non-empty-list<string>|null
     */
    public function keyed(array $columns): ?array
    {
        return $columns !== [] && end($columns) === 'id' ? $columns : null;
    }

    /** @param list<string> $columns */
    public function pointers(array $columns): void
    {
        if ($columns === []) {
            return;
        }
        end($columns);
        dumpType($columns);
        reset($columns);
        dumpType($columns);
        next($columns);
        dumpType($columns);
        prev($columns);
        dumpType($columns);
        sort($columns);
        dumpType($columns);
    }

    /** @param array{a: int, b: string} $row */
    public function shape(array $row): void
    {
        dumpType(end($row));
        dumpType($row);
    }
}
