<?php

declare(strict_types=1);

namespace ArrayAccessCoalesce;

/** @implements \ArrayAccess<int, array{year: int, participants: int}> */
final class Rows implements \ArrayAccess
{
    public function offsetExists(mixed $offset): bool
    {
        return true;
    }

    /** @return array{year: int, participants: int} */
    public function offsetGet(mixed $offset): mixed
    {
        return ['year' => 2026, 'participants' => 1];
    }

    public function offsetSet(mixed $offset, mixed $value): void
    {
    }

    public function offsetUnset(mixed $offset): void
    {
    }
}

function participantsIn(Rows $rows, int $year): int
{
    return $rows[$year]['participants'] ?? 0;
}

/** @param array<int, array{year: int, participants: int}> $rows */
function participantsInArray(array $rows, int $year): int
{
    return $rows[$year]['participants'] ?? 0;
}

/** @param array{year: int, participants: int} $row */
function participants(array $row): int
{
    return $row['participants'] ?? 0;
}
