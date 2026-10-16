<?php

namespace Shop\Catalog;

use Illuminate\Support\Facades\Cache;

class ProductService
{
    /**
     * @return list<Product>
     */
    public function featured(): array
    {
        return Cache::remember('featured', 60, fn () => []);
    }

    public function isProduct(mixed $value): bool
    {
        return $value instanceof Product;
    }
}
