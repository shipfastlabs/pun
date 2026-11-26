<?php

// The declarations of pestphp/pest 4.1.0 and phpunit/phpunit 12 that the fixture uses, trimmed to their signatures.

namespace PHPUnit\Framework {
    abstract class TestCase
    {
        public function assertTrue(mixed $condition): void
        {
        }
    }
}

namespace Pest\PendingCalls {
    /**
     * @mixin \PHPUnit\Framework\TestCase
     */
    final class TestCall
    {
    }

    final class BeforeEachCall
    {
    }
}

namespace Pest {
    final class Configuration
    {
        public function extend(string ...$classAndTraits): self
        {
            return $this;
        }

        public function in(string ...$targets): self
        {
            return $this;
        }
    }
}

namespace {
    use Pest\Configuration;
    use Pest\PendingCalls\BeforeEachCall;
    use Pest\PendingCalls\TestCall;

    function pest(): Configuration
    {
        return new Configuration();
    }

    /**
     * @param-closure-this TestCall  $closure
     */
    function beforeEach(?Closure $closure = null): BeforeEachCall
    {
        return new BeforeEachCall();
    }

    /**
     * @param-closure-this TestCall  $closure
     */
    function test(?string $description = null, ?Closure $closure = null): TestCall
    {
        return new TestCall();
    }
}
