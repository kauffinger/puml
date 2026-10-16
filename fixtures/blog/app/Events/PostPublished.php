<?php

namespace App\Events;

use App\Listeners\NotifyAuthor;
use App\Models\Post;

class PostPublished
{
    public function __construct(public Post $post)
    {
    }

    public function listeners(): array
    {
        return [NotifyAuthor::class];
    }
}
