<?php

declare(strict_types=1);

namespace PunHigherOrderRelations;

use Illuminate\Database\Eloquent\Collection;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\HasMany;

use function PHPStan\Testing\assertType;

final class Answer extends Model
{
}

final class Report extends Model
{
    /** @return HasMany<Answer, $this> */
    public function answers(): HasMany
    {
        return $this->hasMany(Answer::class);
    }
}

/** @param Collection<int, Report> $reports */
function loadAnswerMedia(Collection $reports): void
{
    assertType('Illuminate\Database\Eloquent\Collection<int, mixed>', Collection::make($reports->flatMap->answers));
    Collection::make($reports->flatMap->answers)->load(['media']);
}
