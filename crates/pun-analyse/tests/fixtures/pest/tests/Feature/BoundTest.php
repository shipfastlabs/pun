<?php

use Tests\Box;

beforeEach(function (): void {
    $this->box = new Box();
    \PHPStan\dumpType($this->box);
});

test('reads what beforeEach assigned', function (): void {
    \PHPStan\dumpType($this);
    \PHPStan\dumpType($this->box);
    \PHPStan\dumpType($this->box->name);
    \PHPStan\dumpType($this->brand());
    \PHPStan\dumpType($this->unassigned);
});
