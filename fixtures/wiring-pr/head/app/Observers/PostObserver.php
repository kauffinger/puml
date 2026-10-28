<?php

namespace App\Observers;

use App\Models\Post;

class PostObserver
{
    public function created(Post $post): void
    {
    }

    public function updated(Post $post): void
    {
    }

    public function deleted(Post $post): void
    {
    }

    public function notAnEvent(): void
    {
    }
}
