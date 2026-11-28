<?php

declare(strict_types=1);

namespace CoalesceNullableReceiver;

use function PHPStan\dumpType;

final class Inner
{
    public string $plain = 'x';

    public function __construct(public string $key, public readonly string $ro)
    {
    }
}

final class Holder
{
    public ?Inner $inner = null;

    public Inner $set;

    public function __construct(Inner $set)
    {
        $this->set = $set;
    }
}

function fetch(?Inner $inner, ?string $fallback, Holder $holder, Inner $set): void
{
    $key = $inner->key ?? $fallback;
    dumpType($key);
    if ($key !== null) {
        echo $key;
    }

    dumpType($inner->ro ?? $fallback);
    dumpType($inner->plain ?? $fallback);
    dumpType($inner->plain ?? 'default');
    dumpType($holder->inner->ro ?? $fallback);
    dumpType($holder->inner?->ro ?? $fallback);
    dumpType($set->ro ?? $fallback);
    dumpType($holder->set->ro ?? $fallback);
}
