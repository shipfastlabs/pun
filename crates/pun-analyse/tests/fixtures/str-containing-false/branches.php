<?php

declare(strict_types=1);

namespace StrContainingFalse;

use function PHPStan\dumpType;

function afterReturn(string $v): ?string
{
    if (str_starts_with($v, 'a')) {
        return null;
    }

    dumpType($v);

    return $v === 'b' ? $v : null;
}

function label(string $name): string
{
    if (str_ends_with($name, '_between') && !str_starts_with($name, 'amount')) {
        dumpType($name);

        return $name === 'period_between' ? 'Period' : 'Other';
    }

    return '';
}

function notFound(string $v): void
{
    if (!str_contains($v, 'x')) {
        dumpType($v);
    } else {
        dumpType($v);
    }
}

function found(?string $secret): void
{
    if (!str_starts_with($secret ?? '', 'sk_')) {
        return;
    }

    dumpType($secret);
}

final class Markers
{
    public static function stored(string $text): bool
    {
        $isLine = str_starts_with($text, '//');
        $isMarked = str_contains($text, 'MARK:');

        return $isLine && $isMarked;
    }

    public static function storedInOr(string $text, bool $continues): bool
    {
        $isLine = str_starts_with($text, '//');
        $isMarked = str_contains($text, 'MARK:');

        return $isLine && ($isMarked || $continues);
    }

    public static function storedEnds(string $text): bool
    {
        $isLine = str_ends_with($text, '//');
        $isMarked = str_contains($text, 'MARK:');

        return $isLine && $isMarked;
    }

    public static function storedTrue(string $text): void
    {
        $isLine = str_starts_with($text, '//');
        if ($isLine) {
            dumpType($text);
        }
    }
}
