<?php

namespace PunStorageFacadeCallables;

use Illuminate\Support\Facades\Storage;

use function PHPStan\Testing\assertType;

final class Calls
{
    /** @return array<string|null> */
    public function contents(): array
    {
        $contents = array_map(Storage::get(...), Storage::allFiles());
        assertType('array<string|null>', $contents);

        return $contents;
    }
}
