<?php

declare(strict_types=1);

namespace PunAttributeMake;

use Illuminate\Database\Eloquent\Casts\Attribute;
use Illuminate\Database\Eloquent\Model;

use function PHPStan\Testing\assertType;

final class Setting extends Model
{
    /** @return Attribute<array<string, mixed>, array<string, mixed>|null> */
    protected function configuration(): Attribute
    {
        $attribute = Attribute::make(
            get: fn (mixed $value): array => is_string($value) ? ['raw' => $value] : [],
            set: fn (?array $value): ?string => $value === null ? null : (json_encode($value) ?: null),
        );
        assertType('Illuminate\Database\Eloquent\Casts\Attribute', $attribute);
        assertType('Illuminate\Database\Eloquent\Casts\Attribute<int, never>', Attribute::get(fn (): int => 1));

        return $attribute;
    }
}
