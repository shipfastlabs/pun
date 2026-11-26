<?php

namespace App\Infra;

class Db
{
    public const NAME = 'db';

    public function name(): string
    {
        return self::NAME;
    }
}
