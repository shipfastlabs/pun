<?php

declare(strict_types=1);

namespace ValueOnlyGenerics;

/**
 * @template TKey of array-key
 * @template TValue
 * @implements \IteratorAggregate<TKey, TValue>
 */
final class Items implements \IteratorAggregate
{
    /** @param array<TKey, TValue> $items */
    public function __construct(private array $items)
    {
    }

    public function getIterator(): \ArrayIterator
    {
        return new \ArrayIterator($this->items);
    }
}

/**
 * @template T
 */
final class Box
{
}

final class Item
{
}

/** @return Items<Item> */
function items(): Items
{
    return new Items([new Item()]);
}

/** @param Items<int, Item> $items */
function countItems(Items $items): int
{
    return iterator_count($items);
}

/** @param Items<int, \stdClass> $items */
function countOthers(Items $items): int
{
    return iterator_count($items);
}

/** @param Box<Item> $box */
function box(Box $box): void
{
    \PHPStan\dumpType($box);
}

countItems(items());
countOthers(items());
\PHPStan\dumpType(items());
