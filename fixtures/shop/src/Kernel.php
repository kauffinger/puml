<?php

namespace Shop;

use Shop\Orders\OrderService;

final class Kernel
{
    public function __construct(private OrderService $orders)
    {
    }
}
