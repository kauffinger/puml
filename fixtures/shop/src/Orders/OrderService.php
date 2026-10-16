<?php

namespace Shop\Orders;

use Acme\Payments\Client;
use Shop\Billing\Invoice;
use Shop\Billing\InvoiceRepository;
use Shop\Billing\PaymentFailed;
use Shop\Billing\PaymentGateway;
use Shop\Catalog\Product;

final class OrderService
{
    public function __construct(
        private PaymentGateway $gateway,
        private InvoiceRepository $invoices,
    ) {
    }

    public function place(Product $product): Order
    {
        $order = new Order();

        try {
            $this->gateway->charge($product->price());
        } catch (PaymentFailed $exception) {
            return $order;
        }

        $this->invoices->save(new Invoice($order));

        return $order;
    }

    public function client(string $class): Client
    {
        return new $class();
    }
}
