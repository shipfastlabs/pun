<?php

namespace PunVendorViewFeatures;

use Illuminate\Support\ServiceProvider;

class FeaturesServiceProvider extends ServiceProvider
{
    public function boot(): void
    {
        foreach ([
            Features\SupportLayouts::class,
        ] as $feature) {
            $feature::provide();
        }
    }
}
