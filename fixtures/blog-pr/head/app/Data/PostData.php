<?php

namespace App\Data;

use App\Http\Requests\StorePostRequest;

class PostData
{
    public function __construct(
        public readonly string $title,
        public readonly string $body,
    ) {
    }

    public static function fromRequest(StorePostRequest $request): self
    {
        return new self($request->title, $request->body);
    }
}
