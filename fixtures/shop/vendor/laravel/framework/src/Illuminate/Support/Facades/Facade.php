<?php

namespace Illuminate\Support\Facades;

abstract class Facade
{
    public static function defaultAliases()
    {
        return [
            'Cache' => Cache::class,
        ];
    }
}
