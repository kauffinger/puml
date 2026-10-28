<?php

use App\Console\Commands\PruneDrafts;
use Illuminate\Support\Facades\Schedule;

Schedule::command(PruneDrafts::class)->daily();
