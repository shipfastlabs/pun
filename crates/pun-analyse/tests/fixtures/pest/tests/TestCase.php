<?php

namespace Tests;

final class Box
{
    public string $name = 'box';
}

abstract class TestCase extends \PHPUnit\Framework\TestCase
{
    protected function brand(): string
    {
        return 'uscore';
    }
}
