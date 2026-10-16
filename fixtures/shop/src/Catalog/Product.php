<?php

namespace Shop\Catalog;

use Illuminate\Database\Eloquent\Model;
use Shop\Support\HasUuid;
use Shop\Support\Money;

class Product extends Model
{
    use HasUuid;

    public function category()
    {
        return $this->belongsTo(Category::class);
    }

    public function price(): Money
    {
        return Money::of(100);
    }
}
