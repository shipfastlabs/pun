<?php

namespace App;

function unreachable(array $haystack, string $needle): bool
{
    while (true) {
    }

    return in_array($needle, $haystack, true);
}

/**
 * @param list<mixed> $tokens
 */
function afterLoop(array $tokens, int $index): bool
{
    while (!is_string($tokens[$index]) || $tokens[$index] !== '(') {
        $index++;
    }

    $depth = 0;

    do {
        $depth += match ($tokens[$index]) {
            '(' => 1,
            ')' => -1,
            default => 0,
        };
        $index++;
    } while ($depth > 0);

    return in_array($tokens[$index], ['{', ';'], true) || in_array($tokens[$index], ['{', ';']);
}
-----
33: Call to function in_array() requires parameter #3 to be set.
