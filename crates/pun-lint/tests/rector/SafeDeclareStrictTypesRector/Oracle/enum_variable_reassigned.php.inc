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
        $projected = 'executed';

        return array_key_exists($projected, $values);
    }
}
-----
<?php

declare(strict_types=1);

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
        $projected = 'executed';

        return array_key_exists($projected, $values);
    }
}
