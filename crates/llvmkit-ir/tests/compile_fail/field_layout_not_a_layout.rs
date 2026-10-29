//! llvmkit-specific compile-fail (D2, D10): a field kind names its LLVM shape
//! through `IrField::Layout`, and `FieldLayout` is sealed to the valid struct
//! elements llvmkit builds itself. A kind whose layout is a marker of the
//! user's own — here one standing for `label`, which no struct may contain —
//! does not compile, so no field can be laid out as an invalid element. No
//! upstream counterpart: `StructType::get` takes any `Type *` (llvmkit's
//! stricter erased constructors are `docs/divergences.md` D19).

use llvmkit_ir::{Capability, IrField, LayoutValue, ModuleBrand};

struct LabelMarker;

struct Bad;

impl IrField for Bad {
    type Layout = LabelMarker;
    type Value<'ctx, B: ModuleBrand + 'ctx, C: Capability> = LayoutValue<'ctx, LabelMarker, B, C>;

    fn wrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        raw: LayoutValue<'ctx, Self::Layout, B, C>,
    ) -> Self::Value<'ctx, B, C> {
        raw
    }

    fn unwrap<'ctx, B: ModuleBrand + 'ctx, C: Capability>(
        value: Self::Value<'ctx, B, C>,
    ) -> LayoutValue<'ctx, Self::Layout, B, C> {
        value
    }
}

fn main() {}
