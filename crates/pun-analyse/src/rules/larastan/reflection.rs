//! Larastan's class reflection extensions: Eloquent models forward to their builders, builders to the query
//! builder, relations to their related model's builder, and macros, scopes and magic methods exist.

use mago_codex::metadata::class_like::ClassLikeMetadata;
use pun_phpstan::types::Type;

use super::super::class_reflection::{
    ClassReflectionExtension, MethodAnswer, MethodPhase, Reflected,
};
use super::super::method_reflection::class_display;
use super::super::reflection::Lookup;
use super::super::{Checker, declared, members};
use super::Larastan;

impl ClassReflectionExtension for Larastan {
    fn method(
        &self,
        c: &Checker,
        phase: MethodPhase,
        receiver: &Type,
        k: &ClassLikeMetadata,
        name: &[u8],
    ) -> Option<MethodAnswer> {
        // Larastan's own extensions come before PHPStan's annotations: a macro and the Storage facade's methods win
        // over a `@method` tag. The rest keep the later phases Pun asks them in.
        if phase == MethodPhase::BeforeAnnotations {
            return storage_method(c, k, name)
                .or_else(|| Some(self.macro_answer(c, k, &super::macro_method(c, k, name)?)));
        }
        if super::is_model(c, k) {
            // ModelForwardsCallsExtension: an EloquentBuilderMethodReflection of the model's builder, which is static.
            let builder = model_builder(c, k);
            let name = builder.object_class_names().concat();
            let display = (c.class_like(&name).found())
                .map_or(name.clone(), |b| class_display(c, &builder, b));
            return Some(MethodAnswer::Forward(
                builder,
                Reflected::EloquentBuilder(Some(display)),
            ));
        }
        let original = declared::original_name(k);
        let builder = super::is_a(
            c,
            k.name.as_bytes(),
            b"illuminate\\database\\eloquent\\builder",
        );
        // Larastan's stub drops the builder's `@mixin Query\Builder`, so PHPStan finds the query builder's methods
        // through EloquentBuilderForwardsCallsExtension, not through the mixin.
        if phase == MethodPhase::BeforeMixins && !builder {
            return None;
        }
        if !builder && let Some(m) = super::macro_method(c, k, name) {
            return Some(self.macro_answer(c, k, &m));
        }
        let args = match receiver {
            Type::Object(_, args, _) | Type::Static(_, args) => args.clone(),
            _ => Vec::new(),
        };
        let ancestor_args = |ancestor: &str| -> Vec<Type> {
            if original.eq_ignore_ascii_case(ancestor) {
                return args.clone();
            }
            declared::with_classes(c, || {
                pun_phpstan::symbols::lookup(&original)
                    .and_then(|k| pun_phpstan::types::ancestor_args(&k, ancestor, &args))
                    .unwrap_or_default()
            })
        };
        if builder {
            // EloquentBuilderForwardsCallsExtension reflects every method it finds, except a macro, as an
            // EloquentBuilderMethodReflection of this builder, which is static.
            let here = || class_display(c, receiver, k);
            if let Some(m) = super::macro_method(c, k, name) {
                return Some(self.macro_answer(c, k, &m));
            }
            let model = ancestor_args("Illuminate\\Database\\Eloquent\\Builder")
                .into_iter()
                .next()
                .map_or_else(
                    || {
                        Type::Object(
                            "Illuminate\\Database\\Eloquent\\Model".into(),
                            vec![],
                            false,
                        )
                    },
                    |m| m.resolve_to_bounds(),
                );
            for class in model.object_class_names() {
                if super::has_scope(c, &class, name) {
                    return Some(MethodAnswer::Exists(Some(here())));
                }
            }
            let query = "Illuminate\\Database\\Query\\Builder";
            if let Lookup::Found(q) = c.class_like(query)
                && members::method(c, q, name).is_some()
            {
                return Some(MethodAnswer::Forward(
                    Type::Object(query.into(), vec![], false),
                    Reflected::EloquentBuilder(Some(here())),
                ));
            }
            if let Some(finder) = name.strip_prefix(b"where") {
                let finder = String::from_utf8_lossy(finder);
                let segments = super::where_segments(&finder);
                if !self
                    .switches
                    .contains(crate::LarastanSwitches::CHECK_MODEL_PROPERTIES)
                    || segments.iter().all(|s| {
                        let property = format!("${}", super::snake(s));
                        c.has_property(&model, property.as_bytes(), false).yes()
                    })
                {
                    return Some(MethodAnswer::Exists(Some(here())));
                }
            }
            let soft_deletes = model.object_class_names().iter().any(|m| {
                c.codebase
                    .class_uses_trait(m.as_bytes(), b"Illuminate\\Database\\Eloquent\\SoftDeletes")
            });
            if soft_deletes
                && [
                    &b"withTrashed"[..],
                    b"onlyTrashed",
                    b"withoutTrashed",
                    b"restore",
                    b"createOrRestore",
                    b"restoreOrCreate",
                ]
                .contains(&name)
            {
                return Some(MethodAnswer::Exists(Some(here())));
            }
            return None;
        }
        if super::is_a(
            c,
            k.name.as_bytes(),
            b"illuminate\\database\\eloquent\\factories\\factory",
        ) && super::factory_magic_method(
            c.codebase,
            &mago_codex::ttype::atomic::object::named::TNamedObject::new(k.name),
            &String::from_utf8_lossy(name),
        ) {
            return Some(MethodAnswer::Exists(None));
        }
        if super::with_method_class(c.codebase, k.name.as_bytes(), name).is_some() {
            return Some(MethodAnswer::Exists(None));
        }
        if original.eq_ignore_ascii_case("Illuminate\\Support\\HigherOrderCollectionProxy") {
            let [proxied, value, _] = args.as_slice() else {
                return None;
            };
            if proxied.constant_strings().len() != 1
                || !c.can_call_methods(value).yes()
                || !c.has_method(value, name).yes()
            {
                return None;
            }
            return Some(MethodAnswer::Forward(value.clone(), Reflected::Instance));
        }
        if original.eq_ignore_ascii_case("Illuminate\\Support\\HigherOrderTapProxy") {
            let [target] = args.as_slice() else {
                return None;
            };
            if target.object_class_names().is_empty() || !c.has_method(target, name).yes() {
                return None;
            }
            return Some(MethodAnswer::Forward(target.clone(), Reflected::Declared));
        }
        if super::is_a(
            c,
            k.name.as_bytes(),
            b"illuminate\\database\\eloquent\\relations\\relation",
        ) {
            let related = ancestor_args("Illuminate\\Database\\Eloquent\\Relations\\Relation")
                .into_iter()
                .next()
                .filter(|r| !r.object_class_names().is_empty())
                .unwrap_or_else(|| {
                    Type::Object(
                        "Illuminate\\Database\\Eloquent\\Model".into(),
                        vec![],
                        false,
                    )
                });
            if !related.object_class_names().first().is_some_and(|m| {
                super::is_a(c, m.as_bytes(), b"illuminate\\database\\eloquent\\model")
            }) {
                return None;
            }
            let builder = match related.object_class_names().as_slice() {
                [model] => String::from_utf8_lossy(
                    super::builder_name(c.codebase, model.as_bytes()).as_bytes(),
                )
                .into_owned(),
                _ => "Illuminate\\Database\\Eloquent\\Builder".into(),
            };
            let builder = Type::Object(builder, vec![related], false);
            // RelationForwardsCallsExtension wraps the builder's method in a static EloquentBuilderMethodReflection.
            return Some(MethodAnswer::Forward(
                builder,
                Reflected::EloquentBuilder(None),
            ));
        }
        // AuthsMethodsExtension: the auth contracts have the methods of the first auth model (of every guard's
        // provider in config/auth.php) that declares one, each in a StaticMethodReflection
        if [
            "Illuminate\\Contracts\\Auth\\Authenticatable",
            "Illuminate\\Contracts\\Auth\\CanResetPassword",
            "Illuminate\\Contracts\\Auth\\Access\\Authorizable",
        ]
        .contains(&original.as_str())
        {
            return (self.app.auth_models(None).into_iter())
                .map(|model| Type::Object(model, vec![], false))
                .find(|model| c.has_method(model, name).yes())
                .map(|model| MethodAnswer::Forward(model, Reflected::StaticDeclared));
        }
        // A contract that is its key's first alias (`'queue.connection' => [Queue::class]`) resolves to itself.
        // Larastan's container builds a concrete class there; forwarding to the contract would never end.
        if k.kind.is_interface()
            && original.starts_with("Illuminate\\Contracts")
            && let Some(concrete) = self.app.resolve(&original)
            && let Lookup::Found(class) = c.class_like(concrete)
            && class.name != k.name
        {
            let concrete = Type::Object(declared::original_name(class), vec![], false);
            // ContractsMethodsExtension wraps the concrete class's method in a StaticMethodReflection.
            return Some(MethodAnswer::Forward(concrete, Reflected::StaticDeclared));
        }
        None
    }

