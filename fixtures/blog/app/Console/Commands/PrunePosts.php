<?php

namespace App\Console\Commands;

use App\Http\Controllers\PostController;
use App\Models\Post;
use Illuminate\Console\Command;

class PrunePosts extends Command
{
    public function handle(PostController $controller): void
    {
        Post::query();
    }
}
