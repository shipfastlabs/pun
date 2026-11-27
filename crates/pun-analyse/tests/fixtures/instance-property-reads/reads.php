<?php

namespace InstancePropertyReads;

class Item {}

abstract class Plain {}

final class WithCount
{
    public int $count = 0;
}

final class WithoutCount {}

abstract class Magic
{
    public function __get(string $name): mixed
    {
        return null;
    }
}

final class Holder extends Plain
{
    public ?Item $nullable = null;

    public ?Item $other = null;

    public null $nil = null;

    public ?string $out = null;

    public function failedRead(): void
    {
        \PHPStan\dumpType($this->missing);
    }

    /** A guard on a failed read is not always true, so the code after it stays reachable. */
    public function failedGuard(): void
    {
        $item = $this->missing;

        if ($item === null) {
            return;
        }

        $this->out = 'reached';
    }

    public function failedNullsafeRead(?self $holder): void
    {
        \PHPStan\dumpType($holder?->missing);
    }

    public function invalidSelectorRead(bool $flag): void
    {
        $number = 1;
        \PHPStan\dumpType($this->$number);

        $name = $flag ? 'nullable' : 1;
        \PHPStan\dumpType($this->$name);

        $maybe = $flag ? 'nullable' : null;
        \PHPStan\dumpType($this->$maybe);
    }

    /**
     * @template T of 'nullable'|null
     * @param T $name
     */
    public function nullableTemplateRead($name): void
    {
        \PHPStan\dumpType($this->$name);
    }

    /**
     * @template T of 'nullable'
     * @param T $name
     */
    public function literalTemplateRead($name): void
    {
        \PHPStan\dumpType($this->$name);
    }

    public function nullable(int $value): int
    {
        return $value;
    }

    /**
     * A template bounded by a template bounded by one constant string has no constant strings.
     *
     * @template T of 'nullable'
     * @template U of T
     * @param U $name
     */
    public function nestedTemplateName($name): void
    {
        \PHPStan\dumpType($this->$name);
        \PHPStan\dumpType($this->$name('x'));
    }

    /**
     * @template T of 'nullable'|'other'
     * @template U of T
     * @param U $name
     */
    public function nestedUnionTemplateRead($name): void
    {
        \PHPStan\dumpType($this->$name);
    }

    /**
     * @template T of 'nullable'
     * @template U of T|'other'
     * @param U $name
     */
    public function widenedTemplateRead($name): void
    {
        \PHPStan\dumpType($this->$name);
    }

    /**
     * @template T of 'nullable'
     * @template U of T
     * @template V of U
     * @param V $name
     */
    public function threeLevelTemplateRead($name): void
    {
        \PHPStan\dumpType($this->$name);
    }

    /**
     * @template T of Item::class
     * @template U of T
     * @param U $name
     */
    public function classStringBoundCall($name): void
    {
        \PHPStan\dumpType($this->$name());
    }

    /**
     * @template T of 'nullable'
     * @template U of T
     * @param U|'nullable' $name
     */
    public function widenedTemplateCall($name): void
    {
        \PHPStan\dumpType($this->$name(1));
    }

    /**
     * @template T of string
     * @template U of T&'nullable'
     * @param U $name
     */
    public function intersectionTemplateCall($name): void
    {
        $this->$name('x');
    }

    /**
     * A benevolent union drops a member without constant strings unless it is a string.
     *
     * @template TString of string
     * @template TNested of TString
     * @template TAny
     * @param __benevolent<'nullable'|null> $withNull
     * @param __benevolent<'nullable'|array-key> $withArrayKey
     * @param __benevolent<'nullable'|TString> $withStringTemplate
     * @param __benevolent<'nullable'|TNested> $withNestedTemplate
     * @param __benevolent<'nullable'|TAny> $withUnboundedTemplate
     * @param __benevolent<'a'|array-key> $repeated
     * @param __benevolent<'a'|TAny> $repeatedTemplate
     * @param __benevolent<'a'|TString> $repeatedStringTemplate
     * @param __benevolent<'a'|(TAny&string)> $repeatedIntersection
     * @param __benevolent<'nullable'|(TAny&string)> $withIntersection
     */
    public function benevolentNames($withNull, $withArrayKey, $withStringTemplate, $withNestedTemplate, $withUnboundedTemplate, $repeated, $repeatedTemplate, $repeatedStringTemplate, $repeatedIntersection, $withIntersection): void
    {
        \PHPStan\dumpType($this->$withNull(1));
        \PHPStan\dumpType($this->$withArrayKey(1));
        \PHPStan\dumpType($this->$withStringTemplate(1));
        \PHPStan\dumpType($this->$withNestedTemplate(1));
        \PHPStan\dumpType($this->$withUnboundedTemplate(1));
        \PHPStan\dumpType(str_repeat($repeated, 2));
        \PHPStan\dumpType(str_repeat($repeatedTemplate, 2));
        \PHPStan\dumpType(str_repeat($repeatedStringTemplate, 2));
        \PHPStan\dumpType(str_repeat($repeatedIntersection, 2));
        \PHPStan\dumpType($this->$withIntersection(1));
    }

