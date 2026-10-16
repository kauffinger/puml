<?php

namespace App\Jobs;

use App\Data\PostData;
use App\Events\PostPublished;
use App\Models\Post;

class PublishPost
{
    public function __construct(private Post $post)
    {
    }

    public static function fromData(PostData $data): self
    {
        return new self(new Post());
    }

    public function handle(): Post
    {
        event(new PostPublished($this->post));

        return $this->post;
    }
}
