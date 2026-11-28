<?php

declare(strict_types=1);

namespace DateTimeZones;

use DateTime;
use DateTimeImmutable;
use DateTimeInterface;

function nextDay(DateTimeImmutable $now, string $date): string
{
    return (new DateTimeImmutable($date, $now->getTimezone()))->modify('+1 day')->format('Y-m-d');
}

function timezoneName(DateTimeInterface $date): string
{
    return $date->getTimezone()->getName();
}

function types(DateTime $mutable, DateTimeImmutable $immutable, DateTimeInterface $interface, \DateTimeZone $zone): void
{
    \PHPStan\dumpType($mutable->getTimezone());
    \PHPStan\dumpType($immutable->getTimezone());
    \PHPStan\dumpType($interface->getTimezone());
    \PHPStan\dumpType(DateTime::createFromFormat('Y', '2026'));
}
