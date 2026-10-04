//! llvmkit typestate compile-fail (D1, D8): a `ReadOnly` value handle cannot
//! be renamed through the `SetName` trait.
//!
//! The walk below is the one an `Inspect` pass receives, a `ModuleView`'s
//! blocks, over a `Module<B, Verified>`, so every instruction it reaches is
//! `ReadOnly`, and so is the value each one erases to and every handle that
//! value narrows to through the handle's own `TryFrom`. `SetName` is
//! implemented for these value handles only where the capability
//! `CanMutate`, so each trait-qualified call is refused by `CanMutate`'s own
//! message. The token is a second unverified module of the same brand, which
//! type-checks, so nothing but the handle's capability refuses the calls.
//!
//! The handles are `Value`, `Argument`, `Constant`, `StructValue`,
//! `ArrayValue`, `VectorValue`, `IntValue`, `FloatValue`, `ConstantIntValue`
//! and `ConstantFloatValue`, each with a `SetName` impl of its own, and
//! `PointerValue` and `UndefValue`, which stand for the impl the
//! `decl_value_handle!` and `decl_constant_handle!` macros write for every
//! handle they declare.
//!
//! Each impl's bodies call a `set_name` / `clear_name` that carries the same
//! bound, so widening an impl alone breaks the crate's build. This fixture is
//! what fails if an impl is widened and its bodies are routed around those
//! methods, which keep their bound.
//!
//! The program is never run: the module is empty, and only the calls have to
//! type-check for the law to be tested.
//!
//! No upstream counterpart: `Value::setName` is a plain non-const method, and
//! LLVM has no verified-module typestate.
use llvmkit_ir::{
    Argument, ArrLenDyn, ArrayValue, Constant, ConstantFloatValue, ConstantIntValue, DynBrand,
    ElemDyn, FloatDyn, FloatValue, IntDyn, IntValue, LenDyn, Module, PointerValue, ReadOnly,
    SetName, StructValue, UndefValue, VectorValue,
};

fn main() {
    // A second unverified module of the same brand: its token type-checks.
    let other = Module::dynamic("other");
    let m = Module::dynamic("m").verify().expect("verifies");
    for function in m.as_view().functions() {
        for block in function.basic_blocks() {
            for instruction in block.instructions() {
                let value = instruction.to_erased();
                SetName::set_name(value, &other, "renamed");
                SetName::clear_name(value, &other);
                if let Ok(argument) = Argument::try_from(value) {
                    SetName::set_name(argument, &other, "renamed");
                    SetName::clear_name(argument, &other);
                }
                if let Ok(structure) = StructValue::try_from(value) {
                    SetName::set_name(structure, &other, "renamed");
                    SetName::clear_name(structure, &other);
                }
                if let Ok(array) =
                    ArrayValue::<ElemDyn, ArrLenDyn, DynBrand, ReadOnly>::try_from(value)
                {
                    SetName::set_name(array, &other, "renamed");
                    SetName::clear_name(array, &other);
                }
                if let Ok(vector) =
                    VectorValue::<ElemDyn, LenDyn, DynBrand, ReadOnly>::try_from(value)
                {
                    SetName::set_name(vector, &other, "renamed");
                    SetName::clear_name(vector, &other);
                }
                if let Ok(int) = IntValue::<IntDyn, DynBrand, ReadOnly>::try_from(value) {
                    SetName::set_name(int, &other, "renamed");
                    SetName::clear_name(int, &other);
                }
                if let Ok(float) = FloatValue::<FloatDyn, DynBrand, ReadOnly>::try_from(value) {
                    SetName::set_name(float, &other, "renamed");
                    SetName::clear_name(float, &other);
                }
                if let Ok(pointer) = PointerValue::try_from(value) {
                    SetName::set_name(pointer, &other, "renamed");
                    SetName::clear_name(pointer, &other);
                }
                if let Ok(constant) = Constant::try_from(value) {
                    SetName::set_name(constant, &other, "renamed");
                    SetName::clear_name(constant, &other);
                    if let Ok(undef) = UndefValue::try_from(constant) {
                        SetName::set_name(undef, &other, "renamed");
                        SetName::clear_name(undef, &other);
                    }
                    if let Ok(int) =
                        ConstantIntValue::<IntDyn, DynBrand, ReadOnly>::try_from(constant)
                    {
                        SetName::set_name(int, &other, "renamed");
                        SetName::clear_name(int, &other);
                    }
                    if let Ok(float) =
                        ConstantFloatValue::<FloatDyn, DynBrand, ReadOnly>::try_from(constant)
                    {
                        SetName::set_name(float, &other, "renamed");
                        SetName::clear_name(float, &other);
                    }
                }
            }
        }
    }
}
