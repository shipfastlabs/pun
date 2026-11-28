<?php

declare(strict_types=1);

namespace App;

function welcome(): string
{
    return greet(shout('Ada'));
}

function wrong(): string
{
    return greet(1);
}

function missing(): void
{
    undefined_helper();
}
