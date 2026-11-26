<?php

declare(strict_types=1);

namespace Arch;

use PHPat\Selector\Selector;
use PHPat\Test\Attributes\TestRule;
use PHPat\Test\Builder\Rule;
use PHPat\Test\PHPat;

final class DeclarationTest extends BaseTest
{
    #[TestRule]
    public function entitiesAreFinal(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::implements('App\Domain\Contracts\Entity'))
            ->excluding(Selector::isInterface(), Selector::isAbstract())
            ->should()
            ->beFinal()
            ->because('Entities are not extended');
    }

    public function testNotFinal(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::classname('/^App\\\\Domain\\\\(Report|Handler)$/', true))
            ->shouldNotBeFinal();
    }

    public function testAbstract(): iterable
    {
        yield PHPat::rule()->classes(Selector::classname('App\Domain\Base'))->shouldNotBeAbstract();
        yield PHPat::rule()->classes(Selector::classname('App\Domain\Invoice'))->should()->beAbstract();
    }

    public function testNames(): iterable
    {
        yield PHPat::rule()
            ->classes(Selector::inNamespace('App\Http\Controller'))
            ->shouldBeNamed('/Controller$/', true);
        yield PHPat::rule()
            ->classes(Selector::classname('App\Domain\Money'))
            ->should()->beNamed(classname: 'App\Domain\Cash');
    }

    public function testPublicMethods(): iterable
    {
        $subjects = Selector::AnyOf(Selector::classname('App\Domain\Invoice'), Selector::classname('App\Domain\Report'));
        yield PHPat::rule()->classes($subjects)->shouldHaveOnlyOnePublicMethod();
        yield PHPat::rule()->classes($subjects)->shouldHaveOnlyOnePublicMethodNamed('execute');
        yield PHPat::rule()->classes(Selector::classname('App\Domain\Invoice'))->shouldHaveOnlyOnePublicMethodNamed('/^run/', true);
    }

    public function testKinds(): iterable
    {
        yield PHPat::rule()->classes(Selector::inNamespace('App\Domain\Contracts'), Selector::classname('App\Domain\Money'))->shouldBeInterface();
        yield PHPat::rule()->classes(Selector::classname('App\Domain\Status'))->shouldNotBeEnum();
        yield PHPat::rule()->classes(Selector::classname('App\Domain\Money'))->shouldBeEnum();
        yield PHPat::rule()->classes(Selector::OneOf(Selector::isFinal(), Selector::isReadonly()))->shouldBeInvokable();
        yield PHPat::rule()->classes(Selector::classname('App\Domain\Handler'))->shouldNotBeInvokable();
        yield PHPat::rule()->classes(Selector::classname('App\Domain\Money'))->shouldBeReadonly();
        yield PHPat::rule()->classes(Selector::AtLeastCountOf(2, Selector::isFinal(), Selector::implements('App\Domain\Contracts\Named'), Selector::isAttribute()))->shouldNotExist();
        yield PHPat::rule()->classes(Selector::AtMostCountOf(0, Selector::inNamespace('App'), Selector::isStandardClass()))->shouldNotExist();
        yield PHPat::rule()->classes(Selector::Not(Selector::inNamespace('App')))->shouldNotExist();
    }
}
