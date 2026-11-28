<?php

declare(strict_types=1);

namespace PunRequireExtendsClassStrings;

use Illuminate\Database\Eloquent\Model;
use InvalidArgumentException;

use function PHPStan\Testing\assertType;

/**
 * @phpstan-require-extends Model
 */
interface Commentable
{
}

/** @return class-string<Model&Commentable> */
function commentableClass(string $class): string
{
    if (!is_a($class, Model::class, true)) {
        throw new InvalidArgumentException('Not a model.');
    }

    if (!is_a($class, Commentable::class, true)) {
        throw new InvalidArgumentException('Not commentable.');
    }

    assertType('class-string<Illuminate\Database\Eloquent\Model&PunRequireExtendsClassStrings\Commentable>', $class);

    return $class;
}

/** @param class-string<Commentable> $class */
function reversed(string $class): void
{
    if (is_a($class, Model::class, true)) {
        assertType('class-string<Illuminate\Database\Eloquent\Model&PunRequireExtendsClassStrings\Commentable>', $class);
    }
}
