<?php

namespace App\Jobs;

use App\Events\PostPublished;
use App\Http\Requests\StorePostRequest;
use App\Models\Post;

class PublishPost
{
    public function __construct(private Post $post)
    {
    }

    public static function fromRequest(StorePostRequest $request): self
    {
        return new self(new Post());
    }

    public function handle(): Post
    {
        event(new PostPublished($this->post));

        return $this->post;
    }
}
