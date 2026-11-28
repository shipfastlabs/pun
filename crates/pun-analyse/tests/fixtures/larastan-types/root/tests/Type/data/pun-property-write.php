<?php

declare(strict_types=1);

namespace PunPropertyWrite;

use Illuminate\Database\Eloquent\Model;

use function PHPStan\Testing\assertType;

/**
 * @property-read string|null $title
 * @property-write array<string, string>|string|null $title
 */
final class Module extends Model
{
}

function rename(Module $module): void
{
    $module->title = ['en' => 'Projects', 'nl' => 'Projecten'];
    $module->title = 1;
    assertType('string|null', $module->title);
}

/** @template TModel of Model = Model */
class Page
{
    /** @var ?TModel */
    public ?Model $record = null;
}

final class Product extends Model
{
}

/** @property Product $record */
abstract class CreateProduct extends Page
{
    protected function afterCreate(): void
    {
        assertType('PunPropertyWrite\Product', $this->record);
    }
}
