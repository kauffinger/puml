<?php

namespace App\Listeners;

use App\Events\PostPublished;

class NotifySubscribers
{
    public function handle(PostPublished $event): void
    {
    }
}