    fn has_property(&self, c: &Checker, class: &ClassLikeMetadata, property: &[u8]) -> bool {
        super::has_model_property(c, class, property)
    }

    fn calls_inaccessible_method(&self, c: &Checker, receiver: &Type, method: &str) -> bool {
        ["increment", "decrement"]
            .iter()
            .any(|m| method.eq_ignore_ascii_case(m))
            && receiver
                .object_class_names()
                .iter()
                .all(|n| (c.class_like(n).found()).is_some_and(|k| super::is_model(c, k)))
    }

    fn static_forward(
        &self,
        c: &Checker,
        class: &ClassLikeMetadata,
        declaring: &str,
    ) -> Option<Type> {
        (super::is_model(c, class)
            && !(c.codebase).is_instance_of(class.name.as_bytes(), declaring.as_bytes()))
        .then(|| model_builder(c, class))
    }
}

impl Larastan {
    /// MacroMethodsClassReflectionExtension's reflection of a macro on `k`: a closure (or a mixin's closure) is a
    /// static `Macro` of the class whose macros were read, which is the contract's concrete class or `k` itself; a
    /// function is a `Macro` that isn't static. A method callable is the native method, whose class Pun doesn't keep,
    /// so it reports nothing.
    fn macro_answer(
        &self,
        c: &Checker,
        k: &ClassLikeMetadata,
        m: &crate::larastan::macros::Macro,
    ) -> MethodAnswer {
        MethodAnswer::Exists((!m.native && m.instance.is_none()).then(|| {
            let original = declared::original_name(k);
            match self.app.resolve(&original) {
                Some(concrete) if k.kind.is_interface() => concrete.to_string(),
                _ => c.display(k),
            }
        }))
    }
}

