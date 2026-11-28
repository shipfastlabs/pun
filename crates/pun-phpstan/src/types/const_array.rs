use super::{
    Accessory, ConstArray, Level, NeverKind, Tri, Type, array_key, has_method, intersect,
    is_super_type_of, mixed, to_phpdoc_bytes, union,
};
use crate::symbols;

impl ConstArray {
    pub fn empty() -> ConstArray {
        ConstArray {
            keys: vec![],
            values: vec![],
            next_auto_indexes: vec![0],
            optional: vec![],
            is_list: Tri::Yes,
            unsealed: None,
        }
    }

    pub(super) fn is_optional_key(&self, i: usize) -> bool {
        self.optional.contains(&i)
    }

    pub(super) fn is_unsealed(&self) -> Tri {
        match &self.unsealed {
            None => Tri::Maybe,
            Some((key, _)) => Tri::from(*key != Type::Never(NeverKind::Explicit)),
        }
    }

    pub(super) fn is_iterable_at_least_once(&self) -> Tri {
        if self.keys.is_empty() {
            return if self.is_unsealed().yes() {
                Tri::Maybe
            } else {
                Tri::No
            };
        }
        if self.optional.len() < self.keys.len() {
            Tri::Yes
        } else {
            Tri::Maybe
        }
    }

    pub(super) fn intersect_array(&self, array: &Type) -> Type {
        let (array_key, item) = (array.iterable_key_type(), array.iterable_value_type());
        let mut b = ShapeBuilder::new();
        if self.is_unsealed().yes()
            && let Some((k, v)) = &self.unsealed
        {
            let k = intersect(vec![k.clone(), array_key.clone()]);
            let v = intersect(vec![v.clone(), item.clone()]);
            if !k.is_never() && !v.is_never() {
                b.make_unsealed(k, v);
            }
        }
        for (i, (key, value)) in self.keys.iter().zip(&self.values).enumerate() {
            if is_super_type_of(&array_key, key).no() {
                continue;
            }
            b.set(
                Some(intersect(vec![key.clone(), array_key.clone()])),
                intersect(vec![value.clone(), item.clone()]),
                self.is_optional_key(i),
            );
        }
        b.get()
    }

    pub(super) fn is_callable(&self) -> Tri {
        if (self.unsealed.is_none() || !self.is_unsealed().yes())
            && let Some(t) = self.callable_methods()
        {
            return t;
        }
        self.callable_shape()
    }

    pub(super) fn callable_parts(&self) -> Option<(&str, bool, Vec<&str>)> {
        if self.keys != [Type::ConstInt(0), Type::ConstInt(1)] {
            return None;
        }
        let methods: Vec<&str> = match &self.values[1] {
            Type::ConstString(m) => vec![std::str::from_utf8(m).ok()?],
            Type::Union(ts) => ts
                .iter()
                .map(|t| {
                    if let Type::ConstString(m) = t {
                        std::str::from_utf8(m).ok()
                    } else {
                        None
                    }
                })
                .collect::<Option<_>>()?,
            _ => return None,
        };
        match &self.values[0] {
            Type::ConstString(n) => Some((std::str::from_utf8(n).ok()?, true, methods)),
            Type::Object(n, _, _) | Type::Static(n, _) | Type::This(n) => Some((n, false, methods)),
            _ => None,
        }
    }

    fn callable_methods(&self) -> Option<Tri> {
        let (class, string, methods) = self.callable_parts()?;
        let class = symbols::lookup(class)?;
        let instance = |m: &str| {
            std::iter::once(class.name.as_str())
                .chain(class.ancestors.iter().map(String::as_str))
                .find_map(|n| symbols::lookup(n)?.methods.get(m).map(|f| !f.is_static))
                .unwrap_or(false)
        };
        let has: Vec<Tri> = methods
            .iter()
            .map(|m| m.to_ascii_lowercase())
            .filter(|m| !(string && has_method(&class, m) && instance(m)))
            .map(|m| match has_method(&class, &m) {
                true => Tri::Yes,
                false if class.is_final => Tri::No,
                false => Tri::Maybe,
            })
            .collect();
        if has.is_empty() {
            return Some(Tri::No);
        }
        let found: Vec<Tri> = has.iter().copied().filter(|t| !t.no()).collect();
        if found.is_empty() {
            return Some(Tri::No);
        }
        let optional = if self.optional.is_empty() {
            Tri::Yes
        } else {
            Tri::Maybe
        };
        let missing = if found.len() < has.len() {
            Tri::Maybe
        } else {
            Tri::Yes
        };
        Some(
            found
                .into_iter()
                .fold(Tri::Yes, Tri::and)
                .and(optional)
                .and(missing),
        )
    }

