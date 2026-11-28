<?php

declare(strict_types=1);

namespace PunPivotOverride;

use Illuminate\Database\Eloquent\Relations\Pivot;

final class Membership extends Pivot
{
    #[\Override]
    protected function setKeysForSelectQuery($query)
    {
        parent::setKeysForSelectQuery($query);

        return $query->where('store_id', 1);
    }
}