/// StorageMethodsClassReflectionExtension: the Storage facade has the methods of the filesystem manager, then of the
/// filesystem adapter, each in a StaticMethodReflection. It runs before the facade's own `@method` tags.
/// ClassReflection::getMethod() moves on to the next extension when the scope cannot call the method found, so the
/// manager's protected `get()` leaves `Storage::get()` to the facade's `@method` tag.
fn storage_method(c: &Checker, k: &ClassLikeMetadata, name: &[u8]) -> Option<MethodAnswer> {
    if !declared::original_name(k).eq_ignore_ascii_case("Illuminate\\Support\\Facades\\Storage") {
        return None;
    }
    [
        "Illuminate\\Filesystem\\FilesystemManager",
        "Illuminate\\Filesystem\\FilesystemAdapter",
    ]
    .into_iter()
    .find(|class| {
        c.has_method(&Type::Object((*class).into(), vec![], false), name)
            .yes()
    })
    .filter(|class| {
        c.class_like(class)
            .found()
            .and_then(|root| members::method(c, root, name))
            .is_none_or(|m| m.visibility().is_public())
    })
    .map(|class| Type::Object(class.into(), vec![], false))
    .map(|t| MethodAnswer::Forward(t, Reflected::StaticDeclared))
}

/// The builder a model's calls forward to, typed with the model.
fn model_builder(c: &Checker, model: &ClassLikeMetadata) -> Type {
    let builder = super::builder_name(c.codebase, model.name.as_bytes());
    Type::Object(
        String::from_utf8_lossy(builder.as_bytes()).into_owned(),
        vec![Type::Object(declared::original_name(model), vec![], false)],
        false,
    )
}
