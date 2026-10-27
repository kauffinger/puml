<?php

namespace App\Policies;

use App\Models\Post;

class PostPolicy
{
    public function update($user, Post $post): bool
    {
        return true;
    }
}
