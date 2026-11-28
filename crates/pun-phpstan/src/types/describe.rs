use super::{
    Accessory, Level, NeverKind, Signature, Subject, Type, canonical_class, compare_strings, mixed,
    sort_types, static_object, static_of, to_phpdoc_bytes, union, variance_name,
};
use crate::symbols;

impl Type {
    pub fn describe(&self, level: Level) -> String {
        display_bytes(&self.describe_bytes(level))
    }

    pub(super) fn describe_bytes(&self, level: Level) -> Vec<u8> {
        let _scope = UnionDescriptions::enter();
        self.describe_with_subtraction(level, None)
    }

    fn describe_with_subtraction(&self, level: Level, excluded: Option<&Type>) -> Vec<u8> {
        match self {
            Type::Reasoned(t, _) => t.describe_bytes(level),
            Type::LateArrayShape(s) => s.print(false),
            Type::Mixed { .. } | Type::StrictMixed => "mixed".into(),
            Type::SubtractedObject { object, excluded } => {
                object.describe_with_subtraction(level, Some(excluded))
            }
            Type::SubtractedMixed { excluded, .. } => level.handle3(
                || "mixed".into(),
                || "mixed".into(),
                || {
                    let description = excluded.describe_bytes(level);
                    let mut class = excluded.unreasoned();
                    while let Type::Template(t) = class {
                        class = t.bound.unreasoned();
                    }
                    if matches!(
                        class,
                        Type::Union(_)
                            | Type::Benevolent(_)
                            | Type::SubtractedMixed { .. }
                            | Type::SubtractedObject { .. }
                    ) {
                        description!("mixed~(", description, ")")
                    } else {
                        description!("mixed~", description)
                    }
                },
            ),
            Type::Error => level.handle3(|| "mixed".into(), || "mixed".into(), || "*ERROR*".into()),
            Type::Never(NeverKind::NonAccepting) => "never".into(),
            Type::Never(_) => "*NEVER*".into(),
            Type::Void => "void".into(),
            Type::Null => "null".into(),
            Type::Bool => "bool".into(),
            Type::ConstBool(b) => (if *b { "true" } else { "false" }).into(),
            Type::Int => "int".into(),
            Type::ConstInt(i) => level.handle(|| "int".into(), || i.to_string().into_bytes()),
            Type::IntRange(min, max) => format!(
                "int<{}, {}>",
                min.map_or("min".to_string(), |v| v.to_string()),
                max.map_or("max".to_string(), |v| v.to_string())
            )
            .into_bytes(),
            Type::Float => "float".into(),
            Type::ConstFloat(f) => {
                level.handle(|| "float".into(), || float_to_string(*f).into_bytes())
            }
            Type::String | Type::StringAcceptingStringable | Type::StringNotAcceptingStringable => {
                "string".into()
            }
            Type::ConstString(s) => level.handle3(
                || "string".into(),
                || {
                    if std::str::from_utf8(s)
                        .ok()
                        .and_then(symbols::lookup)
                        .is_some_and(|c| c.name.as_bytes() == s)
                    {
                        export_string(s)
                    } else {
                        match std::str::from_utf8(s) {
                            Ok(s) => export_string(crate::schema::truncate(s, 20, "…").as_bytes()),
                            Err(_) if String::from_utf8_lossy(s).chars().count() <= 20 => {
                                export_string(s)
                            }
                            Err(_) => {
                                export_string(&[&s[..s.len().min(20)], "…".as_bytes()].concat())
                            }
                        }
                    }
                },
                || export_string(s),
            ),
            Type::ClassString(None) => "class-string".into(),
            Type::ClassString(Some(t)) => {
                description!("class-string<", t.describe_bytes(level), ">")
            }
            Type::Resource => "resource".into(),
            Type::ObjectWithoutClass => "object".into(),
            Type::NonexistentParent => "parent".into(),
            Type::Object(class, args, _) => {
                let mut name = match level {
                    Level::Precise => class.clone(),
                    _ => canonical_class(class),
                }
                .into_bytes();
                if level == Level::Precise
                    && let Some(excluded) = excluded
                {
                    let description = excluded.describe_bytes(level);
                    if matches!(
                        excluded,
                        Type::Union(_)
                            | Type::Benevolent(_)
                            | Type::SubtractedMixed { .. }
                            | Type::SubtractedObject { .. }
                    ) {
                        name.extend(description!("~(", description, ")"));
                    } else {
                        name.extend(description!("~", description));
                    }
                }
                if args.is_empty() {
                    name
                } else {
                    let args: Vec<_> = args.iter().map(|a| a.describe_bytes(level)).collect();
                    description!(name, "<", args.join(b", ".as_slice()), ">")
                }
            }
            Type::Static(class, args) => {
                description!(
                    "static(",
                    static_of(class, args).describe_with_subtraction(level, excluded),
                    ")"
                )
            }
            Type::This(class) => description!(
                "$this(",
                static_object(class).describe_with_subtraction(level, excluded),
                ")"
            ),
            Type::EnumCase(class, case) => format!("{class}::{case}").into_bytes(),
            Type::Wildcard => "*".into(),
            Type::Projection(c, t) => description!(variance_name(*c), " ", t.describe_bytes(level)),
            Type::OffsetAccess(..) => to_phpdoc_bytes(self),
            Type::Array(key, item) => {
                let mixed_key = key.is_plain_implicit_mixed();
                let mixed_item = item.is_plain_implicit_mixed();
                if mixed_key || (level != Level::Precise && key.is_never()) {
                    if mixed_item || (level != Level::Precise && item.is_never()) {
                        return "array".into();
                    }
                    return description!("array<", item.describe_bytes(level), ">");
                }
                description!(
                    "array<",
                    key.describe_bytes(level),
                    ", ",
                    item.describe_bytes(level),
                    ">"
                )
            }
            Type::ConstArray(a) => a.describe(level),
            Type::Iterable(key, item) => {
                if matches!(**key, Type::Mixed { .. }) {
                    if matches!(**item, Type::Mixed { .. }) {
                        return "iterable".into();
                    }
                    return description!("iterable<", item.describe_bytes(level), ">");
                }
                description!(
                    "iterable<",
                    key.describe_bytes(level),
                    ", ",
                    item.describe_bytes(level),
                    ">"
                )
            }
            Type::Callable(sig) => level.handle(
                || "callable".into(),
                || {
                    print_callable(
                        "callable",
                        sig.as_deref().unwrap_or(&Signature {
                            params: vec![],
                            ret: mixed(),
                            templates: vec![],
                        }),
                    )
                },
            ),
            Type::Closure(sig) => {
                level.handle(|| "Closure".into(), || print_callable("Closure", sig))
            }
            Type::Union(types) => describe_union(types, level),
            Type::Benevolent(types) => description!("(", describe_union(types, level), ")"),
            Type::Intersection(types) => match level {
                Level::TypeOnly => describe_intersection_type(self, types, level),
                _ => describe_intersection_itself(self, types, level),
            },
            Type::Accessory(a) => a.describe(level),
            Type::Conditional(c) => {
                let subject = match &c.subject {
                    Subject::Parameter(p) => p.as_bytes().to_vec(),
                    Subject::Type(t) => t.describe_bytes(level),
                };
                description!(
                    "(",
                    subject,
                    " ",
                    if c.negated { "is not" } else { "is" },
                    " ",
                    c.target.describe_bytes(level),
                    " ? ",
                    c.then.describe_bytes(level),
                    " : ",
                    c.otherwise.describe_bytes(level),
                    ")"
                )
            }
            Type::Template(t) => {
                let mut basic = t.name.as_bytes().to_vec();
                if !t.bound.is_implicit_or_explicit_plain_mixed() {
                    basic.extend_from_slice(b" of ");
                    basic.extend(t.bound.describe_bytes(level));
                }
                if let Some(default) = &t.default {
                    basic.extend_from_slice(b" = ");
                    basic.extend(default.describe_bytes(level));
                }
                if level == Level::Precise {
                    description!(
                        basic,
                        " (",
                        t.scope,
                        ", ",
                        if t.argument { "argument" } else { "parameter" },
                        ")"
                    )
                } else {
                    basic
                }
            }
            Type::ObjectShape(props) => {
                let items: Vec<_> = props
                    .iter()
                    .map(|(name, ty, optional)| {
                        description!(
                            name,
                            if *optional { "?" } else { "" },
                            ": ",
                            ty.describe_bytes(level)
                        )
                    })
                    .collect();
                description!("object{", items.join(b", ".as_slice()), "}")
            }
        }
    }

