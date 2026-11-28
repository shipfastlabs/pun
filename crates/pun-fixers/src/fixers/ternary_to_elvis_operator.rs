use crate::analyzers::range_analyzer::{Range, range_equals_range};
use pun_format::Fixer;
use pun_tokens::{BlockType, Kind::*, Proto, Tokens};
use serde_json::Value;
use std::path::Path;

const VALID_BEFORE_ENDTYPES: &[Proto<'static>] = &[
    Proto::Char(b"="),
    Proto::Kind(T_OPEN_TAG),
    Proto::Kind(T_OPEN_TAG_WITH_ECHO),
    Proto::Char(b"("),
    Proto::Char(b","),
    Proto::Char(b";"),
    Proto::Char(b"["),
    Proto::Char(b"{"),
    Proto::Char(b"}"),
    Proto::Kind(CT_ARRAY_INDEX_BRACE_OPEN),
    Proto::Kind(T_AND_EQUAL),
    Proto::Kind(T_CONCAT_EQUAL),
    Proto::Kind(T_DIV_EQUAL),
    Proto::Kind(T_MINUS_EQUAL),
    Proto::Kind(T_MOD_EQUAL),
    Proto::Kind(T_MUL_EQUAL),
    Proto::Kind(T_OR_EQUAL),
    Proto::Kind(T_PLUS_EQUAL),
    Proto::Kind(T_POW_EQUAL),
    Proto::Kind(T_SL_EQUAL),
    Proto::Kind(T_SR_EQUAL),
    Proto::Kind(T_XOR_EQUAL),
];

/// The tokens after which a ternary's condition also starts, where Rector's `TernaryToElvisRector` rewrites it and
/// `VALID_BEFORE_ENDTYPES` stops too late: `?` and `:` are the outer ternary's in `$c ? $a ? $a : 1 : 2` and the colon
/// of a `case`, a label or the alternative syntax.
const RECTOR_BEFORE_ENDTYPES: &[Proto<'static>] = &[
    Proto::Char(b"?"),
    Proto::Char(b":"),
    Proto::Kind(CT_ARRAY_BRACKET_OPEN),
    Proto::Kind(CT_DYNAMIC_CLASS_CONSTANT_FETCH_BRACE_OPEN),
    Proto::Kind(CT_DYNAMIC_PROP_BRACE_OPEN),
    Proto::Kind(CT_DYNAMIC_VAR_BRACE_OPEN),
    Proto::Kind(CT_NAMED_ARGUMENT_COLON),
    Proto::Kind(T_CASE),
    Proto::Kind(T_COALESCE_EQUAL),
    Proto::Kind(T_DO),
    Proto::Kind(T_DOLLAR_OPEN_CURLY_BRACES),
    Proto::Kind(T_DOUBLE_ARROW),
    Proto::Kind(T_ECHO),
    Proto::Kind(T_ELLIPSIS),
    Proto::Kind(T_ELSE),
    Proto::Kind(T_INCLUDE),
    Proto::Kind(T_INCLUDE_ONCE),
    Proto::Kind(T_LOGICAL_AND),
    Proto::Kind(T_LOGICAL_OR),
    Proto::Kind(T_LOGICAL_XOR),
    Proto::Kind(T_PRINT),
    Proto::Kind(T_REQUIRE),
    Proto::Kind(T_REQUIRE_ONCE),
    Proto::Kind(T_RETURN),
    Proto::Kind(T_THROW),
    Proto::Kind(T_YIELD),
    Proto::Kind(T_YIELD_FROM),
];

#[derive(Default)]
struct TernaryToElvisOperatorFixer {
    /// Pun's `all_contexts`: also a ternary whose condition starts after `RECTOR_BEFORE_ENDTYPES` or after the `)` of a
    /// control structure without braces (`if ($c) $a ? $a : f();`), as Rector's `TernaryToElvisRector` rewrites it.
    all_contexts: bool,
}

pub(crate) fn fixer() -> Box<dyn Fixer> {
    Box::new(TernaryToElvisOperatorFixer::default())
}

impl Fixer for TernaryToElvisOperatorFixer {
    fn name(&self) -> &'static str {
        "ternary_to_elvis_operator"
    }
    fn priority(&self) -> i32 {
        2
    }
    fn is_risky(&self) -> bool {
        true
    }
    fn configure(&mut self, options: &Value) -> anyhow::Result<()> {
        let contexts = match options {
            Value::Null => None,
            Value::Object(o) if o.len() == 1 => o.get("all_contexts"),
            _ => anyhow::bail!("[{}] Is not configurable.", self.name()),
        };
        self.all_contexts = match contexts {
            None if !options.is_null() => anyhow::bail!("[{}] Is not configurable.", self.name()),
            None => false,
            Some(Value::Bool(b)) => *b,
            Some(v) => anyhow::bail!("[{}] all_contexts must be a bool, not {v}", self.name()),
        };
        Ok(())
    }
    fn is_candidate(&self, tokens: &Tokens) -> bool {
        tokens.is_token_kind_found("?")
    }
    fn fix(&self, _file: &Path, tokens: &mut Tokens) {
        let mut index = tokens.count() as isize - 5;
        while index > 1 {
            let i = index as usize;
            index -= 1;
            if !tokens[i].equals("?", true) {
                continue;
            }
            let next_index = tokens.next_meaningful(i);
            if tokens[next_index].equals(":", true) {
                continue;
            }
            let Some(before_operator) = get_before_operator(tokens, i, self.all_contexts) else {
                continue;
            };
            let after_operator = get_after_operator(tokens, i);
            if range_equals_range(tokens, before_operator, after_operator) {
                clear_meaningful_from_range(tokens, after_operator);
            }
        }
    }
}

