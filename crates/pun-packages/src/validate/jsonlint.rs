//! seld/jsonlint 1.x (as Composer 2.11.2 bundles it): `JsonParser::lint` with no flags, up to the first error, which is
//! where it throws. Its lexer, its LALR table and its messages: the line of the last token it took, the context around
//! the offending one, the tokens it expected, and the hints for quotes, backslashes and trailing commas.

use pcre2::bytes::{Regex, RegexBuilder};
use pun_tokens::php_compat::substr_count;
use std::sync::LazyLock;

/// The message of the first error in `input`, as `ParsingException::getMessage` gives it; `None` for valid JSON.
pub(super) fn lint(input: &[u8]) -> Option<String> {
    if input.starts_with(b"\xEF\xBB\xBF") {
        return Some(
            "BOM detected, make sure your input does not include a Unicode Byte-Order-Mark".into(),
        );
    }
    parse(input).err()
}

/// `Lexer::$rules`, each anchored at the offset (`\G`), in order.
static RULES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\G\s*\n\r?",
        r"\G\s+",
        r"\G-?([0-9]|[1-9][0-9]+)(\.[0-9]+)?([eE][+-]?[0-9]+)?\b",
        r#"\G"(?>\\["bfnrt/\\]|\\u[a-fA-F0-9]{4}|[^\x00-\x1f\\"]++)*+""#,
        r"\G\{",
        r"\G\}",
        r"\G\[",
        r"\G\]",
        r"\G,",
        r"\G:",
        r"\Gtrue\b",
        r"\Gfalse\b",
        r"\Gnull\b",
        r"\G$",
        r"\G\/\/",
        r"\G\/\*",
        r"\G\*\/",
        r"\G.",
    ]
    .iter()
    .map(|r| Regex::new(r).expect("jsonlint's rule"))
    .collect()
});

/// `Lexer`'s token numbers that aren't the parser's terminals.
const EOF: i32 = 1;
const SKIP_WHITESPACE: i32 = 0;
const BREAK_LINE: i32 = 3;
const COMMENT: i32 = 30;
const OPEN_COMMENT: i32 = 31;
const CLOSE_COMMENT: i32 = 32;
const INVALID: i32 = -1;

struct Lexer<'a> {
    input: &'a [u8],
    done: bool,
    offset: usize,
    yylineno: usize,
    /// The text of the token last read.
    matched: Vec<u8>,
}

