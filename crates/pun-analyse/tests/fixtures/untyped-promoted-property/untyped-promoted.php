<?php

declare(strict_types=1);

namespace UntypedPromoted;

final class SignedOut
{
    /**
     * @param \Stringable $user
     */
    public function __construct(public $user)
    {
    }
}

final class Typed
{
    public function __construct(public \Stringable $user)
    {
    }
}

final class Documented
{
    /** @var \Stringable */
    public $user;

    public function __construct(\Stringable $user)
    {
        $this->user = $user;
    }
}

function untyped(SignedOut $event): bool
{
    return $event->user === null;
}

function untypedNot(SignedOut $event): bool
{
    return null !== $event->user;
}

function typed(Typed $event): bool
{
    return $event->user === null;
}

function documented(Documented $event): bool
{
    return $event->user === null;
}

function other(SignedOut $event): bool
{
    return $event->user === 1;
}
