<?php

namespace App\Http\Middleware;

use Closure;

class EnsureAdmin
{
    public function handle($request, Closure $next)
    {
        return $next($request);
    }
}
