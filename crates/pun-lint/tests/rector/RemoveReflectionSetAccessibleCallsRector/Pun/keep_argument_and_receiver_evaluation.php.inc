<?php

function toggle(ReflectionClass $class, string $name): void
{
    $property = $class->getProperty($name);
    $property->setAccessible(checkPermission());
    $class->getProperty($name)->setAccessible(true);
}
