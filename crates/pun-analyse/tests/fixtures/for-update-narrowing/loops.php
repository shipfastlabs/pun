<?php

declare(strict_types=1);

namespace App;

// The update expression runs only after the last condition held (#104).
final class Failures
{
    public function first(?\Throwable $exception): ?\RuntimeException
    {
        for ($failure = $exception; $failure instanceof \Throwable; $failure = $failure->getPrevious()) {
            if ($failure instanceof \RuntimeException) {
                return $failure;
            }
        }

        return null;
    }

    public function emptyBody(?\Throwable $exception): void
    {
        for ($failure = $exception; $failure instanceof \Throwable; $failure = $failure->getPrevious()) {
        }
    }

    public function lastOfSeveral(?\Throwable $exception): void
    {
        for ($failure = $exception, $i = 0; $i < 10, $failure !== null; $failure = $failure->getPrevious(), $i++) {
        }
    }

    public function continued(?\Throwable $exception, bool $skip): int
    {
        $count = 0;
        for ($failure = $exception; $failure instanceof \Throwable; $failure = $failure->getPrevious()) {
            if ($skip) {
                continue;
            }
            $count++;
        }

        return $count;
    }

    // The body reassigns the variable: the update sees the body's type, not the condition's.
    public function reassigned(?\Throwable $exception): void
    {
        for ($failure = $exception; $failure instanceof \Throwable; $failure = $failure->getPrevious()) {
            $failure = $failure->getPrevious();
        }
    }

    // A continue taken before the condition's variable is narrowed again.
    public function continuedAfterReassign(?\Throwable $exception): void
    {
        for ($failure = $exception; $failure instanceof \Throwable; $failure = $failure->getPrevious()) {
            $failure = $failure->getPrevious();
            if ($failure === null) {
                continue;
            }
        }
    }

    // A condition on another variable narrows nothing.
    public function unrelatedCondition(?\Throwable $exception): void
    {
        for ($failure = $exception, $i = 0; $i < 10; $failure = $failure->getPrevious(), $i++) {
        }
    }

    // No condition at all.
    public function noCondition(?\Throwable $exception): void
    {
        for ($failure = $exception; ; $failure = $failure->getPrevious()) {
            if ($failure === null) {
                break;
            }
        }
    }
}