impl Lexer<'_> {
    /// `Lexer::lex`: the next token, past whitespace; comments are an error, as `ALLOW_COMMENTS` is off.
    fn lex(&mut self) -> Result<i32, String> {
        loop {
            match self.next() {
                SKIP_WHITESPACE | BREAK_LINE => {}
                COMMENT | OPEN_COMMENT => {
                    return Err(format!(
                        "Lexical error on line {}. Comments are not allowed.\n{}",
                        self.yylineno + 1,
                        self.show_position()
                    ));
                }
                CLOSE_COMMENT => {
                    return Err(format!(
                        "Lexical error on line {}. Unexpected token.\n{}",
                        self.yylineno + 1,
                        self.show_position()
                    ));
                }
                symbol => return Ok(symbol),
            }
        }
    }

    /// `Lexer::next`: the first rule that matches at the offset, and the token its action gives. One always does: `$`
    /// at the end, a line break, and `.` for any other byte, so its "Unrecognized text" error never comes.
    fn next(&mut self) -> i32 {
        if self.done {
            return EOF;
        }
        if self.offset == self.input.len() {
            self.done = true;
        }
        self.matched.clear();
        let (rule, m) = (RULES.iter().enumerate())
            .find_map(|(rule, re)| Some((rule, re.find_at(self.input, self.offset).ok()??)))
            .expect("`$`, a line break or `.` matches at any offset");
        let text = &self.input[m.start()..m.end()];
        self.yylineno += substr_count(text, b"\n");
        self.matched.extend_from_slice(text);
        self.offset += text.len();
        match rule {
            0 => BREAK_LINE,
            1 => SKIP_WHITESPACE,
            2 => 6,
            3 => 4,
            4 => 17,
            5 => 18,
            6 => 23,
            7 => 24,
            8 => 22,
            9 => 21,
            10 => 10,
            11 => 11,
            12 => 8,
            13 => 14,
            14 => COMMENT,
            15 => OPEN_COMMENT,
            16 => CLOSE_COMMENT,
            _ => INVALID,
        }
    }

    /// `Lexer::showPosition`: up to 20 bytes before the token, it and what follows, with a caret under its start.
    fn show_position(&self) -> String {
        if self.yylineno == 0 && self.offset == 1 && self.matched != b"{" {
            return format!("{}...\n^", lossy(&self.matched));
        }
        let pre = without_newlines(&self.past_input());
        let dashes = "-".repeat(pre.len().saturating_sub(1));
        format!(
            "{}{}\n{dashes}^",
            lossy(&pre),
            lossy(&without_newlines(&self.upcoming_input()))
        )
    }

    /// `Lexer::getPastInput`: the 20 bytes before the token, after "..." when there are more.
    fn past_input(&self) -> Vec<u8> {
        let past = self.offset - self.matched.len();
        let mut out = if past > 20 { b"...".to_vec() } else { vec![] };
        out.extend_from_slice(
            &self.input[past.saturating_sub(20)..past.saturating_sub(20) + past.min(20)],
        );
        out
    }

    /// `Lexer::getUpcomingInput`: the token and what follows, 20 bytes at most, then "..." when there was more.
    fn upcoming_input(&self) -> Vec<u8> {
        let mut next = self.matched.clone();
        if next.len() < 20 {
            let end = (self.offset + 20 - next.len()).min(self.input.len());
            next.extend_from_slice(&self.input[self.offset.min(end)..end]);
        }
        let mut out = next[..next.len().min(20)].to_vec();
        if next.len() > 20 {
            out.extend_from_slice(b"...");
        }
        out
    }

    /// `Lexer::getFullUpcomingInput`: for a string the lexer couldn't read, the rest of it up to a quote or a newline.
    fn full_upcoming_input(&self) -> Vec<u8> {
        let mut next = self.matched.clone();
        let len = self.input.len();
        if next.first() == Some(&b'"') && substr_count(&next, b"\"") == 1 {
            let end = if self.offset == len {
                len
            } else {
                let find = |c: u8| {
                    (self.input.get(self.offset + 1..).unwrap_or_default().iter())
                        .position(|&b| b == c)
                        .map_or(len, |i| self.offset + 1 + i)
                };
                find(b'"').min(find(b'\n'))
            };
            next.extend_from_slice(&self.input[self.offset..end]);
        } else if next.len() < 20 {
            let end = (self.offset + 20 - next.len()).min(len);
            next.extend_from_slice(&self.input[self.offset.min(end)..end]);
        }
        next
    }
}

