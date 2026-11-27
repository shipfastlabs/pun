<?php

declare(strict_types=1);

namespace App;

use function PHPStan\dumpType;

final class Limits
{
    /** @param list<string> $resets */
    public function longest(array $resets): ?int
    {
        $spent = array_filter($resets, static fn (string $seconds, int $window): bool => ctype_digit($seconds) && $window > 0, ARRAY_FILTER_USE_BOTH);

        if ($spent !== []) {
            dumpType(array_map(intval(...), $spent));

            return max(array_map(intval(...), $spent));
        }

        return null;
    }

    /**
     * @param non-empty-array<string, int> $nonEmpty
     * @param array<string, int> $maybeEmpty
     * @param non-empty-list<int> $list
     */
    public function shapes(array $nonEmpty, array $maybeEmpty, array $list): void
    {
        dumpType(array_map(static fn (int $value): bool => $value > 0, $nonEmpty));
        dumpType(array_map(static fn (int $value): bool => $value > 0, $maybeEmpty));
        dumpType(array_map(static fn (int $value): bool => $value > 0, $list));
    }
}
