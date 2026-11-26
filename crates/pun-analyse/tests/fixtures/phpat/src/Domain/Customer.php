<?php

namespace App\Domain;

use App\Infra\Db;
use App\Infra\DbException;
use App\Infra\Logger;

/**
 * @property Logger $logger
 * @method Db connection()
 * @mixin Db
 */
class Customer extends Base
{
    use Timestamps;

    private ?Db $db = null;

    public function __construct(private Logger $log)
    {
    }

    /**
     * @param Db $db
     * @return Logger
     * @throws DbException
     */
    public function save(Db|Logger $db): Logger
    {
        try {
            Logger::log(Db::NAME);
        } catch (DbException $e) {
        }
        if ($db instanceof Logger) {
            return $db;
        }
        /** @var Db $other */
        $other = $this->db;
        $make = static fn (Logger $l): Logger => $l;

        return $make($this->log);
    }

    public function self(): static
    {
        return new static($this->log);
    }
}
