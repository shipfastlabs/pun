<?php

namespace App\Domain;

readonly class Money
{
    public function __construct(public int $amount)
    {
    }
}
