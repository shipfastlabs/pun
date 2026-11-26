<?php

declare(strict_types=1);

namespace Arch;

use PHPat\Selector\Selector;
use PHPat\Test\Builder\Rule;
use PHPat\Test\PHPat;

abstract class BaseTest
{
    public function testInheritedRule(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::withFilepath('/Money\.php$/', true))
            ->shouldNotBeReadonly();
    }
}
