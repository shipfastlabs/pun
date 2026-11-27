<?php

declare(strict_types=1);

namespace BleedingEdgeInheritedConstructors;

final class Apple
{
}

/**
 * @template TKey of array-key
 * @template TValue
 */
class Items
{
    /** @param array<TKey, TValue> $items */
    public function __construct(public array $items = []) {}
}

/**
 * @template TKey of array-key
 * @template TModel of object
 *
 * @extends Items<TKey, TModel>
 */
final class Models extends Items
{
}

final class Packer
{
    /** @return Models<int, Apple> */
    public function pack(): Models
    {
        $make = fn (): Models => new Models([new Apple]);

        return $make();
    }

    /** @return Models<int, Apple> */
    public function invoked(): Models
    {
        return (fn (): Models => new Models([new Apple]))();
    }
}
