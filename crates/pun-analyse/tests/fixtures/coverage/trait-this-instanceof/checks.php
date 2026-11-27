<?php

declare(strict_types=1);

namespace TraitThisInstanceof;

interface Publishable
{
    public function isPublished(): bool;
}

trait HasUrl
{
    public function isUrlActive(): bool
    {
        return !($this instanceof Publishable) || $this->isPublished();
    }

    public function onlyThis(): bool
    {
        if ($this instanceof Publishable) {
            return $this->isPublished();
        }

        return false;
    }

    public function staticSelf(): bool
    {
        return $this instanceof self;
    }

    public function other(Page $page): bool
    {
        return $page instanceof Publishable;
    }
}

final class Page implements Publishable
{
    use HasUrl;

    public function __construct(private bool $published)
    {
    }

    public function isPublished(): bool
    {
        return $this->published;
    }

    public function outsideTrait(): bool
    {
        return $this instanceof Publishable;
    }
}

final class Category
{
    use HasUrl;
}

trait OnlyOneUser
{
    public function check(): bool
    {
        return $this instanceof Publishable;
    }
}

final class Single
{
    use OnlyOneUser;
}
