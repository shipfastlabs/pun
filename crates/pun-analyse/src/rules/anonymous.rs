use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mago_allocator::LocalArena;
use mago_codex::get_anonymous_class_name;
use mago_database::file::{File, FileId};
use mago_names::ResolvedNames;
use mago_span::HasSpan;
use mago_syntax::cst::{AnonymousClass, Program};
use mago_syntax::walker::MutWalker;

use crate::relative_path::{RelativePath, absolute};

struct Collect<'ast, 'arena> {
    classes: Vec<&'ast AnonymousClass<'arena>>,
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Collect<'ast, 'arena> {
    fn walk_in_anonymous_class(&mut self, a: &'ast AnonymousClass<'arena>, (): &mut ()) {
        self.classes.push(a);
    }
}

fn start(a: &AnonymousClass) -> u32 {
    a.attribute_lists
        .first()
        .map(|l| l.span())
        .or_else(|| a.modifiers.first().map(|m| m.span()))
        .unwrap_or(a.class.span)
        .start
        .offset
}

pub(crate) fn names(
    file: &File,
    program: &Program,
    names: &ResolvedNames,
    relative: &RelativePath,
) -> HashMap<Vec<u8>, String> {
    let mut collect = Collect {
        classes: Vec::new(),
    };
    collect.walk_program(program, &mut ());
    let path = relative.get(&absolute(std::path::Path::new(&*String::from_utf8_lossy(
        &file.name,
    ))));
    let lines: Vec<u32> = collect
        .classes
        .iter()
        .map(|a| file.line_number(start(a)) + 1)
        .collect();
    let mut out = HashMap::new();
    let mut seen: HashMap<u32, usize> = HashMap::new();
    for (a, &line) in collect.classes.iter().zip(&lines) {
        let parent = a
            .extends
            .as_ref()
            .and_then(|e| e.types.first())
            .or_else(|| a.implements.as_ref().and_then(|i| i.types.first()))
            .map_or_else(
                || "class".to_string(),
                |t| String::from_utf8_lossy(names.get(t)).into_owned(),
            );
        let mut display = format!("{parent}@anonymous/{path}:{line}");
        if lines.iter().filter(|&&l| l == line).count() > 1 {
            let index = seen.entry(line).or_default();
            *index += 1;
            display = format!("{display}:{index}");
        }
        out.insert(
            get_anonymous_class_name(file, a.span()).as_bytes().to_vec(),
            display,
        );
    }
    out
}

/// Each file's [`names`], shared by every file's checker: naming an anonymous class declared in another file
/// otherwise parses that file again on every call. The names depend only on the file and the project's
/// relative path.
#[derive(Default)]
pub(crate) struct Cache(Mutex<HashMap<FileId, Option<Arc<HashMap<Vec<u8>, String>>>>>);

impl Cache {
    pub(super) fn get(
        &self,
        id: FileId,
        file: impl FnOnce() -> Option<Arc<File>>,
        relative: &RelativePath,
    ) -> Option<Arc<HashMap<Vec<u8>, String>>> {
        mago_codex::reads::record_file(id);
        if let Some(found) = self.0.lock().expect("anonymous names").get(&id) {
            return found.clone();
        }
        let found = file().map(|file| {
            let arena = LocalArena::new();
            let (program, resolved) = crate::codebase::parse(&arena, &file);
            Arc::new(names(&file, program, &resolved, relative))
        });
        (self.0.lock().expect("anonymous names")).insert(id, found.clone());
        found
    }
}

pub(crate) fn internal_name(display: &str, file: &str, relative: &RelativePath) -> String {
    let fuzzy = display
        .split_once("@anonymous/")
        .map_or(display, |(_, k)| k);
    let at = fuzzy
        .rsplit_once(':')
        .map(|(head, tail)| match head.rsplit_once(':') {
            Some((_, line)) if line.bytes().all(|b| b.is_ascii_digit()) && !line.is_empty() => {
                format!("{line}:{tail}")
            }
            _ => tail.to_string(),
        })
        .unwrap_or_default();
    let key = format!("{}:{at}", relative.simple(file));
    let digest = md5(key.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("AnonymousClass{hex}")
}

fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64)
        .map(|i| (f64::from(i + 1).sin().abs() * 4_294_967_296.0) as u32)
        .collect();
    let mut message = input.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((input.len() as u64).wrapping_mul(8)).to_le_bytes());
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for chunk in message.chunks(64) {
        let m: Vec<u32> = chunk
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(k[i])
                .wrapping_add(m[g])
                .rotate_left(S[i]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        state = [
            state[0].wrapping_add(a),
            state[1].wrapping_add(b),
            state[2].wrapping_add(c),
            state[3].wrapping_add(d),
        ];
    }
    let mut out = [0u8; 16];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}
