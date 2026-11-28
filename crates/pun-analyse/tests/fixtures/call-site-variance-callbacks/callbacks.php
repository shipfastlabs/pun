<?php

declare(strict_types=1);

namespace CallSiteVarianceCallbacks;

/** @template T */
final class Items
{
    /** @param list<T> $items */
    public function __construct(private array $items = []) {}

    /**
     * @template TMap
     *
     * @param  callable(T): TMap  $callback
     * @return Items<TMap>
     */
    public function map(callable $callback): Items
    {
        return new Items(array_map($callback, $this->items));
    }
}

class Animal {}

final class Report
{
    /** @param Items<covariant array<string, mixed>> $rows */
    public function covariantArray(Items $rows): void
    {
        $rows->map(fn (array $row): int => count($row));
    }

    /** @param Items<covariant Animal> $animals */
    public function covariantObject(Items $animals): void
    {
        $animals->map(fn (Animal $animal): string => $animal::class);
    }

    /** @param Items<array<string, mixed>> $rows */
    public function invariantArray(Items $rows): void
    {
        $rows->map(fn (array $row): int => count($row));
    }

    /** @param Items<covariant array<string, mixed>> $rows */
    public function untypedParam(Items $rows): void
    {
        $rows->map(fn ($row): int => count($row));
    }

    /** @param Items<covariant Animal> $animals */
    public function wrongType(Items $animals): void
    {
        $animals->map(fn (\stdClass $animal): bool => $animal instanceof \Countable);
    }
}
