<?php

namespace App\Domain;

enum Status: string implements Contracts\Entity
{
    case Open = 'open';
}
