<?php

use App\Console\Commands\PruneDrafts;
use App\Jobs\SendDigest;
use Illuminate\Support\Facades\Schedule;

Schedule::command(PruneDrafts::class)->daily();
Schedule::job(new SendDigest)->weekly();
