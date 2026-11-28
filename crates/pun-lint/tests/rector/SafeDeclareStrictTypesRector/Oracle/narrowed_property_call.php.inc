<?php

namespace Pun\Oracle\SafeStrict;

interface Account
{
}

final class Member implements Account
{
    public function touch(): void
    {
    }
}

final class Login
{
    public function __construct(public Account $account)
    {
    }
}

final class RecordLogin
{
    public function handle(Login $event): void
    {
        if (! $event->account instanceof Member) {
            return;
        }

        $event->account->touch();
    }
}