fn without_newlines(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().copied().filter(|&b| b != b'\n').collect()
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A table entry: shift to a state, reduce by a production, accept, or (for a nonterminal) go to a state.
#[derive(Clone, Copy)]
enum Act {
    Shift(usize),
    Reduce(usize),
    Accept,
    Goto(usize),
}

use Act::{Accept, Goto, Reduce, Shift};

/// `JsonParser::$table`, each state's entries in the PHP array's order, which the expected tokens follow.
const TABLE: [&[(i32, Act)]; 32] = [
    &[
        (3, Goto(5)),
        (4, Shift(12)),
        (5, Goto(6)),
        (6, Shift(13)),
        (7, Goto(3)),
        (8, Shift(9)),
        (9, Goto(4)),
        (10, Shift(10)),
        (11, Shift(11)),
        (12, Goto(1)),
        (13, Goto(2)),
        (15, Goto(7)),
        (16, Goto(8)),
        (17, Shift(14)),
        (23, Shift(15)),
    ],
    &[(1, Accept)],
    &[(14, Shift(16))],
    &[
        (14, Reduce(7)),
        (18, Reduce(7)),
        (22, Reduce(7)),
        (24, Reduce(7)),
    ],
    &[
        (14, Reduce(8)),
        (18, Reduce(8)),
        (22, Reduce(8)),
        (24, Reduce(8)),
    ],
    &[
        (14, Reduce(9)),
        (18, Reduce(9)),
        (22, Reduce(9)),
        (24, Reduce(9)),
    ],
    &[
        (14, Reduce(10)),
        (18, Reduce(10)),
        (22, Reduce(10)),
        (24, Reduce(10)),
    ],
    &[
        (14, Reduce(11)),
        (18, Reduce(11)),
        (22, Reduce(11)),
        (24, Reduce(11)),
    ],
    &[
        (14, Reduce(12)),
        (18, Reduce(12)),
        (22, Reduce(12)),
        (24, Reduce(12)),
    ],
    &[
        (14, Reduce(3)),
        (18, Reduce(3)),
        (22, Reduce(3)),
        (24, Reduce(3)),
    ],
    &[
        (14, Reduce(4)),
        (18, Reduce(4)),
        (22, Reduce(4)),
        (24, Reduce(4)),
    ],
    &[
        (14, Reduce(5)),
        (18, Reduce(5)),
        (22, Reduce(5)),
        (24, Reduce(5)),
    ],
    &[
        (14, Reduce(1)),
        (18, Reduce(1)),
        (21, Reduce(1)),
        (22, Reduce(1)),
        (24, Reduce(1)),
    ],
    &[
        (14, Reduce(2)),
        (18, Reduce(2)),
        (22, Reduce(2)),
        (24, Reduce(2)),
    ],
    &[
        (3, Goto(20)),
        (4, Shift(12)),
        (18, Shift(17)),
        (19, Goto(18)),
        (20, Goto(19)),
    ],
    &[
        (3, Goto(5)),
        (4, Shift(12)),
        (5, Goto(6)),
        (6, Shift(13)),
        (7, Goto(3)),
        (8, Shift(9)),
        (9, Goto(4)),
        (10, Shift(10)),
        (11, Shift(11)),
        (13, Goto(23)),
        (15, Goto(7)),
        (16, Goto(8)),
        (17, Shift(14)),
        (23, Shift(15)),
        (24, Shift(21)),
        (25, Goto(22)),
    ],
    &[(1, Reduce(6))],
    &[
        (14, Reduce(13)),
        (18, Reduce(13)),
        (22, Reduce(13)),
        (24, Reduce(13)),
    ],
    &[(18, Shift(24)), (22, Shift(25))],
    &[(18, Reduce(16)), (22, Reduce(16))],
    &[(21, Shift(26))],
    &[
        (14, Reduce(18)),
        (18, Reduce(18)),
        (22, Reduce(18)),
        (24, Reduce(18)),
    ],
    &[(22, Shift(28)), (24, Shift(27))],
    &[(22, Reduce(20)), (24, Reduce(20))],
    &[
        (14, Reduce(14)),
        (18, Reduce(14)),
        (22, Reduce(14)),
        (24, Reduce(14)),
    ],
    &[(3, Goto(20)), (4, Shift(12)), (20, Goto(29))],
    &[
        (3, Goto(5)),
        (4, Shift(12)),
        (5, Goto(6)),
        (6, Shift(13)),
        (7, Goto(3)),
        (8, Shift(9)),
        (9, Goto(4)),
        (10, Shift(10)),
        (11, Shift(11)),
        (13, Goto(30)),
        (15, Goto(7)),
        (16, Goto(8)),
        (17, Shift(14)),
        (23, Shift(15)),
    ],
    &[
        (14, Reduce(19)),
        (18, Reduce(19)),
        (22, Reduce(19)),
        (24, Reduce(19)),
    ],
    &[
        (3, Goto(5)),
        (4, Shift(12)),
        (5, Goto(6)),
        (6, Shift(13)),
        (7, Goto(3)),
        (8, Shift(9)),
        (9, Goto(4)),
        (10, Shift(10)),
        (11, Shift(11)),
        (13, Goto(31)),
        (15, Goto(7)),
        (16, Goto(8)),
        (17, Shift(14)),
        (23, Shift(15)),
    ],
    &[(18, Reduce(17)), (22, Reduce(17))],
    &[(18, Reduce(15)), (22, Reduce(15))],
    &[(22, Reduce(21)), (24, Reduce(21))],
];

/// `JsonParser::$productions_`: each production's nonterminal and length.
const PRODUCTIONS: [(i32, usize); 22] = [
    (0, 0),
    (3, 1),
    (5, 1),
    (7, 1),
    (9, 1),
    (9, 1),
    (12, 2),
    (13, 1),
    (13, 1),
    (13, 1),
    (13, 1),
    (13, 1),
    (13, 1),
    (15, 2),
    (15, 3),
    (20, 3),
    (19, 1),
    (19, 3),
    (16, 2),
    (16, 3),
    (25, 1),
    (25, 3),
];

/// `JsonParser::$terminals_` for the terminals it names when it expects them.
fn terminal(symbol: i32) -> Option<&'static str> {
    Some(match symbol {
        4 => "STRING",
        6 => "NUMBER",
        8 => "NULL",
        10 => "TRUE",
        11 => "FALSE",
        14 => "EOF",
        17 => "{",
        18 => "}",
        21 => ":",
        22 => ",",
        23 => "[",
        24 => "]",
        _ => return None,
    })
}

