<?php

declare(strict_types=1);

namespace Arch;

use PHPat\Selector\Selector;
use PHPat\Test\Builder\Rule;
use PHPat\Test\PHPat;

final class OptionsTest
{
    public function test_failure_dependencies(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::classname('App\Domain\Failure'))
            ->canOnlyDependOn()
            ->classes(Selector::inNamespace('App'));
    }
}
