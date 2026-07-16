//! llvmkit typestate compile-fail (Doctrine D4/D7).
//! Closest upstream: `FunctionTest.hasLazyArguments` for ordered arguments;
//! llvmkit keeps the validated-params capability scoped to one facade call.

use llvmkit_ir::function_signature::token::ValidatedFunctionParams;
use llvmkit_ir::{
    Argument, FunctionParam, IrResult, Module, ModuleBrand, Type, TypeKindLabel, Unverified, Value,
};

struct Leaker;

impl FunctionParam for Leaker {
    type Value<'ctx, B: ModuleBrand + 'ctx> = ();

    fn ir_type<'ctx, B>(module: &Module<'ctx, B, Unverified>) -> IrResult<Type<'ctx, B>>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(module.i32_type().as_type())
    }

    fn matches_ir_type<'ctx, B>(_ty: Type<'ctx, B>) -> bool
    where
        B: ModuleBrand + 'ctx,
    {
        true
    }

    fn expected_kind_label() -> TypeKindLabel {
        TypeKindLabel::Integer
    }

    fn validate_argument<'ctx, B>(_arg: Argument<'ctx, B>) -> IrResult<()>
    where
        B: ModuleBrand + 'ctx,
    {
        Ok(())
    }

    fn value_from_argument<'ctx, B>(
        _arg: Argument<'ctx, B>,
        validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        let _leaked: &'static ValidatedFunctionParams<'static> = validated;
    }

    fn value_from_value<'ctx, B>(
        _value: Value<'ctx, B>,
        validated: &ValidatedFunctionParams<'_>,
    ) -> Self::Value<'ctx, B>
    where
        B: ModuleBrand + 'ctx,
    {
        let _leaked: &'static ValidatedFunctionParams<'static> = validated;
    }
}

fn main() {}
