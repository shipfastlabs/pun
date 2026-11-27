<?php

declare(strict_types=1);

namespace PunRequireExtendsNarrowing;

use Illuminate\Database\Eloquent\Model;
use LogicException;

use function PHPStan\Testing\assertType;

/**
 * @phpstan-require-extends Model
 */
interface Seoable
{
}

interface Plain
{
}

function requireSeoable(Model $record): Model&Seoable
{
    if (!$record instanceof Seoable) {
        throw new LogicException('Not seoable.');
    }

    return $record;
}

function narrowed(Model $record): ?Model
{
    if ($record instanceof Seoable) {
        assertType('Illuminate\Database\Eloquent\Model&PunRequireExtendsNarrowing\Seoable', $record);

        return $record;
    }

    return null;
}

function plain(Model $record): ?Model
{
    if ($record instanceof Plain) {
        assertType('Illuminate\Database\Eloquent\Model&PunRequireExtendsNarrowing\Plain', $record);

        return $record;
    }

    return null;
}
