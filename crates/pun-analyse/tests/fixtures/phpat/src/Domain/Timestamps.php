<?php

namespace App\Domain;

trait Timestamps
{
    public function touch(): void
    {
        new \App\Infra\Clock();
    }
}
