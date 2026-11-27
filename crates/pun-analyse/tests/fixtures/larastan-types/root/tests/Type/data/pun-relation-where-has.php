<?php

declare(strict_types=1);

namespace PunRelationWhereHas;

use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\BelongsTo;
use Illuminate\Database\Eloquent\Relations\HasMany;

use function PHPStan\Testing\assertType;

final class Tag extends Model
{
}

final class Comment extends Model
{
    /** @return BelongsTo<Tag, $this> */
    public function tag(): BelongsTo
    {
        return $this->belongsTo(Tag::class);
    }
}

final class Post extends Model
{
    /** @return HasMany<Comment, $this> */
    public function comments(): HasMany
    {
        return $this->hasMany(Comment::class);
    }

    /** @return HasMany<Comment, $this> */
    public function taggedComments(): HasMany
    {
        return $this->comments()
            ->whereHas('tag', static fn ($query) => $query->where('active', true));
    }

    public function closures(): void
    {
        $this->comments()->whereHas('tag', static function ($query): void {
            assertType('Illuminate\Database\Eloquent\Builder<PunRelationWhereHas\Tag>', $query);
        });
        $this->comments()->whereDoesntHave('tag', static function ($query): void {
            assertType('Illuminate\Database\Eloquent\Builder<PunRelationWhereHas\Tag>', $query);
        });
        $this->comments()->orWhereHas('tag', static function ($query): void {
            assertType('Illuminate\Database\Eloquent\Builder<PunRelationWhereHas\Tag>', $query);
        });
        Comment::query()->whereHas('tag', static function ($query): void {
            assertType('Illuminate\Database\Eloquent\Builder<PunRelationWhereHas\Tag>', $query);
        });
    }
}
