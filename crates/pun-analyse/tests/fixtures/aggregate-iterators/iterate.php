<?php

namespace App;

use DatePeriod;
use DateTimeImmutable;
use Generator;

/** CarbonPeriod's shape: a DatePeriod whose getIterator() is a Generator without PHPDoc. */
final class Period extends DatePeriod
{
    public function getIterator(): Generator
    {
        yield 0 => new DateTimeImmutable();
    }
}

final class Dates extends DatePeriod
{
    /** @return Generator<int, DateTimeImmutable> */
    public function getIterator(): Generator
    {
        yield 1 => new DateTimeImmutable();
    }
}

final class Inherited extends DatePeriod
{
}

function iterate(Period $period, Dates $dates, Inherited $inherited, DatePeriod $native): void
{
    foreach ($period as $key => $date) {
        \PHPStan\dumpType($key);
        \PHPStan\dumpType($date);
        $date->clone()->startOfMonth();
    }
    foreach ($dates as $key => $date) {
        \PHPStan\dumpType($key);
        \PHPStan\dumpType($date);
    }
    foreach ($inherited as $date) {
        \PHPStan\dumpType($date);
    }
    foreach ($native as $date) {
        \PHPStan\dumpType($date);
    }
}

