<?php

namespace PunStorageFacadeReturns;

use Illuminate\Support\Facades\Storage;

use function PHPStan\Testing\assertType;

final class Calls
{
    /** @param list<string> $keep */
    public function prune(array $keep): bool
    {
        assertType('array<string>', Storage::files('screens'));
        assertType('array<string>', Storage::allFiles());
        assertType('string|null', Storage::get('a'));
        assertType('bool', Storage::delete('a'));

        return Storage::delete(array_diff(Storage::files('screens'), $keep));
    }
}
