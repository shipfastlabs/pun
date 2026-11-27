<?php

declare(strict_types=1);

namespace App;

use function PHPStan\dumpType;

final class Chunks
{
    public function start(mixed $ends, int $chunk): mixed
    {
        $end = is_array($ends) ? ($ends[$chunk] ?? null) : null;

        if (! is_array($end)) {
            return null;
        }

        return $ends[$chunk - 1] ?? null;
    }

    public function verb(mixed $body): mixed
    {
        $verbs = is_array($body) ? array_keys($body) : [];

        if (count($verbs) !== 1) {
            return null;
        }

        return $body[$verbs[0]];
    }
}

/** @param array<string, int> $map */
function holders(mixed $ends, mixed $x, mixed $y, array $map): void
{
    $end = is_array($ends) ? ($ends['a'] ?? null) : null;
    if (is_array($end)) {
        dumpType($ends);
    }
    if ($end === null) {
        dumpType($ends);
    }
    $other = is_array($x) ? $x : 0;
    if ($other !== 0) {
        dumpType($x);
    }
    $values = is_string($y) ? $map : [];
    if ($values !== []) {
        dumpType($y);
    }
    if (count($values) > 0) {
        dumpType($values);
    }
}
