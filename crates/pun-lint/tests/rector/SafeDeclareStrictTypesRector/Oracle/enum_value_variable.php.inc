<?php

namespace Pun\Oracle\SafeStrict;

enum Kind: string
{
    case Projected = 'projected';
}

final class KeyCheck
{
    /** @param array<string, mixed> $values */
    public static function hasProjected(array $values): bool
    {
        $projected = Kind::Projected->value;

        return array_key_exists($projected, $values);
    }
}
