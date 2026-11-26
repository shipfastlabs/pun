<?php

namespace App\Infra;

final class LoggerFactory
{
    public function make(): Logger
    {
        return new Logger();
    }
}
