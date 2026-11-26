<?php

namespace App\Domain\Contracts;

interface Named extends Entity
{
    public function name(): string;
}
