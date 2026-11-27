<?php

declare(strict_types=1);

namespace App;

final class Box
{
    public function label(): string
    {
        return 'box';
    }
}

final class Order
{
    public ?Box $box = null;
}

final class Shipment
{
    /** @var list<Order> */
    public array $orders = [];

    public function add(Order $order): static
    {
        $this->orders[] = $order;

        return $this;
    }

    public function push(Order $order): void
    {
        $this->orders[] = $order;
    }

    /** @phpstan-impure */
    public function impure(Order $order): static
    {
        $this->orders[] = $order;

        return $this;
    }

    /** @return $this */
    public function chain(Order $order): self
    {
        $this->orders[] = $order;

        return $this;
    }
}

final class Sender
{
    public function send(Order $order, Shipment $shipment): string
    {
        if ($order->box === null) {
            return '';
        }

        $shipment->add($order);

        return $order->box->label();
    }

    public function chained(Order $order, Shipment $shipment): string
    {
        if ($order->box === null) {
            return '';
        }

        $shipment->chain($order);

        return $order->box->label();
    }

    public function pushed(Order $order, Shipment $shipment): string
    {
        if ($order->box === null) {
            return '';
        }

        $shipment->push($order);

        return $order->box->label();
    }

    public function impure(Order $order, Shipment $shipment): string
    {
        if ($order->box === null) {
            return '';
        }

        $shipment->impure($order);

        return $order->box->label();
    }
}
