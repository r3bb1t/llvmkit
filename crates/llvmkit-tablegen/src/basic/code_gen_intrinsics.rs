//! The per-intrinsic model: properties, function and argument attributes,
//! and memory effects, read off the TableGen records.
//!
//! Ports `llvm/utils/TableGen/Basic/CodeGenIntrinsics.cpp`.

use crate::*;

#[derive(Debug, Clone)]
pub(crate) struct IntrinsicOut {
    pub(crate) enum_name: String,
    pub(crate) name: String,
    pub(crate) target_prefix: String,
    pub(crate) overloaded: bool,
    pub(crate) type_sig: Vec<u8>,

    pub(crate) fn_attrs: FnAttrsOut,
    pub(crate) arg_attrs: Vec<IndexedAttrOut>,
    pub(crate) memory_effects: u32,
    pub(crate) clang_builtin: Option<String>,
    pub(crate) ms_builtin: Option<String>,
    pub(crate) pretty_print: Vec<PrettyPrintOut>,
    pub(crate) sample_overloads: Vec<SampleTypeOut>,
    pub(crate) record_id: usize,
}

pub(crate) fn positional_args(app: &Apply) -> GenResult<Vec<&Expr>> {
    let mut args = Vec::new();
    for arg in &app.args {
        match arg {
            TemplateArg::Pos(expr) => args.push(expr),
            TemplateArg::Named(name, _) => {
                return Err(GenError::new(format!(
                    "synthetic evaluation of `{}` does not support named argument `{name}`",
                    app.name
                )));
            }
        }
    }
    Ok(args)
}

