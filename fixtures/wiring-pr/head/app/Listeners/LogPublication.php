<?php

namespace App\Listeners;

use App\Events\PostPublished;

class LogPublication
{
    public function handle(PostPublished $event): void
    {
    }
}
