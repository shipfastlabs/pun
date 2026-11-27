<?php

declare(strict_types=1);

namespace NullsafeOffsetChains;

use function PHPStan\dumpType;

final class Address
{
    public string $email = 'a@example.com';

    public function email(): string
    {
        return 'a@example.com';
    }
}

final class Envelope
{
    /** @return Address[] */
    public function recipients(): array
    {
        return [new Address];
    }

    /** @var list<Address> */
    public array $to = [];

    /** @return list<Address|null> */
    public function maybe(): array
    {
        return [null];
    }
}

final class Message
{
    public function envelope(): Envelope
    {
        return new Envelope;
    }
}

final class Mailbox
{
    public function first(?Message $message): ?string
    {
        return $message?->envelope()->recipients()[0]->email();
    }

    public function dumps(?Message $message): void
    {
        dumpType($message?->envelope()->recipients()[0]);
        dumpType($message?->envelope()->recipients()[0]->email());
        dumpType($message?->envelope()->to[0]->email);
        dumpType($message?->envelope()->recipients()[0]->email);
        echo $message?->envelope()->maybe()[0]->email();
    }
}
