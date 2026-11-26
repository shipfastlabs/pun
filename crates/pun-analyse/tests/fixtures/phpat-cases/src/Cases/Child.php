<?php

namespace App\Cases;

/**
 * @method Dep build(Other $other, $untyped)
 * @method helper()
 * @property-read Dep $read
 * @property-write Dep $written
 */
#[Marker]
class Child extends Base implements Contract
{
    use Inner1, Inner2, HasDep, Shared;

    public Dep $hooked {
        get => new Dep();
    }

    /**
     * @deprecated
     * @throws \RuntimeException
     */
    public function run(self $me, parent $base, (Contract&Other)|null $both): int
    {
        parent::x();
        $name = Dep::class;
        new $name();
        $make = Dep::make(...);
        if ($make instanceof \Closure) {
            function nested(Other $other): Dep
            {
                return new Dep();
            }
        }
        $anonymous = new class () {
            public function a(): Dep
            {
                return new Dep();
            }
        };

        return 1 + 2;
    }
}
