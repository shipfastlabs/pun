<?php

declare(strict_types=1);

namespace PunNestedEagerLoad;

use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\BelongsTo;
use Illuminate\Database\Eloquent\Relations\HasMany;

use function PHPStan\Testing\assertType;

final class Review extends Model
{
    /** @return BelongsTo<Report, $this> */
    public function report(): BelongsTo
    {
        return $this->belongsTo(Report::class);
    }
}

final class Report extends Model
{
    /** @return HasMany<Review, $this> */
    public function reviews(): HasMany
    {
        return $this->hasMany(Review::class);
    }
}

final class Organisation extends Model
{
    /** @return HasMany<Report, $this> */
    public function reports(): HasMany
    {
        return $this->hasMany(Report::class);
    }
}

function loadReturnedReviews(Organisation $organisation, bool $onlyReturned): void
{
    $organisation->load([
        'reports' => fn ($reports) => $reports->where('visible', true),
        'reports.reviews' => fn ($reviews) => $reviews
            ->when($onlyReturned, function ($query) {
                assertType('Illuminate\Database\Eloquent\Builder<PunNestedEagerLoad\Report>|Illuminate\Database\Eloquent\Builder<PunNestedEagerLoad\Review>', $query);

                return $query->whereHas('report', fn ($reports) => $reports->whereNotNull('returned_at'));
            }),
    ]);
}
