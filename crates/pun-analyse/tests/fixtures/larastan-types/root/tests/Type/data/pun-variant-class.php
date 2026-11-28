<?php

declare(strict_types=1);

namespace {
    use Illuminate\Database\Eloquent\Model;
    use Illuminate\Database\Eloquent\Relations\HasMany;

    final class Variant extends Model
    {
    }

    final class PunVariantProduct extends Model
    {
        /** @return HasMany<Variant, $this> */
        public function variants(): HasMany
        {
            \PHPStan\Testing\assertType("'Variant'", Variant::class);

            return $this->hasMany(Variant::class);
        }
    }
}
