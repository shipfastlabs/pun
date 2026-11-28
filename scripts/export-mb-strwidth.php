<?php

declare(strict_types=1);

/*
 * The characters `mb_strwidth` counts 2 wide (every other one counts 1) on the PHP that runs this, as the ranges of
 * crate pun-packages's `php::WIDE`: Symfony's console measures its table cells with it (`Helper::width`).
 *
 *   php scripts/export-mb-strwidth.php    (prints the Rust constant)
 */

$ranges = [];
$start = null;
for ($c = 0; $c <= 0x10FFFF; $c++) {
    $wide = !($c >= 0xD800 && $c <= 0xDFFF) && mb_strwidth(mb_chr($c, 'UTF-8'), 'UTF-8') === 2;
    if ($wide && $start === null) {
        $start = $c;
    } elseif (!$wide && $start !== null) {
        $ranges[] = [$start, $c - 1];
        $start = null;
    }
}
echo '/// The characters `mb_strwidth` counts 2 wide on PHP ', PHP_VERSION, " (`scripts/export-mb-strwidth.php`).\n";
echo "const WIDE: [(u32, u32); ", count($ranges), "] = [\n";
foreach ($ranges as [$a, $b]) {
    printf("    (0x%X, 0x%X),\n", $a, $b);
}
echo "];\n";
