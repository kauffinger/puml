<?php

namespace App\Listeners;

use App\Events\PostPublished;

class SendPublishedNotification
{
    public function handle(PostPublished $event): void
    {
    }
}