    fn is_implicit_or_explicit_plain_mixed(&self) -> bool {
        matches!(self, Type::Mixed { .. })
    }
}

impl Accessory {
    fn describe(&self, level: Level) -> Vec<u8> {
        match self {
            Accessory::NonEmptyString => "non-empty-string",
            Accessory::NonFalsyString => "non-falsy-string",
            Accessory::NumericString => "numeric-string",
            Accessory::LiteralString => "literal-string",
            Accessory::LowercaseString => "lowercase-string",
            Accessory::UppercaseString => "uppercase-string",
            Accessory::DecimalIntString { inverse: false } => "decimal-int-string",
            Accessory::DecimalIntString { inverse: true } => "non-decimal-int-string",
            Accessory::List => "list",
            Accessory::NonEmptyArray => "non-empty-array",
            Accessory::OversizedArray => "oversized-array",
            Accessory::HasMethod(name) => return format!("hasMethod({name})").into_bytes(),
            Accessory::ModelProperty(t) => {
                return description!("model property of ", t.describe_bytes(level));
            }
            Accessory::ViewString => "view-string",
            Accessory::HasOffsetValue(key, value) => {
                return description!(
                    "hasOffsetValue(",
                    key.describe_bytes(level),
                    ", ",
                    value.describe_bytes(level),
                    ")"
                );
            }
        }
        .into()
    }

