<?php

namespace App\Cases;

final class Dep
{
    public static function make(): self
    {
        return new self();
    }
}