    fn callable_shape(&self) -> Tri {
        let unsealed = self.unsealed.as_ref().filter(|_| self.is_unsealed().yes());
        if self.keys.len() > 2
            || (unsealed.is_none() && self.keys.len() != 2)
            || self
                .keys
                .iter()
                .any(|k| !matches!(k, Type::ConstInt(0 | 1)))
        {
            return Tri::No;
        }
        let slot = |n: i64, expected: Type| {
            self.keys.contains(&Type::ConstInt(n))
                || unsealed.is_some_and(|(k, v)| {
                    !is_super_type_of(k, &Type::ConstInt(n)).no()
                        && !is_super_type_of(&expected, v).no()
                })
        };
        Tri::from(
            slot(
                0,
                Type::Union(vec![Type::ObjectWithoutClass, Type::ClassString(None)]),
            ) && slot(
                1,
                Type::Intersection(vec![
                    Type::String,
                    Type::Accessory(Accessory::NonFalsyString),
                ]),
            ),
        )
        .and(Tri::Maybe)
    }

    pub(super) fn iterable_key_type(&self) -> Type {
        let mut key = match self.keys.len() {
            0 => Type::Never(NeverKind::Explicit),
            1 => self.keys[0].clone(),
            _ => Type::Union(self.keys.clone()),
        };
        if self.is_unsealed().yes()
            && let Some((unsealed_key, _)) = &self.unsealed
        {
            let k = if matches!(unsealed_key, Type::Mixed { .. }) {
                array_key()
            } else {
                unsealed_key.clone()
            };
            key = union(vec![key, k]);
        }
        key
    }

    pub(super) fn iterable_value_type(&self) -> Type {
        let mut value = if self.values.is_empty() {
            Type::Never(NeverKind::Explicit)
        } else {
            union(self.values.clone())
        };
        if self.is_unsealed().yes()
            && let Some((_, v)) = &self.unsealed
        {
            value = union(vec![value, v.clone()]);
        }
        value
    }

    fn should_be_described_as_a_list(&self) -> bool {
        if !self.is_list.yes() || self.optional.is_empty() {
            return false;
        }
        self.optional.len() > 1 || self.optional[0] != self.keys.len() - 1
    }

    pub(super) fn describe(&self, level: Level) -> Vec<u8> {
        let name = if self.should_be_described_as_a_list() {
            "list"
        } else {
            "array"
        };
        if level == Level::TypeOnly {
            if self.is_iterable_at_least_once().no() {
                return name.into();
            }
            let key = self.iterable_key_type();
            if key.is_benevolent_array_key() {
                return description!(
                    name,
                    "<",
                    self.iterable_value_type().describe_bytes(level),
                    ">"
                );
            }
            return description!(
                name,
                "<",
                key.describe_bytes(level),
                ", ",
                self.iterable_value_type().describe_bytes(level),
                ">"
            );
        }
        let truncate = level == Level::Value;
        let mut items = Vec::new();
        let mut values = Vec::new();
        let mut values_only = true;
        for (i, key) in self.keys.iter().enumerate() {
            if *key != Type::ConstInt(i as i64) {
                values_only = false;
            }
            let optional = self.is_optional_key(i);
            if optional {
                values_only = false;
            }
            let key = match key {
                Type::ConstString(s) if s.contains(&b'"') => {
                    description!("'", s, "'")
                }
                Type::ConstString(s) if s.contains(&b'\'') => {
                    description!("\"", s, "\"")
                }
                Type::ConstString(s) if !is_valid_identifier(s) => {
                    description!("'", s, "'")
                }
                Type::ConstString(s) => s.clone(),
                k => to_phpdoc_bytes(k),
            };
            let value = self.values[i].describe_bytes(level);
            items.push(description!(
                key,
                if optional { "?" } else { "" },
                ": ",
                value
            ));
            values.push(value);
        }
        let mut append = Vec::new();
        if truncate && items.len() > 8 {
            items.truncate(8);
            values.truncate(8);
            append = ", ...".into();
        }
        if self.is_unsealed().yes()
            && let Some((key, value)) = &self.unsealed
        {
            if !items.is_empty() {
                append.extend_from_slice(b", ");
            }
            append.extend_from_slice(b"...");
            let key_description = key.describe_bytes(Level::Precise);
            if key.is_plain_implicit_mixed()
                || (self.is_list.yes() && key_description == b"int<0, max>")
            {
                if !value.is_plain_implicit_mixed() {
                    append.extend(description!("<", value.describe_bytes(level), ">"));
                }
            } else {
                append.extend(description!(
                    "<",
                    key.describe_bytes(level),
                    ", ",
                    value.describe_bytes(level),
                    ">"
                ));
            }
        }
        description!(
            name,
            "{",
            if values_only {
                values.join(b", ".as_slice())
            } else {
                items.join(b", ".as_slice())
            },
            append,
            "}"
        )
    }

