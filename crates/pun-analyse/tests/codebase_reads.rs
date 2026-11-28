//! mago-codex's recordings (`mago_codex::reads`, a Pun patch): what a unit of analysis read, for the analyse result
//! cache. Nested recordings reach the outer one unless kept apart, and a panic leaves no recording behind.

use mago_codex::reads::{active, record_class_like, record_function, recording, recording_alone};
use mago_word::word;

#[test]
fn nested_recordings_reach_the_outer_one() {
    record_class_like(word("ignored"));
    let ((), outer) = recording(|| {
        record_class_like(word("a"));
        let ((), inner) = recording(|| record_function(word("f")));
        assert!(inner.functions.contains(&word("f")));
        assert!(!inner.class_likes.contains(&word("a")));
        let ((), alone) = recording_alone(|| record_function(word("g")));
        assert!(alone.functions.contains(&word("g")));
    });
    assert!(outer.class_likes.contains(&word("a")));
    assert!(outer.functions.contains(&word("f")));
    assert!(!outer.functions.contains(&word("g")));
    assert!(!outer.class_likes.contains(&word("ignored")));
    assert!(!active());
}

#[test]
fn a_panic_leaves_no_recording_behind() {
    let r = std::panic::catch_unwind(|| recording(|| panic!("boom")));
    assert!(r.is_err());
    assert!(!active());
    let ((), after) = recording(|| record_function(word("h")));
    assert_eq!(after.functions.len(), 1);
}
