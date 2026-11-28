use crate::analyzers::docblock::type_expression::REGEX_IDENTIFIER;
use crate::analyzers::fixer_configuration_resolver::resolve;
use crate::analyzers::preg::Preg;
use anyhow::{Result, bail};
use pun_format::Fixer;
use pun_tokens::Kind::*;
use pun_tokens::php_compat::strtolower;
use pun_tokens::tokens_analyzer::TokensAnalyzer;
use pun_tokens::{Token, Tokens};
use serde_json::Value;
use std::path::Path;

struct PsrAutoloadingFixer {
    dir: Option<String>,
}

pub(crate) fn fixer() -> Box<dyn Fixer> {
    Box::new(PsrAutoloadingFixer { dir: None })
}

fn real_path(path: &Path) -> Option<String> {
    std::fs::canonicalize(path)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

fn dirname(path: &str) -> &str {
    &path[..path.rfind('/').unwrap().max(1)]
}

fn substr_from(s: &str, start: usize) -> &str {
    s.get(start..).unwrap_or("")
}

fn substr_tail(s: &str, len: usize) -> &str {
    &s[s.len().saturating_sub(len)..]
}

fn basename_php(path: &Path) -> String {
    let name = path
        .file_name()
        .map_or_default(|n| n.to_string_lossy().into_owned());
    name.strip_suffix(".php").unwrap().to_string()
}

impl Fixer for PsrAutoloadingFixer {
    fn name(&self) -> &'static str {
        "psr_autoloading"
    }
    fn priority(&self) -> i32 {
        -10
    }
    fn is_risky(&self) -> bool {
        true
    }
    fn is_candidate(&self, tokens: &Tokens) -> bool {
        tokens.is_any_token_kinds_found(&[T_CLASS, T_TRAIT, T_INTERFACE, T_ENUM])
    }
    fn supports(&self, file: &Path) -> bool {
        if file.extension().is_none_or(|e| e != "php") {
            return false;
        }
        let name = basename_php(file);
        if !Preg::r#match(format!("/^{REGEX_IDENTIFIER}$/"), name.as_bytes()) {
            return false;
        }
        let code = format!("<?php class {name} {{}}");
        let raw = &pun_tokens::lexer::tokenize(code.as_bytes())[3];
        let token = Token::new(raw.kind.unwrap(), raw.text);
        if token.is_keyword() || token.is_magic_constant() {
            return false;
        }
        let Some(real) = real_path(file) else {
            return false;
        };
        !Preg::r#match(r"{[/\\](stub|fixture)s?[/\\]}i", real.as_bytes())
    }
    fn configure(&mut self, options: &Value) -> Result<()> {
        let o = resolve(self.name(), options, &["dir"])?;
        self.dir = match o.get("dir") {
            None | Some(Value::Null) => None,
            Some(Value::String(dir)) => match real_path(Path::new(dir)) {
                Some(real) => Some(real),
                None => bail!("Failed to resolve configured directory \"{dir}\"."),
            },
            Some(v) => return Err(o.type_error("dir", v, "\"null\" or \"string\"")),
        };
        Ok(())
    }
    fn fix(&self, file: &Path, tokens: &mut Tokens) {
        let real = real_path(file).unwrap();
        if let Some(dir) = &self.dir
            && !real.starts_with(dir.as_str())
        {
            return;
        }
        let mut namespace: Option<(String, usize, usize)> = None;
        let mut classy: Option<(Vec<u8>, usize)> = None;
        for index in 0..tokens.count() {
            if tokens[index].is_kind(T_NAMESPACE) {
                if namespace.is_some() {
                    return;
                }
                let start = tokens.next_meaningful(index);
                // A braced namespace (`namespace A { ... }`) with no `;` after it: PHP-CS-Fixer's
                // getNextTokenOfKind() returns null and generatePartialCode($start, null - 1) is '', so the
                // namespace is empty. The end index is then never read: an empty namespace changes no casing.
                let Some(end) = tokens.get_next_token_of_kind(start, &[";".into()], true) else {
                    namespace = Some((String::new(), start, start));
                    continue;
                };
                let code = tokens.generate_partial_code(start, end - 1);
                let name = String::from_utf8_lossy(&code)
                    .trim_matches(TRIM)
                    .to_string();
                namespace = Some((name, start, end));
            } else if tokens[index].is_classy() {
                if TokensAnalyzer::new(tokens).is_anonymous_class(index) {
                    continue;
                }
                if classy.is_some() {
                    return;
                }
                let classy_index = tokens.next_meaningful(index);
                classy = Some((tokens[classy_index].get_content().to_vec(), classy_index));
            }
        }
        let Some((classy_name, classy_index)) = classy else {
            return;
        };
        let ns = namespace.as_ref().map(|(n, _, _)| n.as_str());
        let expected =
            self.calculate_classy_name(&real, file, ns, &String::from_utf8_lossy(&classy_name));
        if classy_name != expected.as_bytes() && !names_class(tokens, classy_index) {
            tokens.set(classy_index, Token::new(T_STRING, expected));
        }
        let (Some(dir), Some((namespace, start, end))) = (&self.dir, namespace) else {
            return;
        };
        if !Path::new(dir).is_dir() {
            return;
        }
        let file_dir = dirname(&real);
        if dir.len() >= file_dir.len() {
            return;
        }
        let new_namespace = substr_from(&file_dir.replace('/', "\\"), dir.len() + 1).to_string();
        let original_namespace = substr_tail(&namespace, new_namespace.len());
        if original_namespace != new_namespace
            && original_namespace.eq_ignore_ascii_case(&new_namespace)
        {
            tokens.clear_range(start, end);
            let namespace = format!(
                "{}{}",
                &namespace[..namespace.len() - new_namespace.len()],
                new_namespace
            );
            let mut new_tokens =
                Tokens::from_code(format!("<?php namespace {namespace};").as_bytes());
            new_tokens.clear_range(0, 2);
            new_tokens.clear_empty_tokens();
            tokens.insert_at(start, new_tokens.iter().cloned().collect());
        }
    }
}

