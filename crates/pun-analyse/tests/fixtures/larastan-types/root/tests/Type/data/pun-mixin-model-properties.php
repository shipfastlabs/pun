<?php

namespace PunMixinModelProperties;

use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\HasMany;
use Illuminate\Http\Request;
use Illuminate\Http\Resources\Json\JsonResource;

use function PHPStan\Testing\assertType;

final class Apple extends Model
{
}

final class Box extends Model
{
    /** @return HasMany<Apple, $this> */
    public function apples(): HasMany
    {
        return $this->hasMany(Apple::class);
    }
}

/**
 * @mixin Box
 */
final class BoxResource extends JsonResource
{
    /** @return array{apples: int, apple_count: int} */
    public function toArray(Request $request): array
    {
        assertType('Illuminate\Database\Eloquent\Collection<int, PunMixinModelProperties\Apple>', $this->apples);
        assertType('int<0, max>', $this->apples_count);

        return [
            'apples' => $this->apples->count(),
            'apple_count' => (int) $this->apples_count,
        ];
    }
}

final class Plain
{
    public function read(Box $box): void
    {
        assertType('Illuminate\Database\Eloquent\Collection<int, PunMixinModelProperties\Apple>', $box->apples);
        assertType('int<0, max>', $box->apples_count);
    }
}
