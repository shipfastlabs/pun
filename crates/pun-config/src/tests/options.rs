//! Format rule options under Pun names: mapping.toml's `[options]` against the pinned PHP-CS-Fixer's option
//! definitions (`tests/fixer_options.json`, from scripts/export-fixer-options.php), and each kind of option translated
//! both ways with expectations derived by hand from those definitions.

use super::*;
use crate::options::{Kind, rule_options, to_fixer, to_pun};

fn upstream() -> Map<String, Value> {
    serde_json::from_str(include_str!("../../tests/fixer_options.json")).unwrap()
}

fn read(rule: &str, value: &str) -> Result<Value, String> {
    let v: toml::Table = toml::from_str(&format!("x = {value}")).unwrap();
    let id = format!("format/{rule}");
    to_fixer(rule, &id, v["x"].clone()).map_err(|e| e.to_string())
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "a test helper called with temporaries"
)]
fn write(fixer: &str, options: Value) -> Result<Value, String> {
    to_pun(fixer, rule_of(fixer).unwrap(), &options).map_err(|e| e.to_string())
}

#[test]
fn every_option_is_mapped() {
    let up = upstream();
    let mut seen = 0;
    for (fixer, defs) in &up {
        let Some(id) = rule_of(fixer) else {
            continue;
        };
        let opts = rule_options(id);
        let mut names: Vec<&str> = defs
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        names.sort_unstable();
        let mut mapped: Vec<&str> = (opts.iter().filter(|o| !o.pun))
            .map(|o| o.upstream.as_str())
            .collect();
        mapped.sort_unstable();
        assert_eq!(mapped, names, "{id}");
        for d in defs.as_array().unwrap() {
            let o = opts.iter().find(|o| o.upstream == d["name"]).unwrap();
            assert!(
                o.name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{id} {}: not kebab-case",
                o.name
            );
            let mut want: Vec<&str> = (d["values"].as_array().into_iter().flatten())
                .filter_map(Value::as_str)
                .collect();
            let mut got: Vec<&str> = o.values.iter().map(|(_, u)| u.as_str()).collect();
            if !want.is_empty() && !matches!(o.kind, Kind::Map) {
                want.sort_unstable();
                got.sort_unstable();
                assert_eq!(got, want, "{id} {}", o.name);
            }
            let mut types = d["types"].as_array().into_iter().flatten();
            let mut values = d["values"].as_array().into_iter().flatten();
            let takes_null = types.any(|t| t == "null")
                || values.any(Value::is_null)
                || d["types"] == json!(["array<string, ?string>"]);
            assert_eq!(
                o.null.is_some() || o.nullable,
                takes_null,
                "{id} {}",
                o.name
            );
            assert!(!o.values.iter().any(|(p, _)| Some(p) == o.null.as_ref()));
            seen += 1;
        }
    }
    let fixers: Vec<&str> = up.keys().map(String::as_str).collect();
    for (fixer, e) in fixer_entries() {
        let id = e.get("pun").and_then(toml::Value::as_str).unwrap_or("");
        assert_eq!(
            rule_options(id).iter().any(|o| !o.pun),
            !id.starts_with("analyse/") && !id.is_empty() && fixers.contains(&fixer.as_str()),
            "{fixer}"
        );
    }
    assert_eq!(seen, 141 + 81);
}