/// Not in PHP-CS-Fixer: whether the file names the class it declares at `index` anywhere else
/// (`Foo::class`, `new Foo`, a type). Renaming only the declaration would leave those names dangling.
fn names_class(tokens: &Tokens, index: usize) -> bool {
    let name = tokens[index].get_content();
    (0..tokens.count()).any(|i| {
        i != index
            && tokens[i].is_kind(T_STRING)
            && tokens[i].get_content().eq_ignore_ascii_case(name)
            && tokens.get_prev_meaningful_token(i).is_none_or(|p| {
                !tokens[p].is_given_kind(&[
                    T_OBJECT_OPERATOR,
                    T_NULLSAFE_OBJECT_OPERATOR,
                    T_DOUBLE_COLON,
                    T_FUNCTION,
                    T_CONST,
                ])
            })
    })
}

const TRIM: &[char] = &[' ', '\t', '\n', '\r', '\0', '\x0B'];

impl PsrAutoloadingFixer {
    fn calculate_classy_name(
        &self,
        real: &str,
        file: &Path,
        namespace: Option<&str>,
        current_name: &str,
    ) -> String {
        let mut name = basename_php(file);
        let max_namespace = self.calculate_max_namespace(real, namespace);
        if self.dir.is_some() {
            return if max_namespace.is_empty() {
                name
            } else {
                format!("{}_{name}", max_namespace.replace('\\', "_"))
            };
        }
        for namespace_part in max_namespace.split('\\').rev() {
            let name_candidate = format!("{namespace_part}_{name}");
            let tail = substr_tail(current_name, name_candidate.len());
            if strtolower(name_candidate.as_bytes()) != strtolower(tail.as_bytes()) {
                break;
            }
            name = name_candidate;
        }
        name
    }

    fn calculate_max_namespace(&self, real: &str, namespace: Option<&str>) -> String {
        let root = match &self.dir {
            None => "/".to_string(),
            Some(dir) => dir.clone(),
        };
        let location = substr_from(dirname(real), root.len())
            .replace('/', "\\")
            .trim_matches('\\')
            .to_string();
        let Some(namespace) = namespace else {
            return location;
        };
        let mut location_parts_reversed: Vec<&str> = location.split('\\').rev().collect();
        let mut removed = 0;
        for (key, namespace_part) in namespace.split('\\').rev().enumerate() {
            let Some(part) = location_parts_reversed.get(key) else {
                break;
            };
            if strtolower(namespace_part.as_bytes()) != strtolower(part.as_bytes()) {
                break;
            }
            removed = key + 1;
        }
        location_parts_reversed.drain(..removed);
        location_parts_reversed.reverse();
        location_parts_reversed.join("\\")
    }
}

#[cfg(test)]
mod tests {
    use pun_tokens::Tokens;

    fn fix(dir: &std::path::Path, code: &str) -> String {
        let file = dir.join("Pest.php");
        std::fs::write(&file, code).unwrap();
        let mut tokens = Tokens::from_code(code.as_bytes());
        super::fixer().apply(&file, &mut tokens);
        String::from_utf8(tokens.generate_code()).unwrap()
    }

    #[test]
    fn keeps_a_class_the_file_names() {
        let dir = std::env::temp_dir().join(format!("pun-psr-autoloading-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // PHP-CS-Fixer 3.95.25 renames this declaration to `Pest` and leaves `PreventStraySaloonSends::class`
        // naming no class; Pun keeps the name.
        let same = "<?php\n\nuse Saloon\\Contracts\\Sender;\n\nfinal readonly class PreventStraySaloonSends implements Sender\n{\n}\n\nconfig()->set('saloon.default_sender', PreventStraySaloonSends::class);\n";
        assert_eq!(fix(&dir, same), same);
        // PHP-CS-Fixer's output where the file names the class nowhere else, or only as a member name.
        assert_eq!(
            fix(&dir, "<?php\n\nfinal class PreventStraySaloonSends\n{\n    public function x() { $this->PreventStraySaloonSends(); }\n}\n"),
            "<?php\n\nfinal class Pest\n{\n    public function x() { $this->PreventStraySaloonSends(); }\n}\n"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn braced_namespace_without_a_semicolon() {
        let dir =
            std::env::temp_dir().join(format!("pun-psr-autoloading-ns-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Pint 1.32.1's output: no `;` follows the namespace name, so the namespace is empty and the class is renamed.
        assert_eq!(
            fix(&dir, "<?php\n\nnamespace A {\n    final class B {}\n}\n"),
            "<?php\n\nnamespace A {\n    final class Pest {}\n}\n"
        );
        // Pint 1.32.1 leaves a file with two namespaces alone.
        let two = "<?php\n\nnamespace Livewire\\Attributes {\n    #[\\Attribute]\n    final class Computed {}\n}\n\nnamespace Livewire {\n    abstract class Component {}\n}\n";
        assert_eq!(fix(&dir, two), two);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