    pub(super) fn is_string_accessory(&self) -> bool {
        !matches!(
            self,
            Accessory::List
                | Accessory::NonEmptyArray
                | Accessory::OversizedArray
                | Accessory::HasMethod(_)
                | Accessory::HasOffsetValue(..)
        )
    }
}

pub fn float_to_string(f: f64) -> String {
    if f.is_nan() {
        return "NAN".to_string();
    }
    if f.is_infinite() {
        return (if f > 0.0 { "INF" } else { "-INF" }).to_string();
    }
    let s = format!("{f:?}");
    let s = s.strip_suffix(".0").map_or(s.clone(), |x| format!("{x}.0"));
    if let Some((mantissa, exp)) = s.split_once('e') {
        let exp: i32 = exp.parse().expect("exponent");
        let mantissa = if mantissa.contains('.') {
            mantissa.to_string()
        } else {
            format!("{mantissa}.0")
        };
        return format!(
            "{mantissa}E{}{}",
            if exp < 0 { "-" } else { "+" },
            exp.abs()
        );
    }
    s
}

fn export_string(value: &[u8]) -> Vec<u8> {
    let double = value.iter().any(|b| *b < 0x20);
    let quote = if double { b'"' } else { b'\'' };
    let mut out = vec![quote];
    for &byte in value {
        match byte {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b if b == quote => out.extend_from_slice(&[b'\\', b]),
            b'\n' if double => out.extend_from_slice(b"\\n"),
            b'\t' if double => out.extend_from_slice(b"\\t"),
            b'\r' if double => out.extend_from_slice(b"\\r"),
            0x0b if double => out.extend_from_slice(b"\\v"),
            0x0c if double => out.extend_from_slice(b"\\f"),
            0x07 if double => out.extend_from_slice(b"\\a"),
            0x08 if double => out.extend_from_slice(b"\\b"),
            b if b < 0x20 => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            b => out.push(b),
        }
    }
    out.push(quote);
    out
}

pub(super) fn display_bytes(out: &[u8]) -> String {
    let mut display = String::new();
    let mut bytes = out;
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                display.push_str(valid);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                display.push_str(std::str::from_utf8(&bytes[..valid]).unwrap());
                bytes = &bytes[valid..];
                let width = match bytes[0] {
                    0xc2..=0xdf => 2,
                    0xe0..=0xef => 3,
                    0xf0..=0xf4 => 4,
                    _ => 1,
                };
                let consumed = 1 + bytes
                    .iter()
                    .skip(1)
                    .take(width - 1)
                    .take_while(|&&byte| !(byte < 0x80 || (0xc2..=0xf4).contains(&byte)))
                    .count();
                display.push('�');
                bytes = &bytes[consumed..];
            }
        }
    }
    display
}

impl Type {
    pub(super) fn array_class(&self) -> &Type {
        match self {
            Type::Template(t)
                if matches!(
                    t.bound,
                    Type::Array(..) | Type::ConstArray(_) | Type::Iterable(..)
                ) =>
            {
                &t.bound
            }
            _ => self,
        }
    }
}

/// PHPStan's UnionType keeps its sorted members and each verbosity level's description. Pun's types are plain
/// values, so a union's description is kept for the length of the outermost `describe_bytes()` instead. Without it,
/// sorting, joining and generalizing describe each member again, which is exponential in the nesting depth (#43).
struct UnionDescriptions;

thread_local! {
    static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static UNIONS: std::cell::RefCell<std::collections::HashMap<String, Vec<u8>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

impl UnionDescriptions {
    fn enter() -> Self {
        DEPTH.set(DEPTH.get() + 1);
        UnionDescriptions
    }
}

impl Drop for UnionDescriptions {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
        if DEPTH.get() == 0 {
            UNIONS.with_borrow_mut(std::collections::HashMap::clear);
        }
    }
}