fn get_before_operator(tokens: &Tokens, index: usize, all_contexts: bool) -> Option<Range> {
    let prev = |i: usize| tokens.prev_meaningful(i);
    let mut index = prev(index);
    let end = index;
    let mut start = None;
    while !tokens[index].equals_any(VALID_BEFORE_ENDTYPES, true)
        && !(all_contexts && starts_after(tokens, index))
    {
        if tokens[index].is_given_kind(&[T_INC, T_DEC]) {
            return None;
        }
        let detected = Tokens::detect_block_type(&tokens[index]);
        let Some((block_type, false)) = detected else {
            start = Some(index);
            index = prev(index);
            continue;
        };
        let (block_start, block_end) = Tokens::block_edges(block_type);
        let mut open_count = 1;
        loop {
            index = prev(index);
            if tokens[index].is_given_kind(&[T_INC, T_DEC]) {
                return None;
            }
            if tokens[index].equals(block_start, true) {
                open_count += 1;
            } else if tokens[index].equals(block_end, true) {
                open_count -= 1;
            }
            if open_count > 1 {
                break;
            }
        }
        start = Some(index);
        index = prev(index);
    }
    Some(Range { start: start?, end })
}

/// Whether a ternary's condition starts after the token at `index` in Rector's `TernaryToElvisRector` too.
fn starts_after(tokens: &Tokens, index: usize) -> bool {
    if tokens[index].equals(")", true) {
        let open = tokens.find_block_start(BlockType::Parenthesis, index);
        return tokens[tokens.prev_meaningful(open)]
            .is_given_kind(&[T_IF, T_ELSEIF, T_WHILE, T_FOR, T_FOREACH, T_DECLARE]);
    }
    tokens[index].equals_any(RECTOR_BEFORE_ENDTYPES, true)
}

fn get_after_operator(tokens: &Tokens, index: usize) -> Range {
    let mut index = tokens.next_meaningful(index);
    let start = index;
    loop {
        if let Some((block_type, _)) = Tokens::detect_block_type(&tokens[index]) {
            index = tokens.find_block_end(block_type, index);
        }
        let end = index;
        index = tokens.next_meaningful(index);
        if tokens[index].equals(":", true) {
            return Range { start, end };
        }
    }
}

fn clear_meaningful_from_range(tokens: &mut Tokens, range: Range) {
    let mut i = range.end;
    while i >= range.start {
        tokens.clear_token_and_merge_surrounding_whitespace(i);
        i = tokens.prev_meaningful(i);
    }
}

#[cfg(test)]
mod tests {
    /// The fixer, then `ternary_operator_spaces`, which turns the `?  :` it leaves into `?:`, as Pint runs them.
    fn fix(options: &serde_json::Value, code: &str) -> String {
        let mut fixer = super::fixer();
        fixer.configure(options).unwrap();
        let fixed = crate::fix_with(&*fixer, code);
        crate::fix_with(&*crate::ternary_operator_spaces::fixer(), &fixed)
    }

