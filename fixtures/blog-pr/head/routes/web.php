<?php

use App\Http\Controllers\PostController;
use Illuminate\Support\Facades\Route;

Route::post('/posts', [PostController::class, 'store']);