fn describe_union(types: &[Type], level: Level) -> Vec<u8> {
    let key = format!("{level:?}{types:?}");
    if let Some(described) = UNIONS.with_borrow(|unions| unions.get(&key).cloned()) {
        return described;
    }
    let described = describe_union_uncached(types, level);
    UNIONS.with_borrow_mut(|unions| unions.insert(key, described.clone()));
    described
}

fn describe_union_uncached(types: &[Type], level: Level) -> Vec<u8> {
    let join = |types: &[Type]| -> Vec<u8> {
        let mut names = Vec::new();
        for (i, t) in types.iter().enumerate() {
            let d = t.describe_bytes(level);
            let wrap = match t {
                Type::Closure(_) | Type::Callable(_) => true,
                Type::Template(_) if matches!(t.scalar_class_type(), Type::Union(_)) => true,
                Type::Template(tpl) => {
                    i + 1 < types.len()
                        && level != Level::Precise
                        && !matches!(tpl.bound, Type::Mixed { .. })
                }
                Type::Intersection(_) => d.contains(&b'&'),
                _ => false,
            };
            names.push(if wrap { description!("(", d, ")") } else { d });
        }
        if level == Level::Precise {
            let mut counts = std::collections::HashMap::new();
            for name in &names {
                *counts.entry(name.clone()).or_insert(0) += 1;
            }
            let mut indices = std::collections::HashMap::new();
            for name in &mut names {
                if counts[name] > 1 {
                    let index = indices.entry(name.clone()).or_insert(0);
                    *index += 1;
                    name.extend(format!("#{index}").into_bytes());
                }
            }
        } else {
            let mut seen = std::collections::HashSet::new();
            names.retain(|n| seen.insert(n.clone()));
        }
        if names.len() > 1024 {
            names.truncate(1024);
            names.push("…".into());
        }
        names.join(b"|".as_slice())
    };
    let sorted = sort_types(types);
    if level != Level::TypeOnly {
        return join(&sorted);
    }
    let generalized = union(
        sorted
            .iter()
            .map(|t| {
                if t.is_constant_value().yes() && !matches!(t, Type::ConstBool(_)) {
                    match t.scalar_class_type() {
                        scalar @ (Type::ConstString(_) | Type::ConstInt(_)) => scalar.generalize(),
                        Type::Bool | Type::String | Type::Int if matches!(t, Type::Template(_)) => {
                            t.clone()
                        }
                        _ => t.generalize(),
                    }
                } else {
                    t.clone()
                }
            })
            .collect(),
    );
    match generalized {
        Type::Union(types) => join(&sort_types(&types)),
        other => join(&[other]),
    }
}

fn larastan_string(types: &[Type]) -> Option<&Accessory> {
    types.iter().find_map(|t| match t {
        Type::Accessory(a @ (Accessory::ModelProperty(_) | Accessory::ViewString)) => Some(a),
        _ => None,
    })
}

fn describe_intersection_type(this: &Type, types: &[Type], level: Level) -> Vec<u8> {
    if let Some(a) = larastan_string(types) {
        return a.describe(level);
    }
    let is_list = this.is_list().yes();
    let mut names = Vec::new();
    let mut value_type = None;
    for t in sort_types(types) {
        if is_list {
            if matches!(t, Type::Array(..) | Type::ConstArray(_)) {
                value_type = Some(t.iterable_value_type());
                continue;
            }
            if matches!(t, Type::Accessory(Accessory::NonEmptyArray)) {
                continue;
            }
        }
        if matches!(t, Type::Accessory(_)) {
            continue;
        }
        names.push(t.generalize().describe_bytes(level));
    }
    if is_list {
        let inner = match value_type {
            Some(v) if !v.is_plain_implicit_mixed() => {
                description!("<", v.describe_bytes(level), ">")
            }
            _ => Vec::new(),
        };
        names.push(description!("list", inner));
    }
    names.sort_by(|a, b| compare_strings(a, b));
    names.join(b"&".as_slice())
}

