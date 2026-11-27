<?php

declare(strict_types=1);

namespace PunInheritedCasts;

use Illuminate\Database\Eloquent\Model;

trait HasFlags
{
    /** @var array */
    protected $flags = [];
}

class BaseModel extends Model
{
    use HasFlags;
}

abstract class MiddleModel extends BaseModel
{
}

final class Event extends BaseModel
{
    protected $casts = [
        'payload' => 'array',
    ];

    protected $flags = ['a'];
}

final class Deep extends MiddleModel
{
    protected $casts = [
        'payload' => 'array',
    ];

    protected $flags = ['b'];
}

final class Direct extends Model
{
    protected $casts = [
        'payload' => 'array',
    ];
}
