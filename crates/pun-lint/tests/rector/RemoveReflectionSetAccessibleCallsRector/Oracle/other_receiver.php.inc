<?php

final class Access
{
    public function setAccessible(bool $enabled): void {}
}

function access(): Access
{
    return new Access();
}

function toggle(mixed $unknown): void
{
    $custom = access();
    $custom->setAccessible(true);
    $unknown->setAccessible(true);
}
