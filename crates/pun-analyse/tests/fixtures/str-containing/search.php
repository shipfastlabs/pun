<?php

declare(strict_types=1);

namespace App;

use function PHPStan\dumpType;

final class Keys
{
    /** @param array<string, string|null> $env */
    public function fromArray(array $env): string
    {
        if (! str_starts_with($env['SECRET'] ?? '', 'sk_')) {
            return '';
        }

        return $this->listen($env['SECRET']);
    }

    public function fromVariable(?string $secret): string
    {
        if (! str_starts_with($secret ?? '', 'sk_')) {
            return '';
        }

        return $this->listen($secret);
    }

    private function listen(string $secret): string
    {
        return $secret;
    }
}

function searches(string $s, ?string $n, string $needle, ?string $m): void
{
    if (str_contains($s, 'x')) {
        dumpType($s);
    }
    if (str_ends_with($s, '0')) {
        dumpType($s);
    }
    if (str_contains($s, $needle)) {
        dumpType($s);
    }
    if (str_starts_with($n ?? 'sk_', 'sk_')) {
        dumpType($n);
    }
    if (str_contains($m ?? '', 'x')) {
        dumpType($m);
    }
}
