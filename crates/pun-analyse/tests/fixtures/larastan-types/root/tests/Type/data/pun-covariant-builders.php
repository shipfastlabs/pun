<?php

namespace PunCovariantBuilders;

use Illuminate\Database\Eloquent\Builder;
use Illuminate\Database\Eloquent\Model;

/**
 * @param Builder<covariant Model> $query
 * @return Builder<covariant Model>
 */
function withComments(Builder $query): Builder
{
    return $query->with('comments');
}

/**
 * @param Builder<covariant Model> $query
 * @return Builder<covariant Model>
 */
function whereHasComments(Builder $query): Builder
{
    return $query->whereHas('comments', fn (Builder $comments): Builder => $comments->where('approved', true));
}