    /// With `all_contexts`, the expectation is what Rector 2.8.4's `TernaryToElvisRector` writes for the same input,
    /// except `$a++ ? $a++ : 1` and `--$a ? --$a : 1`, which Pun leaves since `?:` runs the increment once. Without it,
    /// the expectation is what PHP-CS-Fixer 3.95.25 writes with `ternary_operator_spaces`.
    #[test]
    fn all_contexts_rewrite_what_rector_rewrites() {
        let before = "<?php
function f($a, $b, $c, $o)
{
    echo $a ? $a : 'x';
    print $a ? $a : 'x';
    yield $a ? $a : 'x';
    yield $b => $a ? $a : 'x';
    yield from $a ? $a : [];
    $x = [$a ? $a : 'x', 'k' => $a ? $a : 'x'];
    $x = match ($b) { 1 => $a ? $a : 'x', default => null };
    $x = fn () => $a ? $a : 'x';
    $x ??= $a ? $a : 'x';
    $x = $b and $a ? $a : 'x';
    $x = $c ? $a ? $a : 1 : 2;
    foo(...$a ? $a : [], name: $a ? $a : 'x');
    $x = $o->{$a ? $a : 'x'} . ${$a ? $a : 'x'} . Foo::{$a ? $a : 'x'} . \"${$a ? $a : 'x'}\";
    include $a ? $a : 'x.php';
    require_once $a ? $a : 'x.php';
    if ($c) $a ? $a : foo(); elseif ($b) $a ? $a : foo(); else $a ? $a : foo();
    while ($c) $a ? $a : foo();
    for ($i = 0; $i < 3; $i++) $a ? $a : foo();
    foreach ($o as $v) $a ? $a : foo();
    do $a ? $a : foo(); while ($c);
    declare(ticks=1) $a ? $a : foo();
    switch ($b) { case $a ? $a : 1: $a ? $a : foo(); default: $a ? $a : foo(); }
    if ($c): $a ? $a : foo(); endif;
    label: $a ? $a : foo();
    if ($c) { throw $a ? $a : new Exception(); }
    $x = $a ? $a : 'x';
    $x = $b . $a ? $a : 1;
    if ($c) foo($a) ? $a : foo();
    $x = foo($b) - $a ? - $a : 1;
    echo $a++ ? $a++ : 1;
    return --$a ? --$a : 1;
}
";
        let after = "<?php
function f($a, $b, $c, $o)
{
    echo $a ?: 'x';
    print $a ?: 'x';
    yield $a ?: 'x';
    yield $b => $a ?: 'x';
    yield from $a ?: [];
    $x = [$a ?: 'x', 'k' => $a ?: 'x'];
    $x = match ($b) { 1 => $a ?: 'x', default => null };
    $x = fn () => $a ?: 'x';
    $x ??= $a ?: 'x';
    $x = $b and $a ?: 'x';
    $x = $c ? $a ?: 1 : 2;
    foo(...$a ?: [], name: $a ?: 'x');
    $x = $o->{$a ?: 'x'} . ${$a ?: 'x'} . Foo::{$a ?: 'x'} . \"${$a ?: 'x'}\";
    include $a ?: 'x.php';
    require_once $a ?: 'x.php';
    if ($c) $a ?: foo(); elseif ($b) $a ?: foo(); else $a ?: foo();
    while ($c) $a ?: foo();
    for ($i = 0; $i < 3; $i++) $a ?: foo();
    foreach ($o as $v) $a ?: foo();
    do $a ?: foo(); while ($c);
    declare(ticks=1) $a ?: foo();
    switch ($b) { case $a ?: 1: $a ?: foo(); default: $a ?: foo(); }
    if ($c): $a ?: foo(); endif;
    label: $a ?: foo();
    if ($c) { throw $a ?: new Exception(); }
    $x = $a ?: 'x';
    $x = $b . $a ? $a : 1;
    if ($c) foo($a) ? $a : foo();
    $x = foo($b) - $a ? - $a : 1;
    echo $a++ ? $a++ : 1;
    return --$a ? --$a : 1;
}
";
        let php_cs_fixer = "<?php
function f($a, $b, $c, $o)
{
    echo $a ? $a : 'x';
    print $a ? $a : 'x';
    yield $a ? $a : 'x';
    yield $b => $a ? $a : 'x';
    yield from $a ? $a : [];
    $x = [$a ? $a : 'x', 'k' => $a ? $a : 'x'];
    $x = match ($b) { 1 => $a ? $a : 'x', default => null };
    $x = fn () => $a ? $a : 'x';
    $x ??= $a ? $a : 'x';
    $x = $b and $a ? $a : 'x';
    $x = $c ? $a ? $a : 1 : 2;
    foo(...$a ? $a : [], name: $a ? $a : 'x');
    $x = $o->{$a ? $a : 'x'} . ${$a ? $a : 'x'} . Foo::{$a ?: 'x'} . \"${$a ? $a : 'x'}\";
    include $a ? $a : 'x.php';
    require_once $a ? $a : 'x.php';
    if ($c) $a ? $a : foo(); elseif ($b) $a ? $a : foo(); else $a ? $a : foo();
    while ($c) $a ? $a : foo();
    for ($i = 0; $i < 3; $i++) $a ? $a : foo();
    foreach ($o as $v) $a ? $a : foo();
    do $a ? $a : foo(); while ($c);
    declare(ticks=1) $a ? $a : foo();
    switch ($b) { case $a ? $a : 1: $a ? $a : foo(); default: $a ? $a : foo(); }
    if ($c): $a ? $a : foo(); endif;
    label: $a ? $a : foo();
    if ($c) { throw $a ? $a : new Exception(); }
    $x = $a ?: 'x';
    $x = $b . $a ? $a : 1;
    if ($c) foo($a) ? $a : foo();
    $x = foo($b) - $a ? - $a : 1;
    echo $a++ ? $a++ : 1;
    return --$a ? --$a : 1;
}
";
        assert_eq!(fix(&serde_json::json!({ "all_contexts": true }), before), after);
        assert_eq!(fix(&serde_json::Value::Null, before), php_cs_fixer);
    }

    #[test]
    fn options() {
        let mut fixer = super::fixer();
        for (options, error) in [
            (serde_json::json!({ "foo": true }), "[ternary_to_elvis_operator] Is not configurable."),
            (
                serde_json::json!({ "all_contexts": 1 }),
                "[ternary_to_elvis_operator] all_contexts must be a bool, not 1",
            ),
            (serde_json::json!(1), "[ternary_to_elvis_operator] Is not configurable."),
        ] {
            assert_eq!(fixer.configure(&options).unwrap_err().to_string(), error);
        }
    }
}
