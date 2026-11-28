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

final class RecordLogin
{
    public function __construct(public Account $account)
    {
    }

    public function handle(): void
    {
        if (! $this->account instanceof Member) {
            return;
        }

        $this->account->touch();
    }
}
