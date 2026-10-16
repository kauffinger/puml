<?php

namespace Shop\Billing;

use Shop\Support\Money;

interface PaymentGateway
{
    public function charge(Money $amount): void;
}
