<?php

declare(strict_types=1);

namespace PunFinalStatic;

use Illuminate\Database\Eloquent\Builder;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Prunable;

trait HasStatus
{
    /**
     * @param Builder<static> $query
     * @return Builder<static>
     */
    public function scopeActive(Builder $query): Builder
    {
        return $query->where('active', true);
    }
}

final class Post extends Model
{
    use HasStatus;
    use Prunable;

    /** @return Builder<static> */
    public function prunable(): Builder
    {
        return self::where('created_at', '<=', now()->subDays(14));
    }
}

class OpenPost extends Model
{
    /** @return Builder<static> */
    public function prunable(): Builder
    {
        return self::where('created_at', '<=', now()->subDays(14));
    }
}

function makeModel(): Model
{
    return new class () extends Model {
        use HasStatus;
    };
}
