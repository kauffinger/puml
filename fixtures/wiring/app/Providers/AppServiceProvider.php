<?php

namespace App\Providers;

use App\Contracts\Feed;
use App\Events\PostPublished;
use App\Listeners\WarmCache;
use App\Services\RssFeed;
use Illuminate\Support\Facades\Event;
use Illuminate\Support\ServiceProvider;

class AppServiceProvider extends ServiceProvider
{
    public function register(): void
    {
        $this->app->singleton(Feed::class, RssFeed::class);
    }

    public function boot(): void
    {
        Event::listen(PostPublished::class, WarmCache::class);
    }
}