    pub(super) fn to_phpdoc(&self) -> Vec<u8> {
        let mut name = if self.should_be_described_as_a_list() {
            "list"
        } else {
            "array"
        };
        let mut items = Vec::new();
        let mut values = Vec::new();
        let mut values_only = true;
        for (i, key) in self.keys.iter().enumerate() {
            let optional = self.is_optional_key(i);
            if *key != Type::ConstInt(i as i64) || optional {
                values_only = false;
            }
            let key = match key {
                Type::ConstString(s) if is_valid_identifier(s) => s.clone(),
                key => to_phpdoc_bytes(key),
            };
            let value = to_phpdoc_bytes(&self.values[i]);
            items.push(description!(
                key,
                if optional { "?" } else { "" },
                ": ",
                value
            ));
            values.push(value);
        }
        let mut items = if values_only { values } else { items };
        if self.is_unsealed().yes()
            && let Some((key, value)) = &self.unsealed
        {
            let tail = if key.is_plain_implicit_mixed()
                || (self.is_list.yes() && key.describe_bytes(Level::Precise) == b"int<0, max>")
            {
                if value.is_plain_implicit_mixed() {
                    "...".into()
                } else {
                    description!("...<", to_phpdoc_bytes(value), ">")
                }
            } else {
                name = "array";
                description!(
                    "...<",
                    to_phpdoc_bytes(key),
                    ", ",
                    to_phpdoc_bytes(value),
                    ">"
                )
            };
            items.push(tail);
        }
        description!(name, "{", items.join(b", ".as_slice()), "}")
    }

    pub(super) fn generalize(&self) -> Type {
        if self.keys.is_empty() && !self.is_unsealed().yes() {
            return Type::ConstArray(Box::new(self.clone()));
        }
        let mut array = Type::array(
            self.iterable_key_type().generalize(),
            self.iterable_value_type().generalize(),
        );
        let mut accessories = Vec::new();
        if self.is_iterable_at_least_once().yes() {
            accessories.push(Type::Accessory(Accessory::NonEmptyArray));
        }
        if self.is_list.yes() {
            array = intersect(vec![array, Type::Accessory(Accessory::List)]);
        }
        if !accessories.is_empty() {
            accessories.insert(0, array);
            return intersect(accessories);
        }
        array
    }

    // ConstantArrayType::generalize() with GeneralizePrecision::moreSpecific().
    pub(super) fn generalize_more_specific(&self) -> Type {
        if self.keys.is_empty() && !self.is_unsealed().yes() {
            return Type::ConstArray(Box::new(self.clone()));
        }
        let mut array = Type::array(
            self.iterable_key_type().generalize_more_specific(),
            self.iterable_value_type().generalize_more_specific(),
        );
        let mut accessories = Vec::new();
        if self.keys.len() - self.optional.len() < 32 {
            for (i, key) in self.keys.iter().enumerate() {
                if !self.is_optional_key(i) {
                    accessories.push(Type::Accessory(Accessory::HasOffsetValue(
                        Box::new(key.clone()),
                        Box::new(self.values[i].generalize_more_specific()),
                    )));
                }
            }
        } else {
            // PHPStan also checks isIterableAtLeastOnce() here, which 32
            // required keys always pass.
            accessories.push(Type::Accessory(Accessory::NonEmptyArray));
        }
        if self.is_list.yes() {
            array = intersect(vec![array, Type::Accessory(Accessory::List)]);
        }
        if !accessories.is_empty() {
            accessories.insert(0, array);
            return intersect(accessories);
        }
        array
    }

    pub(super) fn make_list(&self) -> ConstArray {
        let mut a = self.clone();
        if !a.is_list.no() {
            a.is_list = Tri::Yes;
        }
        a
    }

    pub(super) fn make_first_offset_required(&self) -> ConstArray {
        let mut a = self.clone();
        a.optional.retain(|&i| i != 0);
        a
    }
}

pub(super) fn is_valid_identifier(s: &[u8]) -> bool {
    let mut bytes = s.iter().copied().peekable();
    let mut any = false;
    while bytes.peek().is_some() {
        if bytes.peek() == Some(&b'\\') {
            bytes.next();
        }
        match bytes.next() {
            Some(b) if b.is_ascii_alphabetic() || b == b'_' || b >= 0x80 => {}
            _ => return false,
        }
        while bytes
            .peek()
            .is_some_and(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80)
        {
            bytes.next();
        }
        any = true;
    }
    any
}

