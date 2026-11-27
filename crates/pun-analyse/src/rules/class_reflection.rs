//! PHPStan's class reflection extensions: the methods and properties an extension adds to a class.

use mago_codex::metadata::class_like::ClassLikeMetadata;
use pun_phpstan::types::Type;

use super::Checker;

/// Where the method lookup asks the extensions. PHPStan asks them once, after a class's own methods. Pun asks twice,
/// so that a forward found before `@mixin` keeps its place and the rest still comes after `@phpstan-require-extends`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum MethodPhase {
    /// After the class's native methods, before its `@method` tags, where PHPStan asks every extension it does not
    /// wire itself. Pun asks here only for the extensions that ignore a `@method` tag.
    BeforeAnnotations,
    /// After the class's own methods, before `@mixin`. A forward that finds nothing lets the lookup go on.
    BeforeMixins,
    /// After `@mixin` and `@phpstan-require-extends`: the lookup's last step.
    Last,
}

/// An extension's answer to a method lookup.
pub(super) enum MethodAnswer {
    /// The method is the one this type has, reflected as the second field says.
    Forward(Type, Reflected),
    /// The method exists, with a signature Pun doesn't know; `Some` names its declaring class when PHPStan's
    /// reflection of it is static.
    Exists(Option<String>),
}

/// What the extension's method reflection answers for `isStatic()` and `getDeclaringClass()`, which
/// strict-rules' `staticMethod.dynamicCall` reports.
#[derive(Clone)]
pub(super) enum Reflected {
    /// As the method found says.
    Declared,
    /// Larastan's EloquentBuilderMethodReflection: static, declared in this class (its display name) or, without
    /// one, in the class the method found names, and with no throw type whatever the method declares.
    EloquentBuilder(Option<String>),
    /// Static, declared in the class the method found names.
    StaticDeclared,
    /// Not static.
    Instance,
}

/// PHPStan's `MethodsClassReflectionExtension` and `PropertiesClassReflectionExtension`.
pub(super) trait ClassReflectionExtension {
    /// The extension's answer for `name` on `receiver`, whose class is `class`, or `None` when it has none.
    fn method(
        &self,
        c: &Checker,
        phase: MethodPhase,
        receiver: &Type,
        class: &ClassLikeMetadata,
        name: &[u8],
    ) -> Option<MethodAnswer>;

    fn has_property(&self, c: &Checker, class: &ClassLikeMetadata, property: &[u8]) -> bool;

    /// Whether a call to `method` on `receiver` that the scope can't make natively still works, because the
    /// extension's method answers it: PHPStan's `ClassReflection::getMethod($name, $scope)` fallback.
    fn calls_inaccessible_method(&self, c: &Checker, receiver: &Type, method: &str) -> bool;

    /// The type a static call on `class` to an instance method declared in `declaring` forwards to, as Larastan
    /// returns a builder's method as static for a model.
    fn static_forward(
        &self,
        c: &Checker,
        class: &ClassLikeMetadata,
        declaring: &str,
    ) -> Option<Type>;
}

impl<'a> Checker<'a, '_> {
    pub(super) fn class_reflection_extensions(
        &self,
    ) -> impl Iterator<Item = &'a dyn ClassReflectionExtension> {
        (self.larastan.into_iter())
            .map(|l| l as &dyn ClassReflectionExtension)
            .chain((self.pest.into_iter()).map(|p| p as &dyn ClassReflectionExtension))
            .chain([&SoapClientMethods as &dyn ClassReflectionExtension])
    }
}

/// pest-plugin-phpstan's `universalObjectCratesClasses: [PHPUnit\Framework\TestCase]`: a test case has every
/// property, as the tests assign theirs in `beforeEach()`.
impl ClassReflectionExtension for crate::Pest {
    fn method(
        &self,
        _: &Checker,
        _: MethodPhase,
        _: &Type,
        _: &ClassLikeMetadata,
        _: &[u8],
    ) -> Option<MethodAnswer> {
        None
    }

    fn has_property(&self, c: &Checker, class: &ClassLikeMetadata, _: &[u8]) -> bool {
        let test_case = b"PHPUnit\\Framework\\TestCase";
        class.name.as_bytes().eq_ignore_ascii_case(test_case)
            || c.codebase.is_instance_of(class.name.as_bytes(), test_case)
    }

    fn calls_inaccessible_method(&self, _: &Checker, _: &Type, _: &str) -> bool {
        false
    }

    fn static_forward(&self, _: &Checker, _: &ClassLikeMetadata, _: &str) -> Option<Type> {
        None
    }
}

/// PHPStan's `SoapClientMethodsClassReflectionExtension`, which it registers after every other methods extension:
/// any method exists on `SoapClient` and its subclasses, because ext-soap calls the service's operations through
/// `__call()`.
struct SoapClientMethods;

impl ClassReflectionExtension for SoapClientMethods {
    fn method(
        &self,
        c: &Checker,
        phase: MethodPhase,
        _receiver: &Type,
        class: &ClassLikeMetadata,
        _name: &[u8],
    ) -> Option<MethodAnswer> {
        (phase == MethodPhase::Last
            && (c.codebase).is_instance_of(class.name.as_bytes(), b"soapclient"))
        .then_some(MethodAnswer::Exists(None))
    }

    fn has_property(&self, _c: &Checker, _class: &ClassLikeMetadata, _property: &[u8]) -> bool {
        false
    }

    fn calls_inaccessible_method(&self, _c: &Checker, _receiver: &Type, _method: &str) -> bool {
        false
    }

    fn static_forward(
        &self,
        _c: &Checker,
        _class: &ClassLikeMetadata,
        _declaring: &str,
    ) -> Option<Type> {
        None
    }
}
