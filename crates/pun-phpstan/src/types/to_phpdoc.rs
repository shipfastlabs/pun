use super::{
    Accessory, Level, Signature, Type, display_bytes, float_to_string, intersect, print_templates,
    sort_types, variance_name,
};

pub fn to_phpdoc(t: &Type) -> String {
    display_bytes(&to_phpdoc_bytes(t))
}

thread_local! {
    static NAME_NODES: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn to_phpdoc_fully_qualified(t: &Type) -> String {
    NAME_NODES.set(true);
    let printed = display_bytes(&to_phpdoc_bytes(t));
    NAME_NODES.set(false);
    printed
}

fn phpdoc_class_name(class: &str) -> Vec<u8> {
    if NAME_NODES.get() {
        description!("\\", class)
    } else {
        class.as_bytes().to_vec()
    }
}

pub(super) fn to_phpdoc_bytes(t: &Type) -> Vec<u8> {
    match t {
        Type::Reasoned(t, _) => to_phpdoc_bytes(t),
        Type::LateArrayShape(s) => s.print(true),
        Type::Accessory(Accessory::HasMethod(_) | Accessory::HasOffsetValue(..)) => Vec::new(),
        Type::Intersection(types)
            if types
                .iter()
                .any(|t| matches!(t, Type::Accessory(Accessory::HasOffsetValue(..)))) =>
        {
            to_phpdoc_bytes(&intersect(
                types
                    .iter()
                    .filter(|t| !matches!(t, Type::Accessory(Accessory::HasOffsetValue(..))))
                    .cloned()
                    .collect(),
            ))
        }
        Type::Mixed { .. } | Type::StrictMixed | Type::SubtractedMixed { .. } | Type::Error => {
            "mixed".into()
        }
        Type::Never(_) => "never".into(),
        Type::ConstInt(i) => i.to_string().into_bytes(),
        Type::ConstFloat(f) => float_to_string(*f).into_bytes(),
        // ConstantStringType::toPhpDocNode(): a PHPDoc string can't hold a newline, so it prints the generalized type
        Type::ConstString(s) if s.contains(&b'\n') => {
            let mut parts = vec![Type::String, Type::Accessory(Accessory::LiteralString)];
            if t.is_numeric_string().yes() {
                parts.push(Type::Accessory(Accessory::NumericString));
            }
            parts.push(Type::Accessory(if s.as_slice() == b"0" {
                Accessory::NonEmptyString
            } else {
                Accessory::NonFalsyString
            }));
            if s.to_ascii_lowercase() == *s {
                parts.push(Type::Accessory(Accessory::LowercaseString));
            }
            if s.to_ascii_uppercase() == *s {
                parts.push(Type::Accessory(Accessory::UppercaseString));
            }
            to_phpdoc_bytes(&intersect(parts))
        }
        Type::ConstString(s) => {
            let mut out = vec![b'\''];
            for &byte in s {
                if matches!(byte, b'\\' | b'\'') {
                    out.push(b'\\');
                }
                out.push(byte);
            }
            out.push(b'\'');
            out
        }
        Type::IntRange(min, max) => format!(
            "int<{}, {}>",
            min.map_or("min".to_string(), |v| v.to_string()),
            max.map_or("max".to_string(), |v| v.to_string())
        )
        .into_bytes(),
        Type::ClassString(Some(g)) => description!("class-string<", to_phpdoc_bytes(g), ">"),
        Type::Object(class, args, _) if !args.is_empty() => {
            description!(
                phpdoc_class_name(class),
                "<",
                args.iter()
                    .map(to_phpdoc_bytes)
                    .collect::<Vec<_>>()
                    .join(b", ".as_slice()),
                ">"
            )
        }
        Type::Object(class, _, _) => phpdoc_class_name(class),
        Type::EnumCase(class, case) if NAME_NODES.get() => description!("\\", class, "::", case),
        Type::Static(..) => "static".into(),
        Type::This(_) => "$this".into(),
        Type::Array(key, item) => {
            if key.is_plain_implicit_mixed() {
                if item.is_plain_implicit_mixed() {
                    return "array".into();
                }
                return description!("array<", to_phpdoc_bytes(item), ">");
            }
            description!(
                "array<",
                to_phpdoc_bytes(key),
                ", ",
                to_phpdoc_bytes(item),
                ">"
            )
        }
        Type::ConstArray(array) => array.to_phpdoc(),
        Type::Iterable(key, item) => {
            if matches!(**key, Type::Mixed { .. }) {
                if matches!(**item, Type::Mixed { .. }) {
                    return "iterable".into();
                }
                return description!("iterable<", to_phpdoc_bytes(item), ">");
            }
            description!(
                "iterable<",
                to_phpdoc_bytes(key),
                ", ",
                to_phpdoc_bytes(item),
                ">"
            )
        }
        Type::Union(types) | Type::Benevolent(types) => join_phpdoc(&sort_types(types), "|"),
        Type::Intersection(types)
            if let Some(Type::Array(key, item)) =
                types.iter().find(|t| matches!(t, Type::Array(..))) =>
        {
            let has = |a: Accessory| types.contains(&Type::Accessory(a));
            match (
                types.len(),
                has(Accessory::List),
                has(Accessory::NonEmptyArray),
            ) {
                (2, true, false) if !item.is_plain_implicit_mixed() => {
                    description!("list<", to_phpdoc_bytes(item), ">")
                }
                (3, true, true) if !item.is_plain_implicit_mixed() => {
                    description!("non-empty-list<", to_phpdoc_bytes(item), ">")
                }
                (2, false, true)
                    if !key.is_plain_implicit_mixed() && !item.is_plain_implicit_mixed() =>
                {
                    description!(
                        "non-empty-array<",
                        to_phpdoc_bytes(key),
                        ", ",
                        to_phpdoc_bytes(item),
                        ">"
                    )
                }
                _ => t.describe_bytes(Level::Precise),
            }
        }
        Type::Intersection(types)
            if types
                .iter()
                .any(|t| matches!(t, Type::Accessory(Accessory::HasMethod(_)))) =>
        {
            let printable: Vec<_> = types
                .iter()
                .filter(|t| !matches!(t, Type::Accessory(Accessory::HasMethod(_))))
                .cloned()
                .collect();
            to_phpdoc_bytes(&intersect(printable))
        }
        Type::Intersection(types)
            if types.iter().any(|t| {
                matches!(t, Type::StrictMixed)
                    || matches!(t, Type::Template(t) if matches!(t.bound, Type::StrictMixed))
            }) =>
        {
            join_phpdoc(&sort_types(types), "&")
        }
        Type::Intersection(_) => t.describe_bytes(Level::Precise),
        Type::Template(tpl) => tpl.name.as_bytes().to_vec(),
        Type::Callable(None) => "callable".into(),
        Type::Callable(Some(sig)) => print_callable_named("callable", sig),
        Type::Closure(sig) if NAME_NODES.get() => print_callable_named("\\Closure", sig),
        Type::Closure(sig) => print_callable_named("Closure", sig),
        Type::Projection(c, inner) => description!(variance_name(*c), " ", to_phpdoc_bytes(inner)),
        Type::OffsetAccess(t, o) => {
            let on = to_phpdoc_bytes(t);
            let wrap = matches!(
                **t,
                Type::Union(_) | Type::Benevolent(_) | Type::Callable(Some(_)) | Type::Closure(_)
            ) || (matches!(**t, Type::Intersection(_)) && on.contains(&b'&'));
            if wrap {
                description!("(", on, ")[", to_phpdoc_bytes(o), "]")
            } else {
                description!(on, "[", to_phpdoc_bytes(o), "]")
            }
        }
        _ => t.describe_bytes(Level::Precise),
    }
}

fn join_phpdoc(types: &[Type], sep: &str) -> Vec<u8> {
    types
        .iter()
        .map(|t| {
            let s = to_phpdoc_bytes(t);
            if matches!(
                t,
                Type::Union(_) | Type::Benevolent(_) | Type::Intersection(_)
            ) && s.iter().any(|b| b"|&".contains(b))
            {
                description!("(", s, ")")
            } else {
                s
            }
        })
        .collect::<Vec<_>>()
        .join(sep.as_bytes())
}

fn print_callable_named(ident: &str, sig: &Signature) -> Vec<u8> {
    let params: Vec<_> = sig
        .params
        .iter()
        .map(|p| {
            let name = if p.name.is_empty() {
                String::new()
            } else {
                format!("${}", p.name)
            };
            let s = description!(
                to_phpdoc_bytes(&p.ty),
                " ",
                if p.by_ref { "&" } else { "" },
                if p.variadic { "..." } else { "" },
                name
            );
            description!(s.trim_ascii(), if p.optional { "=" } else { "" })
        })
        .collect();
    let ret = match &sig.ret {
        r @ (Type::Union(_)
        | Type::Benevolent(_)
        | Type::Intersection(_)
        | Type::Callable(Some(_))
        | Type::Closure(_)) => description!("(", to_phpdoc_bytes(r), ")"),
        r => to_phpdoc_bytes(r),
    };
    description!(
        ident,
        print_templates(sig),
        "(",
        params.join(b", ".as_slice()),
        "): ",
        ret
    )
}