#[test]
fn upstream_values_round_trip() {
    let mut checked = 0;
    for (preset, _) in PRESETS {
        for r in rule_set(preset_rules(preset).unwrap()).unwrap() {
            let Some(id) = rule_of(&r.name).filter(|id| id.starts_with("format/")) else {
                continue;
            };
            let pun = to_pun(&r.name, id, &r.options).unwrap();
            let toml = serde_json::from_value::<toml::Value>(pun).unwrap();
            let back = to_fixer(id, id, toml).unwrap();
            let mut want = match &r.options {
                Value::Null => Value::Bool(true),
                o => o.clone(),
            };
            if let Value::Object(m) = &mut want {
                m.retain(|k, v| {
                    !(v.is_null()
                        && rule_options(id)
                            .iter()
                            .any(|o| o.upstream == *k && o.nullable))
                });
            }
            assert_eq!(back, want, "{preset} {id}");
            checked += 1;
        }
    }
    assert!(checked > 100);
    for (fixer, defs) in upstream() {
        let Some(id) = rule_of(&fixer).filter(|id| id.starts_with("format/")) else {
            continue;
        };
        for d in defs.as_array().unwrap() {
            let name = d["name"].as_str().unwrap();
            for v in d["values"].as_array().into_iter().flatten() {
                let o = rule_options(id)
                    .iter()
                    .find(|o| o.upstream == name)
                    .unwrap();
                let v = match o.kind {
                    Kind::List => json!([v]),
                    _ => v.clone(),
                };
                let options = json!({ name: v });
                let pun = to_pun(&fixer, id, &options).unwrap();
                let back = to_fixer(id, id, serde_json::from_value(pun).unwrap()).unwrap();
                assert_eq!(back, options, "{id} {name}");
            }
        }
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn each_kind_translates() {
    let cases: &[(&str, &str, &str, Value)] = &[
        (
            "blank_lines_before_namespace",
            "imports/blank-lines-before-namespace",
            "{ min-line-breaks = 1, max-line-breaks = 3 }",
            json!({"min_line_breaks": 1, "max_line_breaks": 3}),
        ),
        (
            "class_definition",
            "classes/definition",
            "{ single-line = true }",
            json!({"single_line": true}),
        ),
        (
            "no_break_comment",
            "control-flow/no-break-comment",
            "\"fall through\"",
            json!({"comment_text": "fall through"}),
        ),
        (
            "ordered_imports",
            "imports/ordered",
            "{ sort = \"alphabetical\", case-sensitive = true }",
            json!({"sort_algorithm": "alpha", "case_sensitive": true}),
        ),
        (
            "ordered_types",
            "types/ordered",
            "{ null-position = \"last\" }",
            json!({"null_adjustment": "always_last"}),
        ),
        (
            "ordered_interfaces",
            "classes/ordered-interfaces",
            "{ sort = \"length\", direction = \"descending\" }",
            json!({"order": "length", "direction": "descend"}),
        ),
        (
            "method_argument_space",
            "functions/argument-spacing",
            "{ on-multiline = \"ensure-fully-multiline\", attribute-placement = \"same-line\" }",
            json!({"on_multiline": "ensure_fully_multiline", "attribute_placement": "same_line"}),
        ),
        (
            "header_comment",
            "comments/header",
            "{ header = \"h\", comment-type = \"phpdoc\" }",
            json!({"header": "h", "comment_type": "PHPDoc"}),
        ),
        (
            "no_extra_blank_lines",
            "whitespace/no-extra-blank-lines",
            "[\"curly-brace-block\", \"use-trait\", \"extra\"]",
            json!({"tokens": ["curly_brace_block", "use_trait", "extra"]}),
        ),
        (
            "blank_line_before_statement",
            "statements/blank-line-before",
            "[\"require_once\", \"yield-from\"]",
            json!({"statements": ["require_once", "yield_from"]}),
        ),
        (
            "phpdoc_align",
            "phpdoc/align",
            "{ tags = [\"param\", \"phpstan-param\"] }",
            json!({"tags": ["param", "phpstan-param"]}),
        ),
        (
            "phpdoc_separation",
            "phpdoc/separation",
            "{ groups = [[\"param\", \"return\"], [\"throws\"]] }",
            json!({"groups": [["param", "return"], ["throws"]]}),
        ),
        (
            "class_attributes_separation",
            "classes/member-separation",
            "{ elements = { trait-import = \"only-if-meta\", method = \"one\" } }",
            json!({"elements": {"trait_import": "only_if_meta", "method": "one"}}),
        ),
        (
            "binary_operator_spaces",
            "operators/binary-spacing",
            "{ default = \"ignore\", operators = { \"=>\" = \"align-single-space-minimal\", \"|\" = \"ignore\" } }",
            json!({"default": null, "operators": {"=>": "align_single_space_minimal", "|": null}}),
        ),
        (
            "phpdoc_no_alias_tag",
            "phpdoc/no-alias-tag",
            "{ replacements = { link = \"see\" } }",
            json!({"replacements": {"link": "see"}}),
        ),
        (
            "phpdoc_align",
            "phpdoc/align",
            "{ spacing = 2 }",
            json!({"spacing": 2}),
        ),
        (
            "phpdoc_align",
            "phpdoc/align",
            "{ spacing = { param = 2 } }",
            json!({"spacing": {"param": 2}}),
        ),
        (
            "yoda_style",
            "control-flow/yoda",
            "{ equal = false, less-and-greater = \"ignore\" }",
            json!({"equal": false, "less_and_greater": null}),
        ),
        (
            "types_spaces",
            "types/union-spacing",
            "{ space = \"single\", space-multiple-catch = \"same-as-space\" }",
            json!({"space": "single", "space_multiple_catch": null}),
        ),
        (
            "array_syntax",
            "arrays/syntax",
            "\"long\"",
            json!({"syntax": "long"}),
        ),
        (
            "single_line_comment_style",
            "comments/single-line-style",
            "[\"hash\"]",
            json!({"comment_types": ["hash"]}),
        ),
    ];
    for (fixer, rule, pun, fixer_options) in cases {
        assert_eq!(read(rule, pun).as_ref(), Ok(fixer_options), "{rule} {pun}");
        let back = write(fixer, fixer_options.clone()).unwrap();
        let toml: toml::Table = toml::from_str(&format!("x = {pun}")).unwrap();
        assert_eq!(back, serde_json::to_value(&toml["x"]).unwrap(), "{rule}");
    }
    assert_eq!(
        write(
            "ordered_imports",
            json!({"imports_order": null, "sort_algorithm": "length"})
        ),
        Ok(json!({"sort": "length"}))
    );
    assert_eq!(read("imports/ordered", "{}"), Ok(json!({})));
    assert_eq!(
        write("binary_operator_spaces", json!({"operators": []})),
        Ok(json!({"operators": {}}))
    );
    assert_eq!(write("array_syntax", Value::Null), Ok(json!(true)));
    assert_eq!(read("arrays/syntax", "false"), Ok(json!(false)));
    assert_eq!(
        write(
            "single_quote",
            json!({"strings_containing_single_quote_chars": true})
        ),
        Ok(json!({"strings-containing-single-quote-chars": true}))
    );
    assert_eq!(
        write(
            "phpdoc_no_alias_tag",
            json!({"replacements": {"link": "see"}})
        ),
        Ok(json!({"replacements": {"link": "see"}}))
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn pun_toml_errors() {
    let cases = [
        (
            "imports/ordered",
            "{ sort-algorithm = \"alpha\" }",
            "`imports/ordered` has no option `sort-algorithm`: use `sort`",
        ),
        (
            "imports/ordered",
            "{ imports_order = [] }",
            "`imports/ordered` has no option `imports_order`: use `order`",
        ),
        (
            "imports/ordered",
            "{ nope = 1 }",
            "`imports/ordered` has no option `nope`; its options are `case-sensitive`, `order`, `sort`",
        ),
        (
            "imports/ordered",
            "{ sort = \"alpha\" }",
            "`imports/ordered` option `sort` = \"alpha\": expected one of \"alphabetical\", \"length\", \"none\"",
        ),
        (
            "imports/ordered",
            "{ order = [\"class\", \"trait\"] }",
            "`imports/ordered` option `order` = [\"class\", \"trait\"]: expected a list of \"class\", \"function\", \"const\"",
        ),
        (
            "imports/ordered",
            "{ case-sensitive = \"yes\" }",
            "`imports/ordered` option `case-sensitive` = \"yes\": expected true or false",
        ),
        (
            "imports/ordered",
            "\"alphabetical\"",
            "`imports/ordered` must be true, false or a table of options",
        ),
        (
            "imports/blank-lines-before-namespace",
            "{ min-line-breaks = 1.5 }",
            "`imports/blank-lines-before-namespace` option `min-line-breaks` = 1.5: expected an integer",
        ),
        (
            "comments/header",
            "{ header = 1 }",
            "`comments/header` option `header` = 1: expected a string",
        ),
        (
            "phpdoc/align",
            "{ tags = [1] }",
            "`phpdoc/align` option `tags` = [1]: expected a list of strings",
        ),
        (
            "phpdoc/align",
            "{ spacing = { param = \"x\" } }",
            "`phpdoc/align` option `spacing` = { param = \"x\" }: expected an integer or a table of integers",
        ),
        (
            "phpdoc/separation",
            "{ groups = [\"param\"] }",
            "`phpdoc/separation` option `groups` = [\"param\"]: expected a list of lists of strings",
        ),
        (
            "phpdoc/no-alias-tag",
            "{ replacements = { link = 1 } }",
            "`phpdoc/no-alias-tag` option `replacements` = { link = 1 }: expected a table with string keys and string values",
        ),
        (
            "classes/member-separation",
            "{ elements = { trait_import = \"one\" } }",
            "`classes/member-separation` option `elements` = { trait_import = \"one\" }: expected a table with the keys \"const\", \"method\", \"property\", \"trait-import\", \"case\" and the values \"none\", \"one\", \"only-if-meta\"",
        ),
        (
            "operators/binary-spacing",
            "{ default = \"nope\" }",
            "`operators/binary-spacing` option `default` = \"nope\": expected one of \"align\", \"align-by-scope\", \"align-single-space\", \"align-single-space-minimal\", \"align-single-space-by-scope\", \"align-single-space-minimal-by-scope\", \"single-space\", \"no-space\", \"at-least-single-space\", or \"ignore\"",
        ),
        (
            "operators/binary-spacing",
            "{ operators = { \"=>>\" = \"single-space\" } }",
            "`operators/binary-spacing` option `operators` = { \"=>>\" = \"single-space\" }: expected a table with the keys \"=\", \"*\", \"/\", \"%\", \"<\", \">\", \"|\", \"^\", \"+\", \"-\", \"&\", \"&=\", \"&&\", \"||\", \".=\", \"/=\", \"=>\", \"==\", \">=\", \"===\", \"!=\", \"<>\", \"!==\", \"<=\", \"and\", \"or\", \"xor\", \"-=\", \"%=\", \"*=\", \"|=\", \"+=\", \"<<\", \"<<=\", \">>\", \">>=\", \"^=\", \"**\", \"**=\", \"<=>\", \"??\", \"??=\" and the values \"align\", \"align-by-scope\", \"align-single-space\", \"align-single-space-minimal\", \"align-single-space-by-scope\", \"align-single-space-minimal-by-scope\", \"single-space\", \"no-space\", \"at-least-single-space\", or \"ignore\"",
        ),
        (
            "arrays/indentation",
            "{ x = 1 }",
            "`arrays/indentation` has no options: it is true or false, without a table",
        ),
        (
            "arrays/indentation",
            "{}",
            "`arrays/indentation` has no options: it is true or false",
        ),
        (
            "arrays/indentation",
            "\"x\"",
            "`arrays/indentation` must be true or false",
        ),
        (
            "strings/single-quote",
            "\"x\"",
            "`strings/single-quote` must be true, false or a table of options",
        ),
    ];
    for (rule, value, msg) in cases {
        assert_eq!(read(rule, value), Err(msg.to_string()), "{rule} {value}");
    }
    let d = dir(&[(
        "pun.toml",
        "[format.rules]\n\"arrays/syntax\" = \"medium\"\n",
    )]);
    assert_eq!(
        err(&d, Options::default()),
        format!(
            "{}: `arrays/syntax` option `syntax` = \"medium\": expected one of \"long\", \"short\"",
            d.join("pun.toml").display()
        )
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn migrate_errors() {
    let cases = [
        (
            "blank_lines_before_namespace",
            json!({"pun_nope": 1}),
            "[blank_lines_before_namespace] Invalid configuration: The option \"pun_nope\" does not exist. Defined options are: \"max_line_breaks\", \"min_line_breaks\".",
        ),
        (
            "align_multiline_comment",
            json!({"comment_type": "nope"}),
            "[align_multiline_comment] Invalid configuration: The option \"comment_type\" with value \"nope\" is invalid. Accepted values are: \"phpdocs_only\", \"phpdocs_like\", \"all_multiline\".",
        ),
        (
            "increment_style",
            json!({"style": true}),
            "[increment_style] Invalid configuration: The option \"style\" with value true is invalid. Accepted values are: \"pre\", \"post\".",
        ),
        (
            "array_indentation",
            json!({"x": 1}),
            "[array_indentation] Is not configurable.",
        ),
        (
            "header_comment",
            json!({"header": null}),
            "[header_comment] Invalid configuration: The option \"header\" with value null is expected to be of type \"string\", but is of type \"null\".",
        ),
        (
            "array_syntax",
            json!({"syntax": null}),
            "[array_syntax] Invalid configuration: The option \"syntax\" with value null is invalid. Accepted values are: \"long\", \"short\".",
        ),
        (
            "phpdoc_line_span",
            json!({"const": "nope"}),
            "[phpdoc_line_span] Invalid configuration: The option \"const\" with value \"nope\" is invalid. Accepted values are: \"single\", \"multi\", null.",
        ),
        (
            "class_definition",
            json!({"single_line": null}),
            "[class_definition] Invalid configuration: The option \"single_line\" with value null is expected to be of type \"bool\", but is of type \"null\".",
        ),
        (
            "blank_lines_before_namespace",
            json!({"min_line_breaks": null}),
            "[blank_lines_before_namespace] Invalid configuration: The option \"min_line_breaks\" with value null is expected to be of type \"int\", but is of type \"null\".",
        ),
        (
            "phpdoc_align",
            json!({"tags": null}),
            "[phpdoc_align] Invalid configuration: The option \"tags\" with value null is expected to be of type \"string[]\", but is of type \"null\".",
        ),
        (
            "phpdoc_align",
            json!({"spacing": null}),
            "[phpdoc_align] Invalid configuration: The option \"spacing\" with value null is expected to be of type \"int\" or \"int[]\", but is of type \"null\".",
        ),
        (
            "phpdoc_separation",
            json!({"groups": null}),
            "[phpdoc_separation] Invalid configuration: The option \"groups\" with value null is expected to be of type \"string[][]\", but is of type \"null\".",
        ),
        (
            "phpdoc_no_alias_tag",
            json!({"replacements": null}),
            "[phpdoc_no_alias_tag] Invalid configuration: The option \"replacements\" with value null is expected to be of type \"string[]\", but is of type \"null\".",
        ),
        (
            "binary_operator_spaces",
            json!({"operators": null}),
            "[binary_operator_spaces] Invalid configuration: The option \"operators\" with value null is expected to be of type \"array\", but is of type \"null\".",
        ),
        (
            "binary_operator_spaces",
            json!({"operators": {"nope": "single_space"}}),
            "[binary_operator_spaces] Invalid configuration: Unexpected \"operators\" key, expected any of \"=\", \"*\", \"/\", \"%\", \"<\", \">\", \"|\", \"^\", \"+\", \"-\", \"&\", \"&=\", \"&&\", \"||\", \".=\", \"/=\", \"=>\", \"==\", \">=\", \"===\", \"!=\", \"<>\", \"!==\", \"<=\", \"and\", \"or\", \"xor\", \"-=\", \"%=\", \"*=\", \"|=\", \"+=\", \"<<\", \"<<=\", \">>\", \">>=\", \"^=\", \"**\", \"**=\", \"<=>\", \"??\" and \"??=\", got \"string#nope\".",
        ),
        (
            "binary_operator_spaces",
            json!({"operators": {"=>": "nope"}}),
            "[binary_operator_spaces] Invalid configuration: Unexpected value for operator \"=>\", expected any of \"'align'\", \"'align_by_scope'\", \"'align_single_space'\", \"'align_single_space_minimal'\", \"'align_single_space_by_scope'\", \"'align_single_space_minimal_by_scope'\", \"'single_space'\", \"'no_space'\", \"'at_least_single_space'\" and \"null\", got \"string#nope\".",
        ),
        (
            "phpdoc_tag_type",
            json!({"tags": {"param": "nope"}}),
            "[phpdoc_tag_type] Invalid configuration: Unknown tag type \"nope\".",
        ),
        (
            "group_import",
            json!({"group_types": ["classy", "nope"]}),
            "[group_import] Invalid configuration: Invalid group type: nope, allowed types: \"classy\", \"functions\" and \"constants\".",
        ),
        (
            "ordered_imports",
            json!({"imports_order": ["nope"]}),
            "[ordered_imports] Invalid configuration: Missing sort types \"class\", \"const\" and \"function\".",
        ),
        (
            "ordered_imports",
            json!({"imports_order": ["class", "const"]}),
            "[ordered_imports] Invalid configuration: Missing sort type \"function\".",
        ),
        (
            "ordered_imports",
            json!({"imports_order": ["class", "const", "function", "nope"]}),
            "[ordered_imports] Invalid configuration: Unknown sort type \"nope\".",
        ),
        (
            "blank_line_before_statement",
            json!({"statements": ["nope"]}),
            "[blank_line_before_statement] Invalid configuration: The option \"statements\" with value array is invalid.",
        ),
        (
            "class_attributes_separation",
            json!({"elements": {"nope": "one"}}),
            "[class_attributes_separation] Invalid configuration: Unexpected element type, expected any of \"const\", \"method\", \"property\", \"trait_import\" and \"case\", got \"string#nope\".",
        ),
        (
            "class_attributes_separation",
            json!({"elements": {"method": "nope"}}),
            "[class_attributes_separation] Invalid configuration: Unexpected spacing for element type \"nope\", expected any of \"none\", \"one\" and \"only_if_meta\", got \"string#nope\".",
        ),
    ];
    for (fixer, options, msg) in cases {
        assert_eq!(write(fixer, options), Err(msg.to_string()), "{fixer}");
    }
    let e = crate::migrate::from_pint(
        &dir(&[(
            "pint.json",
            r#"{"rules":{"array_syntax":{"syntax":"medium"}}}"#,
        )])
        .join("pint.json"),
        Path::new("/project"),
        None,
        &|_| true,
    );
    assert_eq!(
        e.err().unwrap().to_string(),
        "[array_syntax] Invalid configuration: The option \"syntax\" with value \"medium\" is invalid. Accepted values are: \"long\", \"short\"."
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one test walks every case of the behaviour it checks"
)]
fn migrate_entry_types() {
    let elements = |of: &str, types: &str, got: &str| {
        format!(
            "Invalid configuration: The option \"{of}\" with value array is expected to be of type \"{types}\", but one of the elements is of type \"{got}\"."
        )
    };
    let cases = [
        (
            "class_attributes_separation",
            json!({"elements": {"method": null}}),
            elements("elements", "string[]", "null"),
        ),
        (
            "class_attributes_separation",
            json!({"elements": {"nope": null}}),
            elements("elements", "string[]", "null"),
        ),
        (
            "class_attributes_separation",
            json!({"elements": {"method": 1.5}}),
            elements("elements", "string[]", "float"),
        ),
        (
            "class_attributes_separation",
            json!({"elements": {"method": true}}),
            elements("elements", "string[]", "bool"),
        ),
        (
            "phpdoc_tag_type",
            json!({"tags": {"param": null}}),
            elements("tags", "string[]", "null"),
        ),
        (
            "phpdoc_tag_type",
            json!({"tags": {"param": []}}),
            elements("tags", "string[]", "array"),
        ),
        (
            "phpdoc_no_alias_tag",
            json!({"replacements": {"a": null}}),
            elements("replacements", "string[]", "null"),
        ),
        (
            "group_import",
            json!({"group_types": [null]}),
            elements("group_types", "string[]", "null"),
        ),
        (
            "group_import",
            json!({"group_types": [1]}),
            elements("group_types", "string[]", "int"),
        ),
        (
            "group_import",
            json!({"group_types": [null, 1, null]}),
            elements("group_types", "string[]", "null|int"),
        ),
        (
            "phpdoc_align",
            json!({"tags": [null]}),
            elements("tags", "string[]", "null"),
        ),
        (
            "phpdoc_align",
            json!({"spacing": {"param": null}}),
            elements("spacing", "int\" or \"int[]", "array|null"),
        ),
        (
            "blank_line_before_statement",
            json!({"statements": [null]}),
            elements("statements", "string[]", "null"),
        ),
        (
            "phpdoc_separation",
            json!({"groups": [[null]]}),
            elements("groups", "string[][]", "null"),
        ),
        (
            "phpdoc_separation",
            json!({"groups": ["a"]}),
            elements("groups", "string[][]", "string"),
        ),
        (
            "phpdoc_separation",
            json!({"groups": [["a", 1]]}),
            elements("groups", "string[][]", "int"),
        ),
    ];
    for (fixer, options, msg) in cases {
        assert_eq!(
            write(fixer, options),
            Err(format!("[{fixer}] {msg}")),
            "{fixer}"
        );
    }
    assert_eq!(
        write(
            "class_attributes_separation",
            json!({"elements": {"method": "one", "property": "nope"}})
        ),
        Err("[class_attributes_separation] Invalid configuration: Unexpected spacing for element type \"nope\", expected any of \"none\", \"one\" and \"only_if_meta\", got \"string#nope\".".to_string())
    );
    assert_eq!(
        write("binary_operator_spaces", json!({"operators": {"nope": null}})),
        Err("[binary_operator_spaces] Invalid configuration: Unexpected \"operators\" key, expected any of \"=\", \"*\", \"/\", \"%\", \"<\", \">\", \"|\", \"^\", \"+\", \"-\", \"&\", \"&=\", \"&&\", \"||\", \".=\", \"/=\", \"=>\", \"==\", \">=\", \"===\", \"!=\", \"<>\", \"!==\", \"<=\", \"and\", \"or\", \"xor\", \"-=\", \"%=\", \"*=\", \"|=\", \"+=\", \"<<\", \"<<=\", \">>\", \">>=\", \"^=\", \"**\", \"**=\", \"<=>\", \"??\" and \"??=\", got \"string#nope\".".to_string())
    );
    let e = crate::migrate::from_pint(
        &dir(&[(
            "pint.json",
            r#"{"rules":{"group_import":{"group_types":[null]}}}"#,
        )])
        .join("pint.json"),
        Path::new("/project"),
        None,
        &|_| true,
    );
    assert_eq!(
        e.err().unwrap().to_string(),
        format!(
            "[group_import] {}",
            elements("group_types", "string[]", "null")
        )
    );
}

#[test]
fn lint_rule_options() {
    let d = dir(&[(
        "pun.toml",
        "[lint.rules]\n\"phpunit/require-method-casing\" = { case = \"snake-case\" }\n\"phpdoc/no-superfluous-tags\" = { level = \"warn\", allow-mixed = true }\n",
    )]);
    let cfg = load(&d, &Options::default()).unwrap();
    let rule = |id: &str| cfg.lint.rules.iter().find(|r| r.id == id).unwrap();
    assert_eq!(
        rule("lint/phpunit/require-method-casing").options,
        json!({ "case": "snake_case" })
    );
    assert_eq!(
        rule("lint/phpdoc/no-superfluous-tags").options,
        json!({ "allow_mixed": true })
    );
    let shown = cfg.show();
    assert_eq!(
        shown["lint"]["rules"]["phpunit/require-method-casing"],
        json!({ "case": "snake-case" })
    );
    assert_eq!(
        shown["lint"]["rules"]["phpdoc/no-superfluous-tags"],
        json!({ "level": "warn", "allow-mixed": true })
    );
    // Pun's own options of a rule a fixer runs reach it, and show under their names beside the fixer's.
    let d = dir(&[(
        "pun.toml",
        "[lint.rules]\n\"control-flow/no-return-assignment\" = { skip-commented = true, skip-named-var-tags = true }\n\"style/prefer-class-constant\" = { namespaced-only = true }\n",
    )]);
    let cfg = load(&d, &Options::default()).unwrap();
    let rule = |id: &str| cfg.lint.rules.iter().find(|r| r.id == id).unwrap();
    assert_eq!(
        rule("lint/control-flow/no-return-assignment").options,
        json!({ "skip_commented": true, "skip_named_var_tags": true })
    );
    let shown = &cfg.show()["lint"]["rules"];
    assert_eq!(
        shown["control-flow/no-return-assignment"],
        json!({ "skip-commented": true, "skip-named-var-tags": true })
    );
    assert_eq!(
        shown["style/prefer-class-constant"],
        json!({ "namespaced-only": true })
    );
    let d = dir(&[(
        "pun.toml",
        "[lint.rules]\n\"phpdoc/no-superfluous-tags\" = { allow_mixed = true }\n",
    )]);
    assert!(
        err(&d, Options::default()).ends_with(
            "`phpdoc/no-superfluous-tags` has no option `allow_mixed`: use `allow-mixed`"
        ),
        "{}",
        err(&d, Options::default())
    );
    let d = dir(&[(
        "pun.toml",
        "[lint.rules]\n\"phpunit/require-method-casing\" = { case = \"snake_case\" }\n",
    )]);
    assert!(
        err(&d, Options::default()).ends_with(
            "option `case` = \"snake_case\": expected one of \"camel-case\", \"snake-case\""
        ),
        "{}",
        err(&d, Options::default())
    );
}
