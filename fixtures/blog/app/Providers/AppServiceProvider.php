<?php

namespace App\Providers;

use App\Http\Controllers\PostController;
use App\Models\Post;
use Illuminate\Support\ServiceProvider;

class AppServiceProvider extends ServiceProvider
{
    public function bindings(): array
    {
        return [PostController::class, Post::class];
    }
}
