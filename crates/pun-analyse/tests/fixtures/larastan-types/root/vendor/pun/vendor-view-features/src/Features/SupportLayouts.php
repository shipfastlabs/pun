<?php

namespace PunVendorViewFeatures\Features;

use Illuminate\View\View;

class SupportLayouts
{
    public static function provide(): void
    {
        static::registerLayoutViewMacros();
    }

    public static function registerLayoutViewMacros(): void
    {
        View::macro('punLayout', function ($view, $params = []): static {
            return $this;
        });
    }
}
