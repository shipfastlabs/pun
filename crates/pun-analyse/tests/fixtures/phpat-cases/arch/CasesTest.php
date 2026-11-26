<?php

namespace Arch;

use PHPat\Selector\Selector;
use PHPat\Test\Builder\Rule;
use PHPat\Test\PHPat;

final class CasesTest
{
    public function testRegex(): iterable
    {
        yield 'namespace' => PHPat::rule()
            ->classes(Selector::inNamespace('/^App\\\\Cases$/', true))
            ->excluding(Selector::isFinal(), Selector::isInterface(), Selector::isTrait())
            ->shouldBeFinal();
        yield 'filepath' => PHPat::rule()->classes(Selector::withFilepath('src/Cases/Child.php'))->shouldBeFinal();
        yield 'implements' => PHPat::rule()->classes(Selector::implements('/Contract$/', true))->shouldBeFinal();
        yield 'extends' => PHPat::rule()->classes(Selector::extends('/Base$/', true))->shouldBeFinal();
        yield 'attribute' => PHPat::rule()->classes(Selector::appliesAttribute('/Marker$/', true))->shouldBeFinal();
        yield 'includes' => PHPat::rule()->classes(Selector::includes('/Inner2$/', true))->shouldBeFinal();
    }

    public function testTraits(): iterable
    {
        yield 'parent' => PHPat::rule()
            ->classes(Selector::includes('App\Cases\Shared'))
            ->excluding(Selector::isAbstract())
            ->shouldBeFinal();
        yield 'nested' => PHPat::rule()->classes(Selector::includes('App\Cases\Inner2'))->shouldBeFinal();
    }

    public function testDependencies(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::classname('App\Cases\Child'))
            ->shouldNotDependOn()
            ->classes(Selector::inNamespace('App\Cases'), Selector::classname('RuntimeException'));
    }
}
