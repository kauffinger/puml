<?php

namespace App\Http\Middleware;

use Closure;

class TrackVisits
{
    public function handle($request, Closure $next)
    {
        return $next($request);
    }
}
