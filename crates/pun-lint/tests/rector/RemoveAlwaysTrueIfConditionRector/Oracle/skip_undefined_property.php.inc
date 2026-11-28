<?php

declare(strict_types=1);

abstract class UndefinedPropertyParent
{
}

/**
 * @property-read stdClass|null $user
 */
final class UndefinedPropertyGuard extends UndefinedPropertyParent
{
    public ?string $description = null;

    public function load(): void
    {
        $user = $this->user;

        if ($user === null) {
            return;
        }

        $this->description = 'loaded';
    }

    public function loadWithoutTag(): void
    {
        $user = $this->missing;

        if ($user === null) {
            return;
        }

        $this->description = 'loaded';
    }
}