/// `ConstantArrayTypeBuilder::ARRAY_COUNT_LIMIT`: a constant array literal with
/// more items is built by `oversized_array`.
pub const ARRAY_COUNT_LIMIT: usize = 256;

/// PHPStan's `OversizedArrayBuilder::build()`, after unpacked items are
/// expanded: each item is a key type (`None` for an implicit key) and a value
/// type. Keys and values are generalized and deduplicated by their precise
/// description, so the result stays small however many items there are.
pub fn oversized_array(entries: impl IntoIterator<Item = (Option<Type>, Type)>) -> Type {
    fn add(seen: &mut Vec<(Vec<u8>, Type)>, t: Type) {
        let description = t.describe_bytes(Level::Precise);
        if !seen.iter().any(|(d, _)| *d == description) {
            seen.push((description, t));
        }
    }
    let mut is_list = true;
    let mut next_auto_index = Some(0_i64);
    let (mut keys, mut values) = (Vec::new(), Vec::new());
    for (key, value) in entries {
        let key = match key {
            Some(key) => {
                match key {
                    Type::ConstInt(i) if Some(i) == next_auto_index => {
                        next_auto_index = i.checked_add(1);
                    }
                    Type::ConstInt(i) => {
                        is_list = false;
                        next_auto_index = i.checked_add(1);
                    }
                    _ => is_list = false,
                }
                key
            }
            None => match next_auto_index {
                Some(i) => {
                    next_auto_index = i.checked_add(1);
                    Type::ConstInt(i)
                }
                None => {
                    is_list = false;
                    Type::Int
                }
            },
        };
        add(&mut keys, key.generalize_more_specific());
        add(&mut values, value.generalize_more_specific());
    }
    let array = Type::array(
        union(keys.into_iter().map(|(_, t)| t).collect()),
        union(values.into_iter().map(|(_, t)| t).collect()),
    );
    let mut types = vec![
        array,
        Type::Accessory(Accessory::NonEmptyArray),
        Type::Accessory(Accessory::OversizedArray),
    ];
    if is_list {
        types.push(Type::Accessory(Accessory::List));
    }
    intersect(types)
}

pub struct ShapeBuilder {
    array: ConstArray,
}

impl ShapeBuilder {
    pub fn new() -> Self {
        ShapeBuilder {
            array: ConstArray::empty(),
        }
    }

    pub fn set(&mut self, key: Option<Type>, value: Type, optional: bool) {
        let key = key.as_ref().map(crate::resolve::to_array_key);
        let a = &mut self.array;
        let Some(key) = key else {
            let max = *a.next_auto_indexes.iter().max().expect("auto index");
            a.keys.push(Type::ConstInt(max));
            a.values.push(value);
            a.next_auto_indexes = vec![max + 1];
            return;
        };
        if let Some(i) = a.keys.iter().position(|k| *k == key) {
            a.values[i] = if optional {
                union(vec![value, a.values[i].clone()])
            } else {
                value
            };
            if !optional {
                a.optional.retain(|&o| o != i);
            }
            return;
        }
        a.keys.push(key.clone());
        a.values.push(value);
        let non_list = if optional {
            a.is_list.and(Tri::Maybe)
        } else {
            Tri::No
        };
        match key {
            Type::ConstInt(offset) if !a.next_auto_indexes.is_empty() => {
                let min = *a.next_auto_indexes.iter().min().expect("auto index");
                let max = *a.next_auto_indexes.iter().max().expect("auto index");
                if offset >= 0 {
                    if offset > min {
                        a.is_list = if offset <= max {
                            a.is_list.and(Tri::Maybe)
                        } else {
                            non_list
                        };
                    }
                } else {
                    a.is_list = non_list;
                }
                if offset >= max {
                    let next = offset.checked_add(1);
                    if !optional {
                        a.next_auto_indexes = next.into_iter().collect();
                    } else {
                        a.next_auto_indexes.extend(next);
                    }
                }
            }
            _ => a.is_list = non_list,
        }
        if optional {
            a.optional.push(a.keys.len() - 1);
        }
    }

    pub fn make_unsealed(&mut self, key: Type, value: Type) {
        self.array.unsealed = Some((key, value));
    }

    pub fn get(self) -> Type {
        let mut array = self.array;
        if array.keys.is_empty() {
            if let Some((key, value)) = array.unsealed.clone()
                && key != Type::Never(NeverKind::Explicit)
            {
                return Type::array(key, value);
            }
            return Type::ConstArray(Box::new(array));
        }
        if let Some((key, _)) = &mut array.unsealed
            && key.is_benevolent_array_key()
        {
            *key = mixed();
        }
        Type::ConstArray(Box::new(array))
    }
}

impl Default for ShapeBuilder {
    fn default() -> Self {
        Self::new()
    }
}