    public function classStringMethodCall(): void
    {
        $method = Item::class;
        $this->$method();
    }

    public function partialUnionRead(WithCount|WithoutCount $counter): void
    {
        \PHPStan\dumpType($counter->count);
    }

    public function nullableRead(): void
    {
        \PHPStan\dumpType($this->nullable);
    }

    public function nullRead(): void
    {
        $item = $this->nil;

        if ($item === null) {
            return;
        }

        $this->out = 'unreachable';
    }
}

/**
 * @template C of 'nullable'
 */
final class Bounded
{
    public ?Item $nullable = null;

    /**
     * @template U of C
     * @param U $name
     */
    public function classTemplateName($name): void
    {
        \PHPStan\dumpType($this->$name);
    }
}

final class Statics
{
    public const NAME = 'nullable';

    public static ?Item $nullable = null;

    public static function nullable(int $value): int
    {
        return $value;
    }

    /**
     * @template T of 'nullable'
     * @template U of T
     * @param U $name
     */
    public function nestedTemplateStatics($name): void
    {
        \PHPStan\dumpType(self::${$name});
        \PHPStan\dumpType(self::$name('x'));
    }

    /**
     * @template T of 'MISSING'
     * @template U of T
     * @param U $name
     */
    public function nestedTemplateConstant($name): void
    {
        \PHPStan\dumpType(self::{$name});
    }

    /**
     * @template T of '/missing/path.php'
     * @template U of T
     * @param U $path
     */
    public function nestedTemplateInclude($path): void
    {
        include $path;
    }

    public function nullableStaticCallable(bool $flag): void
    {
        $name = $flag ? 'nullable' : null;
        \PHPStan\dumpType(self::$name(...));

        $known = 'nullable';
        \PHPStan\dumpType(self::$known(...));
    }

    public function nullableStaticName(bool $flag): void
    {
        $name = $flag ? 'nullable' : null;
        \PHPStan\dumpType(self::${$name});
    }
}

/**
 * A template's intersection members sit beside it, as in PHPStan's IntersectionType: their constant strings, lengths
 * and string flags count.
 */
final class Intersected
{
    /**
     * @template T
     * @template U of T&'a'
     * @template S of string
     * @param T&class-string $classString
     * @param T&true $true
     * @param T&1.5 $float
     * @param U $nested
     * @param S&class-string $stringClassString
     * @param T&'ab' $literal
     * @param __benevolent<'a'|(T&'a')> $benevolent
     * @param __benevolent<'a'|(S&'b')> $benevolentString
     * @param int<1, max> $times
     */
    public function measure($classString, $true, $float, $nested, $stringClassString, $literal, $benevolent, $benevolentString, int $times): void
    {
        \PHPStan\dumpType(strlen($classString));
        \PHPStan\dumpType(str_repeat($classString, $times));
        \PHPStan\dumpType(strlen($true));
        \PHPStan\dumpType(strlen($float));
        \PHPStan\dumpType(strlen($nested));
        \PHPStan\dumpType(str_repeat($nested, 2));
        \PHPStan\dumpType(strlen($stringClassString));
        \PHPStan\dumpType(str_repeat($stringClassString, $times));
        \PHPStan\dumpType(str_repeat($literal, $times));
        \PHPStan\dumpType(str_repeat($benevolent, 2));
        \PHPStan\dumpType(str_repeat($benevolentString, 2));
    }

    /**
     * Members reached through a template's bound are still concrete types.
     *
     * @template T
     * @template UTrue of T&true
     * @template UInt of T&123
     * @template ULiteral of T&'a'
     * @template VLiteral of ULiteral
     * @param UTrue $true
     * @param UInt $int
     * @param ULiteral $literal
     * @param VLiteral $deeper
     * @param int<1, max> $times
     */
    public function nested($true, $int, $literal, $deeper, int $times): void
    {
        \PHPStan\dumpType(strlen($true));
        \PHPStan\dumpType(strlen($int));
        \PHPStan\dumpType(strlen($literal));
        \PHPStan\dumpType(str_repeat($literal, $times));
        \PHPStan\dumpType(strlen($deeper));
        \PHPStan\dumpType(str_repeat($deeper, 2));
    }
}

/** @property-read Item|null $item */
final class Computed extends Magic
{
    public ?string $out = null;

    public function magicGuard(): void
    {
        $item = $this->item;

        if ($item === null) {
            return;
        }

        $this->out = 'reached';
    }
}
