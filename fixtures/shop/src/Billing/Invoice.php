<?php

namespace Shop\Billing;

use Shop\Orders\Order;

final class Invoice
{
    public function __construct(public readonly Order $order)
    {
    }
}
