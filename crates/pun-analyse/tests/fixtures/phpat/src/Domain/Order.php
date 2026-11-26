<?php

namespace App\Domain;

use App\Attr\Aggregate;
use App\Infra\Db;

#[Aggregate]
final class Order extends Base implements Contracts\Named
{
    public function name(): string
    {
        return (new Db())->name();
    }
}
