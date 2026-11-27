<?php

declare(strict_types=1);

namespace PunRelationNarrowing;

use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\BelongsTo;

use function PHPStan\Testing\assertType;

final class Box extends Model
{
    public function label(): string
    {
        return 'box';
    }
}

final class Order extends Model
{
    /** @return BelongsTo<Box, $this> */
    public function box(): BelongsTo
    {
        return $this->belongsTo(Box::class);
    }
}

function test(Order $order): bool
{
    assertType('PunRelationNarrowing\Box|null', $order->box);
    if ($order->box instanceof Box) {
        assertType('PunRelationNarrowing\Box', $order->box);
    }
    if ($order->box !== null) {
        assertType('PunRelationNarrowing\Box', $order->box);
    }
    assertType('PunRelationNarrowing\Box|null', $order->box);

    return $order->box === null;
}

function label(Order $order): string
{
    return $order->box instanceof Box ? $order->box->label() : '';
}

function unguarded(Order $order): string
{
    return $order->box->label();
}
