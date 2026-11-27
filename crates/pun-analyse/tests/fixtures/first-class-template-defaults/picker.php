<?php

declare(strict_types=1);

namespace FirstClassTemplateDefaults;

use function PHPStan\dumpType;

final class Apple
{
}

final class Basket
{
    /** @param array<string, Apple> $apples */
    public function __construct(private array $apples) {}

    /**
     * @template TDefault
     *
     * @param  TDefault  $default
     * @return Apple|TDefault
     */
    public function get(string $name, mixed $default = null): mixed
    {
        return $this->apples[$name] ?? $default;
    }
}

final class Picker
{
    /**
     * @param  list<string>  $names
     * @return array<int, Apple>
     */
    public function pick(Basket $basket, array $names): array
    {
        dumpType($basket->get(...));
        dumpType(array_map($basket->get(...), $names));

        return array_filter(array_map($basket->get(...), $names));
    }

    /**
     * @param  list<string>  $names
     * @return list<Apple>
     */
    public function all(Basket $basket, array $names): array
    {
        return array_map($basket->get(...), $names);
    }

    /**
     * @param  array<string, string>  $names
     * @return array<string, Apple>
     */
    public function keyed(Basket $basket, array $names): array
    {
        return array_map($basket->get(...), $names);
    }
}
