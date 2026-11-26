<?php

namespace App\Cases;

trait HasDep
{
    public function dep(): void
    {
        new Dep();
    }
}
