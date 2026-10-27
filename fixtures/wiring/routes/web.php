<?php

use App\Http\Controllers\PostController;
use Illuminate\Support\Facades\Route;

Route::view('/', 'welcome')->name('home');

Route::controller(PostController::class)->prefix('posts')->name('posts.')->group(function () {
    Route::get('/', 'index')->name('index');
    Route::get('{post:slug}', 'show')->name('show');
});

Route::middleware('admin')->group(function () {
    Route::post('posts', [PostController::class, 'store'])->name('posts.store');
    Route::delete('posts/{post}', [PostController::class, 'destroy'])->withoutMiddleware('web');
});

if (app()->isLocal()) {
    Route::get('debug', fn () => 'debug');
}
