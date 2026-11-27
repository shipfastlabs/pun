<?php

declare(strict_types=1);

namespace ClosureUnionParameters;

/** @template T */
final class Box
{
}

final class Apple
{
}

final class Pear
{
}

final class Plum
{
}

final class Fruit
{
    /** @param \Closure(Box<Apple>|Box<Pear>): mixed $scope */
    public function each(\Closure $scope): void
    {
        /** @var Box<Pear> $box */
        $box = new Box;
        $scope($box);
        /** @var Box<Apple> $apple */
        $apple = new Box;
        $scope($apple);
        /** @var Box<Plum> $plum */
        $plum = new Box;
        $scope($plum);
    }

    /** @param callable(Box<Apple>|Box<Pear>): mixed $scope */
    public function callable(callable $scope): void
    {
        /** @var Box<Pear> $box */
        $box = new Box;
        $scope($box);
        /** @var Box<Plum> $plum */
        $plum = new Box;
        $scope($plum);
    }
}
