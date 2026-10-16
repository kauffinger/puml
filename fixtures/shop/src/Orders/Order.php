<?php

namespace Shop\Orders;

use Illuminate\Database\Eloquent\Model;
use Shop\Billing\Invoice;
use Shop\Catalog\Product;
use Shop\Support\HasUuid;

class Order extends Model
{
    use HasUuid;

    public OrderStatus $status = OrderStatus::Pending;

    /** @var list<Product> */
    public array $lines = [];

    public function products()
    {
        return $this->belongsToMany(Product::class);
    }

    public function invoice()
    {
        return $this->hasOne(Invoice::class);
    }

    public function billable()
    {
        return $this->morphTo();
    }
}
