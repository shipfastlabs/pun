<?php

test('is bound to no test case of its own', function (): void {
    \PHPStan\dumpType($this);
    $this->brand();
});
