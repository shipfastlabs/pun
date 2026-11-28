use pun_phpstan::types::{
    Accessory, Level, ShapeBuilder, Tri, Type, accepts, is_super_type_of, oversized_array, union,
};

// Expected descriptions come from PHPStan 2.3.5 (precise verbosity):
// ConstantStringType/ConstantArrayType::generalize(GeneralizePrecision::moreSpecific())
// and OversizedArrayBuilder::build() over the same items.

const V: &str = "literal-string&lowercase-string&non-falsy-string";

fn shape(n: i64, optional: &[i64]) -> Type {
    let mut builder = ShapeBuilder::new();
    for i in 0..n {
        builder.set(
            Some(Type::ConstInt(i)),
            Type::ConstString(b"v".to_vec()),
            optional.contains(&i),
        );
    }
    builder.get()
}

#[test]
fn constant_strings_generalize_more_specifically() {
    for (s, want) in [
        ("", "literal-string"),
        (
            "0",
            "literal-string&lowercase-string&non-empty-string&numeric-string&uppercase-string",
        ),
        (
            "12",
            "literal-string&lowercase-string&non-falsy-string&numeric-string&uppercase-string",
        ),
        (
            "1.5",
            "literal-string&lowercase-string&non-falsy-string&numeric-string&uppercase-string",
        ),
        ("abc", V),
        ("ABC", "literal-string&non-falsy-string&uppercase-string"),
        ("Mixed", "literal-string&non-falsy-string"),
    ] {
        let t = Type::ConstString(s.as_bytes().to_vec()).generalize_more_specific();
        assert_eq!(t.describe(Level::Precise), want, "{s:?}");
    }
    let non_empty = Type::Intersection(vec![
        Type::String,
        Type::Accessory(Accessory::NonEmptyString),
    ]);
    assert_eq!(non_empty.generalize_more_specific(), Type::String);
}

// Below 32 required keys each one stays as hasOffsetValue; from 32 on, only
// non-empty-array is kept, and nothing when every key is optional.
#[test]
fn constant_arrays_generalize_more_specifically() {
    for (t, want) in [
        (
            shape(2, &[]),
            format!("non-empty-list<{V}>&hasOffsetValue(0, {V})&hasOffsetValue(1, {V})"),
        ),
        (shape(32, &[]), format!("non-empty-list<{V}>")),
        (shape(33, &[5]), format!("non-empty-array<int, {V}>")),
        (shape(2, &[0, 1]), format!("array<int, {V}>")),
        (shape(0, &[]), "array{}".to_string()),
    ] {
        assert_eq!(t.generalize_more_specific().describe(Level::Precise), want);
    }
}

#[test]
fn oversized_array_keys_follow_the_auto_index() {
    let v = || Type::ConstString(b"x".to_vec());
    let int = |i: i64| Some(Type::ConstInt(i));
    for (entries, want) in [
        (
            vec![(None, v()), (None, Type::ConstInt(1))],
            format!("non-empty-list<int|({V})>&oversized-array"),
        ),
        (
            vec![(int(0), v()), (int(2), v()), (None, v())],
            format!("non-empty-array<int, {V}>&oversized-array"),
        ),
        (
            vec![(Some(Type::ConstString(b"k".to_vec())), v()), (None, v())],
            format!("non-empty-array<int|({V}), {V}>&oversized-array"),
        ),
        (
            vec![(int(i64::MAX), v()), (None, v())],
            format!("non-empty-array<int, {V}>&oversized-array"),
        ),
        (
            vec![(int(i64::MAX - 1), v()), (None, v()), (None, v())],
            format!("non-empty-array<int, {V}>&oversized-array"),
        ),
    ] {
        assert_eq!(oversized_array(entries).describe(Level::Precise), want);
    }
}

// An oversized array is accepted by, and a subtype of, an array type that its
// key and value types only maybe fit, unless they cannot fit at all.
#[test]
fn oversized_array_fits_where_its_types_maybe_fit() {
    let over = oversized_array([
        (None, Type::ConstString(b"x".to_vec())),
        (None, Type::ConstInt(1)),
    ]);
    for (a, want) in [
        (Type::array(Type::Int, Type::String), Tri::Yes),
        (Type::array(Type::String, Type::String), Tri::No),
        (
            Type::array(Type::Int, union(vec![Type::Int, Type::String])),
            Tri::Yes,
        ),
    ] {
        assert_eq!(
            (accepts(&a, &over, true), is_super_type_of(&a, &over)),
            (want, want),
            "{}",
            a.describe(Level::Precise)
        );
    }
}
