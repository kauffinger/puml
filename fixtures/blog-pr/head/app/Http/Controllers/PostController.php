<?php

namespace App\Http\Controllers;

use App\Console\Commands\PrunePosts;
use App\Data\PostData;
use App\Http\Requests\StorePostRequest;
use App\Jobs\PublishPost;
use App\Models\Post;

class PostController
{
    public function store(StorePostRequest $request): Post
    {
        $job = PublishPost::fromData(PostData::fromRequest($request));

        return $job->handle();
    }

    public static function urlFor(Post $post): string
    {
        return '/posts/' . $post->id;
    }

    public function prune(): string
    {
        return PrunePosts::class;
    }
}
