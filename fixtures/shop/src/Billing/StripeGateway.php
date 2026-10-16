<?php

namespace Shop\Billing;

use Acme\Payments\Client;
use Shop\Support\Money;

final class StripeGateway implements PaymentGateway
{
    public function __construct(private Client $client)
    {
    }

    public function charge(Money $amount): void
    {
    }
}
