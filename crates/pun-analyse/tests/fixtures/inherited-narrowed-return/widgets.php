<?php

declare(strict_types=1);

namespace InheritedNarrowedReturn;

abstract class Widget
{
    /**
     * @return array<string, mixed>|\Stringable|null
     */
    protected function getOptions(): array|\Stringable|null
    {
        return null;
    }

    /**
     * @return list<int>|string
     */
    public function ids(): array|string
    {
        return '';
    }
}

final class Chart extends Widget
{
    #[\Override]
    protected function getOptions(): array
    {
        return ['legend' => false];
    }

    #[\Override]
    public function ids(): array
    {
        return [1];
    }

    public function first(): string
    {
        return $this->ids()[0];
    }
}
