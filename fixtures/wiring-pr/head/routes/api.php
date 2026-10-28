<?php

use App\Http\Controllers\PostController;
use Illuminate\Support\Facades\Route;

// The mobile app reads the same posts as the website.
Route::get('posts', [PostController::class, 'index'])->name('api.posts');
