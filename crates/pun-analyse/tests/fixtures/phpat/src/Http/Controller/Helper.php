<?php

namespace App\Http\Controller;

use App\Domain\Order;

class Helper
{
    public function order(): Order
    {
        return new Order();
    }
}
