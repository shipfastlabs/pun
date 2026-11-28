<?php

namespace Pun\Oracle\SafeStrict;

final class Member
{
    public function touch(): void
    {
    }
}

final class Visit
{
    public function __construct(public Member $member)
    {
    }
}

final class RecordVisit
{
    public function handle(Visit $event): void
    {
        $event->member->touch();
    }
}
-----
<?php

declare(strict_types=1);

namespace Pun\Oracle\SafeStrict;

final class Member
{
    public function touch(): void
    {
    }
}

final class Visit
{
    public function __construct(public Member $member)
    {
    }
}

final class RecordVisit
{
    public function handle(Visit $event): void
    {
        $event->member->touch();
    }
}
