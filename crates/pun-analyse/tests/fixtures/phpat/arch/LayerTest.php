<?php

declare(strict_types=1);

namespace Arch;

use App\Domain\Customer;
use App\Domain\Order;
use PHPat\Selector\Selector;
use PHPat\Test\Builder\Rule;
use PHPat\Test\PHPat;

final class LayerTest
{
    private const DOMAIN = 'App\Domain';

    public function test_domain_does_not_depend_on_infra(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::inNamespace(self::DOMAIN))
            ->excluding(Selector::classname(Customer::class))
            ->shouldNotDependOn()
            ->classes(Selector::inNamespace('App\Infra'))
            ->because('The domain stays free of infrastructure');
    }

    public function testCustomerDependencies(): Rule
    {
        $infra = Selector::inNamespace('App\Infra');

        return PHPat::rule()
            ->classes(Selector::classname(Customer::class))
            ->canOnly()
            ->dependOn()
            ->classes(Selector::inNamespace(self::DOMAIN), $infra)
            ->excluding(Selector::classname('/^App\\\\Infra\\\\Db/', true))
            ->because('one', 'two');
    }

    public function testControllersDependOnDomain(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::classname('App\Http\Controller\Helper'))
            ->should()
            ->dependOn()
            ->classes(Selector::classname(Order::class), Selector::classname('App\Infra\Clock'));
    }

    public function testNothingConstructsTheDb(): Rule
    {
        return PHPat::rule()
            ->classes(Selector::all())
            ->excluding(Selector::inNamespace('App\Infra'))
            ->shouldNotConstruct()
            ->classes(Selector::classname('App\Infra\Db'), Selector::extends('Exception'));
    }

    public function testExtendsAndImplements(): iterable
    {
        yield 'extend' => PHPat::rule()
            ->classes(Selector::AllOf(Selector::inNamespace('App\Domain'), Selector::NoneOf(Selector::isInterface(), Selector::isEnum(), Selector::isTrait(), Selector::isException())))
            ->excluding(Selector::classname('App\Domain\Base'))
            ->shouldExtend()
            ->classes(Selector::classname('App\Domain\Base'));
        yield PHPat::rule()
            ->classes(Selector::extends('App\Domain\Base'))
            ->shouldImplement()
            ->classes(Selector::classname('App\Domain\Contracts\Named'));
        yield 'not.implement' => PHPat::rule()
            ->classes(Selector::isException())
            ->shouldNotImplement()
            ->classes(Selector::classname('Stringable'))
            ->nonIgnorable();
        yield 'not_extend' => PHPat::rule()
            ->classes(Selector::isThrowable())
            ->should()->extend()
            ->classes(Selector::classname('LogicException'));
    }

    public function testTraitsAndAttributes(): array
    {
        return [
            'include' => PHPat::rule()
                ->classes(Selector::includes('App\Domain\Timestamps'))
                ->shouldNotInclude()
                ->classes(Selector::isTrait()),
            PHPat::rule()
                ->classes(Selector::appliesAttribute('App\Attr\Aggregate'))
                ->shouldApplyAttribute()
                ->classes(Selector::classname('App\Attr\Missing'), Selector::isAttribute()),
            'exceptions' => PHPat::rule()
                ->classes(Selector::isError())
                ->shouldBeFinal(),
        ];
    }

    protected function testNotARule(): Rule
    {
        return PHPat::rule();
    }

    public function helper(): void
    {
    }
}
