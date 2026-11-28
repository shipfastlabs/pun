<?php

declare(strict_types=1);

namespace PunIntersectedModels;

use Illuminate\Database\Eloquent\Model;

use function PHPStan\Testing\assertType;

interface Publishable
{
    public function label(): string;
}

/**
 * @template TModel of Model&Publishable
 *
 * @param class-string<Model&Publishable> $class
 * @param class-string<TModel> $template
 */
function types(string $class, string $template, Model&Publishable $record, mixed $id): void
{
    assertType('Illuminate\Database\Eloquent\Model|null', $class::find(1));
    assertType('Illuminate\Database\Eloquent\Builder<Illuminate\Database\Eloquent\Model&PunIntersectedModels\Publishable>', $class::query());
    assertType('Illuminate\Database\Eloquent\Collection<int, Illuminate\Database\Eloquent\Model&PunIntersectedModels\Publishable>', $class::query()->get());
    assertType('Illuminate\Database\Eloquent\Collection<int, Illuminate\Database\Eloquent\Model&PunIntersectedModels\Publishable>', $class::all());
    assertType('Illuminate\Database\Eloquent\Builder<Illuminate\Database\Eloquent\Model&PunIntersectedModels\Publishable>', $record::query());
    assertType('Illuminate\Database\Eloquent\Collection<int, Illuminate\Database\Eloquent\Model&PunIntersectedModels\Publishable>|(Illuminate\Database\Eloquent\Model&PunIntersectedModels\Publishable)|null', $record::query()->find($id));
}
