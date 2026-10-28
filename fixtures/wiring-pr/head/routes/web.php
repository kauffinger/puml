<?php

use App\Http\Controllers\CommentController;
use App\Http\Controllers\PostController;
use Illuminate\Support\Facades\Route;

Route::controller(PostController::class)->prefix('posts')->name('posts.')->group(function () {
    Route::get('/', 'index')->name('index');
    Route::get('{post:slug}', 'show')->name('show');
});

Route::post('posts', [PostController::class, 'store'])->name('posts.store');

Route::middleware('admin')->group(function () {
    Route::delete('posts/{post}', [PostController::class, 'destroy'])->withoutMiddleware('web');
    Route::post('posts/{post}/comments', [CommentController::class, 'store'])->name('comments.store');
});

if (app()->isLocal()) {
    Route::get('debug', fn () => 'debug');
}
