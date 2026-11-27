<?php

declare(strict_types=1);

namespace ReflectionParameterLists;

use function PHPStan\dumpType;

final class Signature
{
    /** @return list<string> */
    public function names(\ReflectionMethod $method): array
    {
        return array_map(static fn (\ReflectionParameter $parameter): string => $parameter->getName(), $method->getParameters());
    }

    public function dumps(\ReflectionMethod $method, \ReflectionFunction $function, \ReflectionClass $class): void
    {
        dumpType($method->getParameters());
        dumpType($function->getParameters());
        dumpType($class->getInterfaceNames());
    }
}
