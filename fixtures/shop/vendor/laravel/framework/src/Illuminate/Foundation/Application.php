<?php

namespace Illuminate\Foundation;

class Application
{
    public function registerCoreContainerAliases()
    {
        foreach ([
            'app' => [self::class, \Illuminate\Contracts\Foundation\Application::class],
            'cache' => [\Illuminate\Cache\CacheManager::class, \Illuminate\Contracts\Cache\Factory::class],
        ] as $key => $aliases) {
            //
        }
    }
}
