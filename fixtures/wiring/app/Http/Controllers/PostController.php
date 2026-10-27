<?php

namespace App\Http\Controllers;

use App\Models\Post;

class PostController
{
    public function index()
    {
        return Post::all();
    }

    public function show(Post $post)
    {
        return $post;
    }

    public function store()
    {
    }

    public function destroy(Post $post)
    {
        $post->delete();
    }
}
