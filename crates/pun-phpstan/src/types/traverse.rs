use super::{
    Accessory, Conditional, Param, Signature, Subject, Template, Traversal, Type, intersect,
    to_benevolent, union,
};

impl Type {
    pub fn generalize(&self) -> Type {
        match self {
            Type::ConstInt(_) | Type::IntRange(..) => Type::Int,
            Type::ConstFloat(_) => Type::Float,
            Type::ConstBool(_) => Type::Bool,
            Type::ConstString(_) => Type::String,
            Type::Accessory(a) if a.is_string_accessory() => Type::String,
            Type::ConstArray(a) => a.generalize(),
            Type::EnumCase(class, _) => Type::Object(class.clone(), vec![], false),
            _ => self.traverse(&Type::generalize),
        }
    }

    /// `generalize(GeneralizePrecision::moreSpecific())`: like `generalize()`,
    /// except that constant strings and constant arrays keep their accessories.
    pub fn generalize_more_specific(&self) -> Type {
        match self {
            Type::ConstString(s) => more_specific_string(s),
            Type::ConstArray(a) => a.generalize_more_specific(),
            Type::ConstInt(_)
            | Type::IntRange(..)
            | Type::ConstFloat(_)
            | Type::ConstBool(_)
            | Type::EnumCase(..) => self.generalize(),
            Type::Accessory(a) if a.is_string_accessory() => Type::String,
            _ => self.traverse(&Type::generalize_more_specific),
        }
    }

    pub fn traverse(&self, f: &dyn Fn(&Type) -> Type) -> Type {
        self.map_children(f)
    }

    pub fn traverse_source(&self, f: &dyn Fn(&Type) -> Traversal) -> Traversal {
        let replaced = std::cell::Cell::new(false);
        let mapped = self.map_children(&|inner| match f(inner) {
            Traversal::Retain => inner.clone(),
            Traversal::Replace(ty) => {
                replaced.set(true);
                ty
            }
        });
        if replaced.get() {
            Traversal::Replace(mapped)
        } else {
            Traversal::Retain
        }
    }

    fn map_children(&self, f: &dyn Fn(&Type) -> Type) -> Type {
        let sig = |s: &Signature| Signature {
            params: s
                .params
                .iter()
                .map(|p| Param {
                    ty: f(&p.ty),
                    ..p.clone()
                })
                .collect(),
            ret: f(&s.ret),
            templates: s.templates.clone(),
        };
        match self {
            Type::Reasoned(t, reason) => f(t).with_unresolvable_reason(reason.clone()),
            Type::SubtractedObject { object, excluded } => Type::SubtractedObject {
                object: Box::new(f(object)),
                excluded: Box::new(f(excluded)),
            },
            Type::LateArrayShape(s) => s.traverse(f),
            Type::Array(k, v) => Type::array(f(k), f(v)),
            Type::Accessory(Accessory::HasOffsetValue(k, v)) => {
                Type::Accessory(Accessory::HasOffsetValue(k.clone(), Box::new(f(v))))
            }
            Type::Accessory(Accessory::ModelProperty(t)) => {
                Type::Accessory(Accessory::ModelProperty(Box::new(f(t))))
            }
            Type::Iterable(k, v) => Type::Iterable(Box::new(f(k)), Box::new(f(v))),
            Type::Union(types) => union(types.iter().map(f).collect()),
            Type::Benevolent(types) => to_benevolent(union(types.iter().map(f).collect())),
            Type::Intersection(types) => {
                let mapped: Vec<Type> = types.iter().map(f).collect();
                if mapped == *types {
                    Type::Intersection(mapped)
                } else {
                    intersect(mapped)
                }
            }
            Type::Callable(Some(s)) => Type::Callable(Some(Box::new(sig(s)))),
            Type::Closure(s) => Type::Closure(Box::new(sig(s))),
            Type::Object(class, args, exact) if !args.is_empty() => {
                Type::Object(class.clone(), args.iter().map(f).collect(), *exact)
            }
            Type::Projection(c, t) => Type::Projection(*c, Box::new(f(t))),
            Type::OffsetAccess(t, o) => Type::OffsetAccess(Box::new(f(t)), Box::new(f(o))),
            Type::ClassString(Some(g)) => Type::ClassString(Some(Box::new(f(g)))),
            Type::Static(class, args) if !args.is_empty() => {
                Type::Static(class.clone(), args.iter().map(f).collect())
            }
            Type::Template(t) => {
                let mut bound = f(&t.bound);
                let default = t.default.as_ref().map(f);
                if matches!(bound, Type::StrictMixed) && (bound != t.bound || default != t.default)
                {
                    bound = Type::Mixed { explicit: true };
                }
                Type::Template(Box::new(Template {
                    bound,
                    default,
                    ..(**t).clone()
                }))
            }
            Type::Conditional(c) => Type::Conditional(Box::new(Conditional {
                subject: match &c.subject {
                    Subject::Type(t) => Subject::Type(f(t)),
                    p => p.clone(),
                },
                target: f(&c.target),
                then: f(&c.then),
                otherwise: f(&c.otherwise),
                negated: c.negated,
                resolved: Default::default(),
            })),
            Type::ConstArray(a) => {
                let mut a = a.clone();
                a.values = a.values.iter().map(f).collect();
                a.unsealed = a.unsealed.map(|(k, v)| (f(&k), f(&v)));
                Type::ConstArray(a)
            }
            Type::ObjectShape(props) => Type::ObjectShape(
                props
                    .iter()
                    .map(|(n, t, o)| (n.clone(), f(t), *o))
                    .collect(),
            ),
            _ => self.clone(),
        }
    }
}

// ConstantStringType::generalize() with GeneralizePrecision::moreSpecific().
fn more_specific_string(s: &[u8]) -> Type {
    let mut types = vec![Type::String, Type::Accessory(Accessory::LiteralString)];
    if s.is_empty() {
        return Type::Intersection(types);
    }
    if Type::ConstString(s.to_vec()).is_numeric_string().yes() {
        types.push(Type::Accessory(Accessory::NumericString));
    }
    types.push(Type::Accessory(if s == b"0" {
        Accessory::NonEmptyString
    } else {
        Accessory::NonFalsyString
    }));
    if s.to_ascii_lowercase() == s {
        types.push(Type::Accessory(Accessory::LowercaseString));
    }
    if s.to_ascii_uppercase() == s {
        types.push(Type::Accessory(Accessory::UppercaseString));
    }
    Type::Intersection(types)
}
