<?php

namespace App\Models;

use App\Console\Commands\PrunePosts;
use App\Http\Controllers\PostController;
use Illuminate\Database\Eloquent\Model;

class Post extends Model
{
    public function author()
    {
        return $this->belongsTo(User::class);
    }

    public function url(): string
    {
        return PostController::urlFor($this);
    }

    public function pruner(): string
    {
        return PrunePosts::class;
    }
}
