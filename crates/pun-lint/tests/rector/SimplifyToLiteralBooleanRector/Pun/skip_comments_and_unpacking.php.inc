<?php

test('keeps comments and unpacked arguments', function (array $values) {
    expect($values)->toBe(/* preserve the comment */ []);
    expect($values)->toBe(...[]);
});
