<?php

declare(strict_types=1);

namespace App;

final class Queued
{
    public function __construct(public mixed $job) {}
}

final class Guard
{
    public function variable(object $job): bool
    {
        return property_exists($job, 'afterCommit') && $job->afterCommit !== false;
    }

    public function propertyFetch(Queued $queued): bool
    {
        if (! is_object($queued->job)) {
            return false;
        }

        return property_exists($queued->job, 'afterCommit') && $queued->job->afterCommit !== false;
    }

    public function unguarded(Queued $queued): bool
    {
        if (! is_object($queued->job)) {
            return false;
        }

        return $queued->job->afterCommit !== false;
    }

    public function reassigned(Queued $queued, object $other): bool
    {
        if (! is_object($queued->job) || ! property_exists($queued->job, 'afterCommit')) {
            return false;
        }
        $queued->job = $other;

        return $queued->job->afterCommit !== false;
    }
}