fn describe_intersection_itself(this: &Type, types: &[Type], level: Level) -> Vec<u8> {
    if let Some(a) = larastan_string(types) {
        return a.describe(level);
    }
    let mut base: Vec<(usize, &Type)> = Vec::new();
    let mut to_describe: Vec<(usize, Type)> = Vec::new();
    let mut skip_names: Vec<&[u8]> = Vec::new();
    let is_list = this.is_list().yes();
    let is_array = this.is_array().yes();
    let is_non_empty_array = this.is_iterable_at_least_once().yes();
    let has_template_array = types
        .iter()
        .any(|t| matches!(t, Type::Template(t) if matches!(t.bound, Type::Array(..))));
    let mut described = Vec::new();
    let sorted = sort_types(types);
    for (i, t) in sorted.iter().enumerate() {
        if level != Level::Precise
            && matches!(
                t,
                Type::Accessory(
                    Accessory::HasMethod(_)
                        | Accessory::HasOffsetValue(..)
                        | Accessory::OversizedArray
                )
            )
        {
            continue;
        }
        if let Type::Accessory(a) = t
            && a.is_string_accessory()
        {
            if matches!(a, Accessory::LowercaseString | Accessory::UppercaseString)
                && level != Level::Precise
            {
                continue;
            }
            to_describe.push((i, t.clone()));
            skip_names.push(b"string");
            continue;
        }
        if is_list || is_array {
            match t {
                Type::Array(key, value) => {
                    if is_list {
                        let inner = if value.is_plain_implicit_mixed() {
                            Vec::new()
                        } else {
                            description!("<", value.describe_bytes(level), ">")
                        };
                        described.push((
                            i,
                            description!(
                                if is_non_empty_array {
                                    "non-empty-list"
                                } else {
                                    "list"
                                },
                                inner
                            ),
                        ));
                    } else {
                        let inner = if !key.is_plain_implicit_mixed() {
                            description!(
                                "<",
                                key.describe_bytes(level),
                                ", ",
                                value.describe_bytes(level),
                                ">"
                            )
                        } else if !value.is_plain_implicit_mixed() {
                            description!("<", value.describe_bytes(level), ">")
                        } else {
                            Vec::new()
                        };
                        let kind = if is_non_empty_array {
                            "non-empty-array"
                        } else {
                            "array"
                        };
                        described.push((i, description!(kind, inner)));
                    }
                    continue;
                }
                Type::ConstArray(a) => {
                    let d = a.describe(level);
                    let rest = d
                        .strip_prefix(b"list")
                        .unwrap_or_else(|| &d["array".len()..]);
                    let mut begin = String::from(if is_list { "list" } else { "array" });
                    if is_non_empty_array && !t.is_iterable_at_least_once().yes() {
                        begin = format!("non-empty-{begin}");
                    }
                    described.push((i, description!(begin, rest)));
                    continue;
                }
                Type::Accessory(Accessory::NonEmptyArray | Accessory::List) => {
                    if has_template_array {
                        described.push((i, t.describe_bytes(level)));
                    }
                    continue;
                }
                _ => {}
            }
        }
        if matches!(t, Type::Callable(None)) {
            to_describe.push((i, t.clone()));
            skip_names.push(b"object");
            skip_names.push(b"string");
            continue;
        }
        base.push((i, t));
    }
    'base: for (i, t) in base {
        let d = t.describe_bytes(level);
        if (d == b"object" || d == b"string")
            && skip_names.contains(&d.as_slice())
            && let Some(pos) = to_describe
                .iter()
                .position(|(_, x)| matches!(x, Type::Callable(None)))
        {
            described.push((i, description!("callable-", d)));
            to_describe.remove(pos);
            continue 'base;
        }
        if skip_names.contains(&d.as_slice()) {
            continue;
        }
        described.push((i, d));
    }
    for (i, t) in to_describe {
        described.push((i, t.describe_bytes(level)));
    }
    described.sort_by_key(|(i, _)| *i);
    described
        .into_iter()
        .map(|(_, d)| d)
        .collect::<Vec<_>>()
        .join(b"&".as_slice())
}

fn print_callable(ident: &str, sig: &Signature) -> Vec<u8> {
    let params: Vec<_> = sig
        .params
        .iter()
        .map(|p| {
            description!(
                to_phpdoc_bytes(&p.ty),
                if p.variadic { " ..." } else { "" },
                if p.optional && !p.variadic { "=" } else { "" }
            )
        })
        .collect();
    let ret = to_phpdoc_bytes(&sig.ret);
    let ret = if matches!(
        sig.ret,
        Type::Union(_)
            | Type::Benevolent(_)
            | Type::Intersection(_)
            | Type::Callable(Some(_))
            | Type::Closure(_)
    ) && ret.iter().any(|b| b"|&(".contains(b))
    {
        description!("(", ret, ")")
    } else {
        ret
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

pub(super) fn print_templates(sig: &Signature) -> Vec<u8> {
    if sig.templates.is_empty() {
        return Vec::new();
    }
    let tags: Vec<_> = sig
        .templates
        .iter()
        .map(|t| description!(t.name, " of ", to_phpdoc_bytes(&t.bound)))
        .collect();
    description!("<", tags.join(b", ".as_slice()), ">")
}