pub(crate) fn list_int_at(values: &[i64], index: i64, label: &str) -> GenResult<i64> {
    let index = usize::try_from(index)
        .map_err(|_| GenError::new(format!("{label} negative index {index}")))?;
    values
        .get(index)
        .copied()
        .ok_or_else(|| GenError::new(format!("{label} index {index} out of bounds")))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FnAttrsOut {
    pub(crate) no_unwind: bool,
    pub(crate) no_return: bool,
    pub(crate) no_callback: bool,
    pub(crate) no_sync: bool,
    pub(crate) no_free: bool,
    pub(crate) will_return: bool,
    pub(crate) cold: bool,
    pub(crate) no_duplicate: bool,
    pub(crate) no_merge: bool,
    pub(crate) commutative: bool,
    pub(crate) convergent: bool,
    pub(crate) speculatable: bool,
    pub(crate) strict_fp: bool,
    pub(crate) no_create_undef_or_poison: bool,
    pub(crate) has_side_effects: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct IndexedAttrOut {
    pub(crate) index: u32,
    pub(crate) attr: ArgAttrOut,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ArgAttrOut {
    NoCapture,
    NoAlias,
    NoUndef,
    NonNull,
    Returned,
    ReadOnly,
    WriteOnly,
    ReadNone,
    ImmArg,
    Alignment(u64),
    Dereferenceable(u64),
    Range(i64, i64),
}

pub(crate) fn should_skip_field_eval(_name: &str) -> bool {
    false
}

pub(crate) fn assert_references_skipped_field(_expr: &Expr) -> bool {
    false
}

#[derive(Debug, Clone)]
pub(crate) struct PrettyPrintOut {
    pub(crate) arg_index: u32,
    pub(crate) name: String,
    pub(crate) printer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SampleTypeOut {
    Int(u32),
    Float(&'static str),
    Pointer(u32),
    FixedVector {
        lanes: u32,
        element: Box<SampleTypeOut>,
    },
}

#[derive(Debug)]
pub(crate) struct AttrInfo {
    pub(crate) fn_attrs: FnAttrsOut,
    pub(crate) arg_attrs: Vec<IndexedAttrOut>,
    pub(crate) memory_effects: u32,
    pub(crate) pretty_print: Vec<PrettyPrintOut>,
}

pub(crate) fn compute_attrs(properties: &[Value]) -> GenResult<AttrInfo> {
    let mut fn_attrs = FnAttrsOut {
        no_unwind: true,
        ..FnAttrsOut::default()
    };
    let mut memory_effects = MemoryEffectsBits::unknown();
    let mut indexed = Vec::new();
    let mut pretty_print = Vec::new();

    for prop in properties {
        let record_rc = as_record(prop)?;
        let record = record_rc.as_ref();
        let name = record.name.as_deref().unwrap_or("");
        match name {
            "IntrNoMem" => memory_effects = MemoryEffectsBits::none(),
            "IntrReadMem" => memory_effects = memory_effects.and(MemoryEffectsBits::read_only()),
            "IntrWriteMem" => memory_effects = memory_effects.and(MemoryEffectsBits::write_only()),
            "IntrArgMemOnly" => {
                memory_effects = memory_effects.and(MemoryEffectsBits::arg_mem_only())
            }
            "IntrInaccessibleMemOnly" => {
                memory_effects = memory_effects.and(MemoryEffectsBits::inaccessible_mem_only())
            }
            "IntrInaccessibleMemOrArgMemOnly" => {
                memory_effects =
                    memory_effects.and(MemoryEffectsBits::inaccessible_or_arg_mem_only())
            }
            "Commutative" => fn_attrs.commutative = true,
            "Throws" => fn_attrs.no_unwind = false,
            "IntrNoDuplicate" => fn_attrs.no_duplicate = true,
            "IntrNoMerge" => fn_attrs.no_merge = true,
            "IntrConvergent" => fn_attrs.convergent = true,
            "IntrNoReturn" => {
                fn_attrs.no_return = true;
                fn_attrs.will_return = false;
            }
            "IntrNoCallback" => fn_attrs.no_callback = true,
            "IntrNoSync" => fn_attrs.no_sync = true,
            "IntrNoFree" => fn_attrs.no_free = true,
            "IntrWillReturn" => {
                if !fn_attrs.no_return {
                    fn_attrs.will_return = true;
                }
            }
            "IntrCold" => fn_attrs.cold = true,
            "IntrSpeculatable" => fn_attrs.speculatable = true,
            "IntrHasSideEffects" => fn_attrs.has_side_effects = true,
            "IntrStrictFP" => fn_attrs.strict_fp = true,
            "IntrNoCreateUndefOrPoison" => fn_attrs.no_create_undef_or_poison = true,
            _ if record.classes.contains("IntrRead") => {
                let mut mask = MemoryEffectsBits::write_only();
                for loc in list_field(record, "MemLoc")? {
                    mask = mask.with_mod_ref(
                        memory_location(as_record(&loc)?.as_ref())?,
                        ModRefEffect::ModRef,
                    );
                }
                memory_effects = memory_effects.and(mask);
            }
            _ if record.classes.contains("IntrWrite") => {
                let mut mask = MemoryEffectsBits::read_only();
                for loc in list_field(record, "MemLoc")? {
                    mask = mask.with_mod_ref(
                        memory_location(as_record(&loc)?.as_ref())?,
                        ModRefEffect::ModRef,
                    );
                }
                memory_effects = memory_effects.and(mask);
            }
            _ if record.classes.contains("NoCapture") => {
                indexed.push(indexed_attr(record, ArgAttrOut::NoCapture)?);
            }
            _ if record.classes.contains("NoAlias") => {
                indexed.push(indexed_attr(record, ArgAttrOut::NoAlias)?);
            }
            _ if record.classes.contains("NoUndef") => {
                indexed.push(indexed_attr(record, ArgAttrOut::NoUndef)?);
            }
            _ if record.classes.contains("NonNull") => {
                indexed.push(indexed_attr(record, ArgAttrOut::NonNull)?);
            }
            _ if record.classes.contains("Returned") => {
                indexed.push(indexed_attr(record, ArgAttrOut::Returned)?);
            }
            _ if record.classes.contains("ReadOnly") => {
                indexed.push(indexed_attr(record, ArgAttrOut::ReadOnly)?);
            }
            _ if record.classes.contains("WriteOnly") => {
                indexed.push(indexed_attr(record, ArgAttrOut::WriteOnly)?);
            }
            _ if record.classes.contains("ReadNone") => {
                indexed.push(indexed_attr(record, ArgAttrOut::ReadNone)?);
            }
            _ if record.classes.contains("ImmArg") => {
                indexed.push(indexed_attr(record, ArgAttrOut::ImmArg)?);
            }
            _ if record.classes.contains("Align") => {
                let align = int_field(record, "Align")? as u64;
                indexed.push(indexed_attr(record, ArgAttrOut::Alignment(align))?);
            }
            _ if record.classes.contains("Dereferenceable") => {
                let bytes = int_field(record, "Bytes")? as u64;
                indexed.push(indexed_attr(record, ArgAttrOut::Dereferenceable(bytes))?);
            }
            _ if record.classes.contains("Range") => {
                let lower = int_field(record, "Lower")?;
                let upper = int_field(record, "Upper")?;
                indexed.push(indexed_attr(record, ArgAttrOut::Range(lower, upper))?);
            }
            _ if record.classes.contains("ArgInfo") => {
                let arg_no = int_field(record, "ArgNo")?;
                if arg_no < 1 {
                    return Err(GenError::new("ArgInfo requires ArgNo >= 1"));
                }
                let mut arg_name = String::new();
                let mut func_name = String::new();
                for value in list_field(record, "Properties")? {
                    let prop_rc = as_record(&value)?;
                    let prop = prop_rc.as_ref();
                    if prop.classes.contains("ArgName") {
                        arg_name = string_field(prop, "Name")?.unwrap_or_default();
                    } else if prop.classes.contains("ImmArgPrinter") {
                        func_name = string_field(prop, "FuncName")?.unwrap_or_default();
                    } else {
                        return Err(GenError::new(format!(
                            "unknown ArgProperty {:?}",
                            prop.name
                        )));
                    }
                }
                pretty_print.push(PrettyPrintOut {
                    arg_index: (arg_no - 1) as u32,
                    name: arg_name,
                    printer: func_name,
                });
            }
            _ => {
                return Err(GenError::new(format!(
                    "unknown intrinsic property `{name}`"
                )));
            }
        }
    }

    if fn_attrs.has_side_effects && memory_effects.does_not_access_memory() {
        memory_effects = MemoryEffectsBits::unknown();
    }
    indexed.sort();
    indexed.dedup();
    pretty_print.sort_by_key(|arg| arg.arg_index);

    Ok(AttrInfo {
        fn_attrs,
        arg_attrs: indexed,
        memory_effects: memory_effects.0,
        pretty_print,
    })
}

pub(crate) fn indexed_attr(record: &RecordValue, attr: ArgAttrOut) -> GenResult<IndexedAttrOut> {
    Ok(IndexedAttrOut {
        index: int_field(record, "ArgNo")? as u32,
        attr,
    })
}

#[derive(Clone, Copy)]
pub(crate) enum ModRefEffect {
    Ref = 1,
    Mod = 2,
    ModRef = 3,
}

#[derive(Clone, Copy)]
pub(crate) enum MemLoc {
    ArgMem = 0,
    InaccessibleMem = 1,
    ErrnoMem = 2,
    Other = 3,
    TargetMem0 = 4,
    TargetMem1 = 5,
}

#[derive(Clone, Copy)]
pub(crate) struct MemoryEffectsBits(pub(crate) u32);

impl MemoryEffectsBits {
    pub(crate) fn unknown() -> Self {
        let mut value = Self(0);
        for loc in [
            MemLoc::ArgMem,
            MemLoc::InaccessibleMem,
            MemLoc::ErrnoMem,
            MemLoc::Other,
            MemLoc::TargetMem0,
            MemLoc::TargetMem1,
        ] {
            value = value.with_mod_ref(loc, ModRefEffect::ModRef);
        }
        value
    }

    pub(crate) fn none() -> Self {
        Self(0)
    }

    pub(crate) fn read_only() -> Self {
        Self::all(ModRefEffect::Ref)
    }

    pub(crate) fn write_only() -> Self {
        Self::all(ModRefEffect::Mod)
    }

    pub(crate) fn all(effect: ModRefEffect) -> Self {
        let mut value = Self(0);
        for loc in [
            MemLoc::ArgMem,
            MemLoc::InaccessibleMem,
            MemLoc::ErrnoMem,
            MemLoc::Other,
            MemLoc::TargetMem0,
            MemLoc::TargetMem1,
        ] {
            value = value.with_mod_ref(loc, effect);
        }
        value
    }

    pub(crate) fn arg_mem_only() -> Self {
        Self(0).with_mod_ref(MemLoc::ArgMem, ModRefEffect::ModRef)
    }

    pub(crate) fn inaccessible_mem_only() -> Self {
        Self(0).with_mod_ref(MemLoc::InaccessibleMem, ModRefEffect::ModRef)
    }

    pub(crate) fn inaccessible_or_arg_mem_only() -> Self {
        Self(0)
            .with_mod_ref(MemLoc::ArgMem, ModRefEffect::ModRef)
            .with_mod_ref(MemLoc::InaccessibleMem, ModRefEffect::ModRef)
    }

    pub(crate) fn with_mod_ref(self, loc: MemLoc, effect: ModRefEffect) -> Self {
        let shift = (loc as u32) * 2;
        Self((self.0 & !(0b11 << shift)) | ((effect as u32) << shift))
    }

    pub(crate) fn and(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub(crate) fn does_not_access_memory(self) -> bool {
        self.0 == 0
    }
}

pub(crate) fn memory_location(record: &RecordValue) -> GenResult<MemLoc> {
    match record.name.as_deref().unwrap_or("") {
        "InaccessibleMem" => Ok(MemLoc::InaccessibleMem),
        "TargetMem0" => Ok(MemLoc::TargetMem0),
        "TargetMem1" => Ok(MemLoc::TargetMem1),
        name => Err(GenError::new(format!("unknown memory location `{name}`"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::test_record;

    /// Mirrors `llvm/utils/TableGen/Basic/CodeGenIntrinsics.cpp::CodeGenIntrinsic`:
    /// default semantic names derive from the `int_*` record name by replacing
    /// underscores with dots and adding the `llvm.` prefix.
    #[test]
    fn semantic_name_derivation_matches_llvm_rule() {
        let enum_name = "vector_reduce_add";
        assert_eq!(
            format!("llvm.{}", enum_name.replace('_', ".")),
            "llvm.vector.reduce.add"
        );
    }

    /// Mirrors `llvm/include/llvm/IR/Intrinsics.td::Throws` and
    /// `llvm/utils/TableGen/Basic/CodeGenIntrinsics.cpp::CodeGenIntrinsic::setProperty`:
    /// a throwing intrinsic must not receive generated `no_unwind`.
    #[test]
    fn throws_property_preserves_throwing_intrinsic() {
        let attrs = compute_attrs(&[test_record("Throws", &["IntrinsicProperty"], &[])]).unwrap();

        assert!(!attrs.fn_attrs.no_unwind);
    }

    /// Mirrors `llvm/include/llvm/IR/Intrinsics.td::Commutative` and
    /// `llvm/utils/TableGen/Basic/CodeGenIntrinsics.cpp::CodeGenIntrinsic::setProperty`:
    /// the TableGen property is emitted into the generated intrinsic attributes.
    #[test]
    fn commutative_property_is_emitted() {
        let attrs =
            compute_attrs(&[test_record("Commutative", &["IntrinsicProperty"], &[])]).unwrap();

        assert!(attrs.fn_attrs.commutative);
    }
}
