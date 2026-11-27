<?php

declare(strict_types=1);

namespace BleedingEdgeClosureParameters;

final class Box
{
    public function __construct(public int $shelf) {}
}

final class Shelf
{
    /** @param list<Box> $boxes */
    public function __construct(private int $shelf, private array $boxes) {}

    public function first(): ?int
    {
        $shelf = $this->shelf;
        $elsewhere = static fn (int $otherShelf): bool => $otherShelf !== $shelf;

        return array_find([0, 1], fn (int $entry): bool => $elsewhere($this->boxes[$entry]->shelf));
    }

    public function label(bool $boxed, Box $default): string
    {
        $label = static fn (string $name, ?Box $box = null): string => $boxed || $box instanceof Box
            ? $name.($box ?? $default)->shelf
            : $name;

        return $label('b', new Box(1));
    }

    public function register(): void
    {
        $autoload = static function (string $name): void {
        };

        spl_autoload_register($autoload);
    }
}
