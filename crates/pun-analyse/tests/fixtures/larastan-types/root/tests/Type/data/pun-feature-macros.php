<?php

declare(strict_types=1);

namespace PunFeatureMacros;

use Illuminate\View\View;

use function PHPStan\Testing\assertType;

function page(View $view): View
{
    assertType('Illuminate\View\View', $view->punLayout('layouts.app'));

    return $view->punLayout('layouts.app');
}
