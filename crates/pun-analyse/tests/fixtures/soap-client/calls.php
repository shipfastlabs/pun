<?php

namespace App;

use SoapClient;

final class Weather extends SoapClient
{
    public function today(): mixed
    {
        return $this->getForecast(['day' => 1]);
    }
}

final class Magic
{
    /** @param array<mixed> $arguments */
    public function __call(string $name, array $arguments): mixed
    {
        return null;
    }
}

function operations(SoapClient $client, Weather $weather, Magic $magic): void
{
    \PHPStan\dumpType($client->download(['id' => 1]));
    \PHPStan\dumpType($weather->getForecast());
    \PHPStan\dumpType($client->__getFunctions());
    $magic->download();
}
