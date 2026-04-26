//! Sealed marker types for IEEE/non-IEEE floating-point kinds.
//!
//! Mirrors the closed set of float kinds in `Type.h` (`HalfTyID`,
//! `BFloatTyID`, `FloatTyID`, `DoubleTyID`, `X86_FP80TyID`, `FP128TyID`,
//! `PPC_FP128TyID`). Encoding the kind in the type system lets
//! `FloatValue<'ctx, KFloat>` be a different type from
//! `FloatValue<'ctx, KDouble>` -- a `build_fp_add` between them is a
//! compile error rather than a runtime check.
//!
//! [`KDyn`] is the runtime-checked escape hatch for parsed IR and APIs
//! that have not yet been narrowed.

use core::fmt;

use crate::r#type::sealed;

/// Sealed marker trait implemented by every IEEE-like float kind tag.
pub trait FloatKind: sealed::Sealed + Copy + 'static + fmt::Debug {
    /// LangRef keyword for this kind. `None` for [`KDyn`].
    fn ieee_label() -> Option<&'static str>;
}

macro_rules! decl_static_kind {
    ($(#[$attr:meta])* $name:ident, $label:expr) => {
        $(#[$attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $name;
        impl sealed::Sealed for $name {}
        impl FloatKind for $name {
            #[inline]
            fn ieee_label() -> Option<&'static str> { Some($label) }
        }
    };
}

decl_static_kind!(
    /// IEEE 754 binary16. Mirrors `Type::HalfTyID`.
    KHalf, "half"
);
decl_static_kind!(
    /// Brain-float (1 sign / 8 exp / 7 frac). Mirrors `Type::BFloatTyID`.
    KBFloat, "bfloat"
);
decl_static_kind!(
    /// IEEE 754 binary32. Mirrors `Type::FloatTyID`.
    KFloat, "float"
);
decl_static_kind!(
    /// IEEE 754 binary64. Mirrors `Type::DoubleTyID`.
    KDouble, "double"
);
decl_static_kind!(
    /// IEEE 754 binary128. Mirrors `Type::FP128TyID`.
    KFp128, "fp128"
);
decl_static_kind!(
    /// X87 80-bit extended precision. Mirrors `Type::X86_FP80TyID`.
    KX86Fp80, "x86_fp80"
);
decl_static_kind!(
    /// PowerPC double-double. Mirrors `Type::PPC_FP128TyID`.
    KPpcFp128, "ppc_fp128"
);

/// Kind-erased marker. The handle still tracks its kind via the
/// underlying type-arena entry -- this marker only signals "the
/// type system does not know which kind." Used by parsed IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KDyn;
impl sealed::Sealed for KDyn {}
impl FloatKind for KDyn {
    #[inline]
    fn ieee_label() -> Option<&'static str> {
        None
    }
}
