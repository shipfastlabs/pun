<?php

declare(strict_types=1);

namespace SprintfLiteralStrings;

use function PHPStan\dumpType;

final class Sql
{
    private const string TABLE = 'orders';

    private const string QUERY = 'SELECT * FROM %s';

    /** @param literal-string $sql */
    public function run(string $sql): void
    {
    }

    public function fromConstants(): void
    {
        dumpType(sprintf(self::QUERY, self::TABLE));
        $this->run(sprintf(self::QUERY, self::TABLE));
    }

    public function concatenated(): void
    {
        $sql = 'WITH '.sprintf(self::QUERY, self::TABLE);
        dumpType($sql);
        $this->run("({$sql}) AS o");
    }

    public function dynamic(string $table): void
    {
        dumpType(sprintf('%d-%s%%', 5, 'x'));
        dumpType(sprintf('%s', $table));
        $this->run(sprintf('%s', $table));
    }
}
