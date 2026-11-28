<?php

function invoke(ReflectionClass $class, object $instance): void
{
    $property = $class->getProperty('name');
    $property->setAccessible(true);
    $property->getValue($instance);

    $method = $class->getMethod('run');
    $method->setAccessible(false);
    $method->invoke($instance);
}
-----
<?php

function invoke(ReflectionClass $class, object $instance): void
{
    $property = $class->getProperty('name');
    $property->getValue($instance);

    $method = $class->getMethod('run');
    $method->invoke($instance);
}