fn action(state: usize, symbol: i32) -> Option<Act> {
    TABLE[state]
        .iter()
        .find(|(s, _)| *s == symbol)
        .map(|(_, a)| *a)
}

/// `JsonParser::parse` until it accepts or reaches its first error.
fn parse(input: &[u8]) -> Result<(), String> {
    let mut lexer = Lexer {
        input,
        done: false,
        offset: 0,
        yylineno: 0,
        matched: vec![],
    };
    // The PHP stack: states, each nonterminal or token between two.
    let mut stack: Vec<usize> = vec![0];
    let mut symbol: Option<i32> = None;
    // The line of the last token shifted, which the error names.
    let mut yylineno = 0;
    loop {
        let state = *stack.last().expect("a state");
        let act = if state == 16 {
            Some(Reduce(6))
        } else {
            let s = match symbol {
                Some(s) => s,
                None => *symbol.insert(lexer.lex()?),
            };
            action(state, s)
        };
        match act {
            Some(Shift(next)) => {
                stack.push(usize::try_from(symbol.take().unwrap_or_default()).unwrap_or_default());
                stack.push(next);
                yylineno = lexer.yylineno;
            }
            Some(Reduce(6) | Accept) => return Ok(()),
            Some(Reduce(production)) => {
                let (lhs, len) = PRODUCTIONS[production];
                stack.truncate(stack.len() - 2 * len);
                let below = *stack.last().expect("a state");
                let lhs_usize = usize::try_from(lhs).expect("a nonterminal");
                let next = (TABLE[below].iter())
                    .find_map(|(s, a)| match a {
                        Goto(next) if *s == lhs => Some(*next),
                        _ => None,
                    })
                    .expect("jsonlint's table goes to a state after each reduction");
                stack.push(lhs_usize);
                stack.push(next);
            }
            Some(Goto(_)) | None => return Err(error(&lexer, state, yylineno)),
        }
    }
}

/// The parse error at `state`, as `JsonParser::parse` builds it.
fn error(lexer: &Lexer, state: usize, yylineno: usize) -> String {
    static BACKSLASH: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"".+?(\\[^"bfnrt/\\u](...)?)"#).expect("valid"));
    static UNTERMINATED: LazyLock<Regex> = LazyLock::new(|| {
        RegexBuilder::new()
            .multi_line(true)
            .build(r#""(?:[^"]++|\\")*$"#)
            .expect("valid")
    });
    let expected: Vec<String> = (TABLE[state].iter())
        .filter(|(s, _)| *s > 2)
        .filter_map(|(s, _)| terminal(*s))
        .map(|t| format!("'{t}'"))
        .collect();
    let mut message = None;
    let first = lexer.matched.first().copied();
    if expected.iter().any(|e| e == "'STRING'") && matches!(first, Some(b'"' | b'\'')) {
        let full = lexer.full_upcoming_input();
        let mut text = String::from("Invalid string");
        if first == Some(b'\'') {
            text.push_str(", it appears you used single quotes instead of double quotes");
        } else if let Ok(Some(caps)) = BACKSLASH.captures(&full) {
            text.push_str(", it appears you have an unescaped backslash at: ");
            text.push_str(&lossy(caps.get(1).map_or(&[][..], |m| m.as_bytes())));
        } else if UNTERMINATED.is_match(&full).unwrap_or(false) {
            text.push_str(
                ", it appears you forgot to terminate a string, or attempted to write a multiline string which is invalid",
            );
        }
        message = Some(text);
    }
    let mut out = format!(
        "Parse error on line {}:\n{}\n",
        yylineno + 1,
        lexer.show_position()
    );
    if let Some(m) = message {
        out.push_str(&m);
    } else {
        out.push_str(if expected.len() > 1 {
            "Expected one of: "
        } else {
            "Expected: "
        });
        out.push_str(&expected.join(", "));
    }
    let past = lossy(&lexer.past_input());
    if past
        .trim_matches([' ', '\t', '\n', '\r', '\0', '\x0B'])
        .ends_with(',')
    {
        out.push_str(" - It appears you have an extra trailing comma");
    }
    out
}
