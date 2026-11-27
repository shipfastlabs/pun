<?php

declare(strict_types=1);

namespace App;

use function PHPStan\dumpType;

final class Sessions implements \SessionHandlerInterface
{
    public function open($path, $name): bool
    {
        return true;
    }

    public function close(): bool
    {
        return true;
    }

    public function read($id): string|false
    {
        return $id === '' ? false : strtoupper($id);
    }

    public function write($id, $data): bool
    {
        dumpType($data);

        return true;
    }

    public function destroy($id): bool
    {
        return true;
    }

    public function gc($max_lifetime): int|false
    {
        return $max_lifetime > 0 ? time() - $max_lifetime : false;
    }
}

class Handler extends \SessionHandler
{
    public function read($id): string|false
    {
        dumpType($id);

        return '';
    }
}

interface Store
{
    public function put(string $key): void;
}

final class Memory implements Store
{
    public function put($key): void
    {
        dumpType($key);
    }
}
