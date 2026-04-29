//! Target data layout. Mirrors `llvm/include/llvm/IR/DataLayout.h`
//! and `llvm/lib/IR/DataLayout.cpp`.
//!
//! A [`DataLayout`] is a parsed representation of the
//! `target datalayout = "..."` directive: endianness, mangling mode,
//! per-bit-width primitive alignments (`i` / `f` / `v`), per-address-
//! space pointer specs (`p[<as>]:<size>:<abi>:<pref>:<idx>`), legal
//! native integer widths (`n`), aggregate alignments (`a`), stack
//! natural alignment (`S`), function pointer alignment (`F`), program
//! / alloca / globals address spaces (`P` / `A` / `G`), and the set of
//! non-integral pointer address spaces (`ni`).
//!
//! Every accessor cites the upstream method by name; each
//! private parser function mirrors `parsePrimitiveSpec` /
//! `parseAggregateSpec` / `parsePointerSpec` /
//! `parseSpecification` / `parseLayoutString` line for line.
//!
//! ## What's not ported (yet)
//!
//! - `getStructLayout` caching (we recompute every call; matches the
//!   upstream lazy-init shape but without the cache).
//! - `getGEPIndicesForOffset` / `getIndexedOffsetInType` (depends on
//!   the planned ConstantExpr layer).
//! - `hasMicrosoftFastStdCallMangling` and friends (presentation-only
//!   helpers; reachable through [`DataLayout::mangling_mode`]).

use core::fmt;

use crate::align::Align;
use crate::error::{IrError, IrResult};
use crate::module::Module;
use crate::r#type::{Type, TypeData, TypeId};

// --------------------------------------------------------------------------
// Sub-records
// --------------------------------------------------------------------------

/// Per-bit-width primitive alignment (`i<N>:<abi>:<pref>` / `f<...>` /
/// `v<...>`). Mirrors `DataLayout::PrimitiveSpec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrimitiveSpec {
    pub bit_width: u32,
    pub abi_align: Align,
    pub pref_align: Align,
}

/// Per-address-space pointer specification
/// (`p[<flags+as>]:<size>:<abi>:<pref>[:<idx>]`). Mirrors
/// `DataLayout::PointerSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PointerSpec {
    pub address_space: u32,
    pub bit_width: u32,
    pub abi_align: Align,
    pub pref_align: Align,
    pub index_bit_width: u32,
    /// Pointer values in this address space have an unstable
    /// representation: bit-identical pointers may compare unequal,
    /// or `inttoptr(ptrtoint(p))` may not equal `p`. Mirrors the `u`
    /// flag in `parsePointerSpec`.
    pub has_unstable_repr: bool,
    /// Pointers in this address space carry external state stored
    /// outside the in-memory bit pattern (e.g. CHERI capability
    /// tags). Mirrors the `e` flag in `parsePointerSpec`.
    pub has_external_state: bool,
    /// Symbolic name (`p1(global):...` -> `"global"`). Empty string
    /// if no name is set.
    pub address_space_name: String,
    /// Whether this address space is non-integral (set by `ni:<as>`
    /// specs after primary parsing). Mirrors the post-pass in
    /// `parseLayoutString`.
    pub non_integral: bool,
}

/// Function-pointer alignment kind. Mirrors
/// `DataLayout::FunctionPtrAlignType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum FunctionPtrAlignType {
    /// `Fi<abi>` -- pointer alignment is independent of function
    /// alignment.
    #[default]
    Independent,
    /// `Fn<abi>` -- pointer alignment is a multiple of the function
    /// alignment.
    MultipleOfFunctionAlign,
}

/// Symbol-name mangling mode. Mirrors
/// `DataLayout::ManglingModeT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ManglingMode {
    /// No mangling (default).
    #[default]
    None,
    /// `m:e` -- ELF mangling.
    Elf,
    /// `m:o` -- Mach-O mangling.
    MachO,
    /// `m:w` -- WinCOFF mangling.
    WinCoff,
    /// `m:x` -- WinCOFF x86 mangling (with `_` prefix).
    WinCoffX86,
    /// `m:l` -- GOFF mangling.
    Goff,
    /// `m:m` -- MIPS mangling.
    Mips,
    /// `m:a` -- XCOFF mangling.
    XCoff,
}

impl ManglingMode {
    /// `.ll` keyword-suffix for `m:<keyword>`, or `None` for
    /// [`Self::None`] (no `m:` token in the layout string).
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Elf => Some("e"),
            Self::MachO => Some("o"),
            Self::WinCoff => Some("w"),
            Self::WinCoffX86 => Some("x"),
            Self::Goff => Some("l"),
            Self::Mips => Some("m"),
            Self::XCoff => Some("a"),
        }
    }
}

// --------------------------------------------------------------------------
// DataLayout
// --------------------------------------------------------------------------

/// Parsed `target datalayout = "..."` directive. Mirrors
/// `class DataLayout` in `IR/DataLayout.h`.
///
/// Construction goes through [`Self::parse`] (the canonical entry,
/// equivalent to `static Expected<DataLayout> DataLayout::parse`) or
/// [`Self::default`] which seeds the same per-platform-agnostic
/// defaults as `DataLayout::DataLayout()`.
///
/// The layout is cheap to clone and round-trips through
/// [`Display`](fmt::Display): every parsed [`DataLayout`] re-emits a
/// canonical (deterministically-ordered) version of its specs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataLayout {
    big_endian: bool,
    alloca_addr_space: u32,
    program_addr_space: u32,
    default_globals_addr_space: u32,
    stack_natural_align: Option<Align>,
    function_ptr_align: Option<Align>,
    function_ptr_align_type: FunctionPtrAlignType,
    mangling_mode: ManglingMode,
    /// Native integer widths in declaration order (mirrors
    /// `LegalIntWidths`). Stored as `u32` rather than `unsigned char`
    /// to avoid the upstream truncation FIXME.
    legal_int_widths: Vec<u32>,
    /// Sorted by `bit_width`. Mirrors `IntSpecs` (kept sorted by
    /// `setPrimitiveSpec`'s `lower_bound` insert).
    int_specs: Vec<PrimitiveSpec>,
    /// Sorted by `bit_width`. Mirrors `FloatSpecs`.
    float_specs: Vec<PrimitiveSpec>,
    /// Sorted by `bit_width`. Mirrors `VectorSpecs`.
    vector_specs: Vec<PrimitiveSpec>,
    /// Sorted by `address_space`. Mirrors `PointerSpecs`.
    pointer_specs: Vec<PointerSpec>,
    struct_abi_align: Align,
    struct_pref_align: Align,
    /// The unparsed layout string, in declaration order. Returned by
    /// [`Self::string_representation`].
    string_representation: String,
}

impl Default for DataLayout {
    /// Mirrors the default `DataLayout::DataLayout()` ctor: seeds the
    /// platform-agnostic primitive / pointer specs, then leaves the
    /// rest of the fields zero-default.
    fn default() -> Self {
        let mut layout = Self {
            big_endian: false,
            alloca_addr_space: 0,
            program_addr_space: 0,
            default_globals_addr_space: 0,
            stack_natural_align: None,
            function_ptr_align: None,
            function_ptr_align_type: FunctionPtrAlignType::Independent,
            mangling_mode: ManglingMode::None,
            legal_int_widths: Vec::new(),
            int_specs: vec![
                PrimitiveSpec {
                    bit_width: 8,
                    abi_align: Align::ONE,
                    pref_align: Align::ONE,
                },
                PrimitiveSpec {
                    bit_width: 16,
                    abi_align: const_align(2),
                    pref_align: const_align(2),
                },
                PrimitiveSpec {
                    bit_width: 32,
                    abi_align: const_align(4),
                    pref_align: const_align(4),
                },
                PrimitiveSpec {
                    bit_width: 64,
                    abi_align: const_align(4),
                    pref_align: const_align(8),
                },
            ],
            float_specs: vec![
                PrimitiveSpec {
                    bit_width: 16,
                    abi_align: const_align(2),
                    pref_align: const_align(2),
                },
                PrimitiveSpec {
                    bit_width: 32,
                    abi_align: const_align(4),
                    pref_align: const_align(4),
                },
                PrimitiveSpec {
                    bit_width: 64,
                    abi_align: const_align(8),
                    pref_align: const_align(8),
                },
                PrimitiveSpec {
                    bit_width: 128,
                    abi_align: const_align(16),
                    pref_align: const_align(16),
                },
            ],
            vector_specs: vec![
                PrimitiveSpec {
                    bit_width: 64,
                    abi_align: const_align(8),
                    pref_align: const_align(8),
                },
                PrimitiveSpec {
                    bit_width: 128,
                    abi_align: const_align(16),
                    pref_align: const_align(16),
                },
            ],
            pointer_specs: Vec::new(),
            struct_abi_align: Align::ONE,
            struct_pref_align: const_align(8),
            string_representation: String::new(),
        };
        // Default pointer spec for AS 0: 64-bit pointers, 8-byte
        // ABI/pref alignment, 64-bit index width. Mirrors the trailing
        // `setPointerSpec(0, ...)` call in `DataLayout::DataLayout()`.
        layout.set_pointer_spec(PointerSpec {
            address_space: 0,
            bit_width: 64,
            abi_align: const_align(8),
            pref_align: const_align(8),
            index_bit_width: 64,
            has_unstable_repr: false,
            has_external_state: false,
            address_space_name: String::new(),
            non_integral: false,
        });
        layout
    }
}

impl DataLayout {
    /// Parse a layout string. Mirrors
    /// `static Expected<DataLayout> DataLayout::parse(StringRef)`.
    /// Returns [`IrError::InvalidDataLayout`] on the first parse
    /// failure.
    pub fn parse(s: impl AsRef<str>) -> IrResult<Self> {
        let s = s.as_ref();
        let mut layout = Self::default();
        layout.parse_layout_string(s)?;
        Ok(layout)
    }

    /// `true` for layout strings that came from the empty default
    /// (no `target datalayout` directive). Mirrors
    /// `DataLayout::isDefault`.
    #[inline]
    pub fn is_default(&self) -> bool {
        self.string_representation.is_empty()
    }

    /// Stored layout string. Mirrors
    /// `DataLayout::getStringRepresentation`.
    #[inline]
    pub fn string_representation(&self) -> &str {
        &self.string_representation
    }

    /// Endianness. Mirrors `DataLayout::isLittleEndian`.
    #[inline]
    pub fn is_little_endian(&self) -> bool {
        !self.big_endian
    }

    /// Mirrors `DataLayout::isBigEndian`.
    #[inline]
    pub fn is_big_endian(&self) -> bool {
        self.big_endian
    }

    /// Symbol mangling mode. Mirrors the reading half of
    /// `ManglingMode`.
    #[inline]
    pub fn mangling_mode(&self) -> ManglingMode {
        self.mangling_mode
    }

    /// Mirrors `DataLayout::getStackAlignment`.
    #[inline]
    pub fn stack_alignment(&self) -> Option<Align> {
        self.stack_natural_align
    }

    /// Mirrors `DataLayout::getFunctionPtrAlign`.
    #[inline]
    pub fn function_ptr_align(&self) -> Option<Align> {
        self.function_ptr_align
    }

    /// Mirrors `DataLayout::getFunctionPtrAlignType`.
    #[inline]
    pub fn function_ptr_align_type(&self) -> FunctionPtrAlignType {
        self.function_ptr_align_type
    }

    /// Mirrors `DataLayout::getAllocaAddrSpace`.
    #[inline]
    pub fn alloca_addr_space(&self) -> u32 {
        self.alloca_addr_space
    }

    /// Mirrors `DataLayout::getProgramAddressSpace`.
    #[inline]
    pub fn program_addr_space(&self) -> u32 {
        self.program_addr_space
    }

    /// Mirrors `DataLayout::getDefaultGlobalsAddressSpace`.
    #[inline]
    pub fn default_globals_addr_space(&self) -> u32 {
        self.default_globals_addr_space
    }

    /// Mirrors `DataLayout::isLegalInteger(BitWidth)`. The set of
    /// "legal" integer widths is the `n<size>:<size>...` spec.
    #[inline]
    pub fn is_legal_integer(&self, bit_width: u32) -> bool {
        self.legal_int_widths.contains(&bit_width)
    }

    /// Mirrors `DataLayout::isIllegalInteger(BitWidth)`.
    #[inline]
    pub fn is_illegal_integer(&self, bit_width: u32) -> bool {
        !self.is_legal_integer(bit_width)
    }

    /// Mirrors `DataLayout::fitsInLegalInteger`.
    pub fn fits_in_legal_integer(&self, bit_width: u32) -> bool {
        self.legal_int_widths.iter().any(|&w| bit_width <= w)
    }

    /// Mirrors `DataLayout::getLargestLegalIntTypeSizeInBits`.
    pub fn largest_legal_int_type_size_in_bits(&self) -> u32 {
        self.legal_int_widths.iter().copied().max().unwrap_or(0)
    }

    /// Mirrors `DataLayout::getNonStandardAddressSpaces`. Returns the
    /// address spaces with an explicit pointer spec (every spec
    /// except `0`).
    pub fn non_standard_address_spaces(&self) -> Vec<u32> {
        self.pointer_specs
            .iter()
            .filter(|s| s.address_space != 0)
            .map(|s| s.address_space)
            .collect()
    }

    /// Mirrors `DataLayout::getStructPrefAlignment` (the protected
    /// member; we expose it for parity with [`Self::struct_abi_align`]).
    #[inline]
    pub fn struct_pref_align(&self) -> Align {
        self.struct_pref_align
    }

    /// Mirrors `DataLayout::getStructAlignment` minimum (the ABI
    /// alignment seeded by the `a:<abi>` spec).
    #[inline]
    pub fn struct_abi_align(&self) -> Align {
        self.struct_abi_align
    }

    /// Mirrors `DataLayout::getNonIntegralAddressSpaces` (returns
    /// every address space marked by `ni:<as>...`).
    pub fn non_integral_address_spaces(&self) -> Vec<u32> {
        self.pointer_specs
            .iter()
            .filter(|s| s.non_integral)
            .map(|s| s.address_space)
            .collect()
    }

    /// Mirrors `DataLayout::isNonIntegralAddressSpace`.
    pub fn is_non_integral_address_space(&self, addr_space: u32) -> bool {
        self.pointer_spec(addr_space).non_integral
    }

    /// Mirrors `DataLayout::hasUnstableRepresentation(unsigned)`.
    pub fn has_unstable_representation(&self, addr_space: u32) -> bool {
        self.pointer_spec(addr_space).has_unstable_repr
    }

    /// Mirrors `DataLayout::hasExternalState(unsigned)`.
    pub fn has_external_state(&self, addr_space: u32) -> bool {
        self.pointer_spec(addr_space).has_external_state
    }

    /// Mirrors `DataLayout::getAddressSpaceName`.
    pub fn address_space_name(&self, addr_space: u32) -> &str {
        &self.pointer_spec(addr_space).address_space_name
    }

    /// Mirrors `DataLayout::getNamedAddressSpace`.
    pub fn named_address_space(&self, name: &str) -> Option<u32> {
        self.pointer_specs
            .iter()
            .find(|s| s.address_space_name == name)
            .map(|s| s.address_space)
    }

    // ----- Pointer accessors -----

    /// Mirrors `DataLayout::getPointerSize(AS)`. Bytes.
    pub fn pointer_size(&self, addr_space: u32) -> u32 {
        div_ceil_u32(self.pointer_spec(addr_space).bit_width, 8)
    }

    /// Mirrors `DataLayout::getPointerSizeInBits(AS)`.
    #[inline]
    pub fn pointer_size_in_bits(&self, addr_space: u32) -> u32 {
        self.pointer_spec(addr_space).bit_width
    }

    /// Mirrors `DataLayout::getIndexSize(AS)`. Bytes.
    pub fn index_size(&self, addr_space: u32) -> u32 {
        div_ceil_u32(self.pointer_spec(addr_space).index_bit_width, 8)
    }

    /// Mirrors `DataLayout::getIndexSizeInBits(AS)`.
    #[inline]
    pub fn index_size_in_bits(&self, addr_space: u32) -> u32 {
        self.pointer_spec(addr_space).index_bit_width
    }

    /// Mirrors `DataLayout::getPointerABIAlignment(AS)`.
    #[inline]
    pub fn pointer_abi_align(&self, addr_space: u32) -> Align {
        self.pointer_spec(addr_space).abi_align
    }

    /// Mirrors `DataLayout::getPointerPrefAlignment(AS)`.
    #[inline]
    pub fn pointer_pref_align(&self, addr_space: u32) -> Align {
        self.pointer_spec(addr_space).pref_align
    }

    /// Mirrors `DataLayout::getPointerSpec`. Crate-internal because
    /// the spec struct is plain data.
    pub fn pointer_spec(&self, addr_space: u32) -> &PointerSpec {
        if addr_space != 0
            && let Some(spec) = self
                .pointer_specs
                .iter()
                .find(|s| s.address_space == addr_space)
        {
            return spec;
        }
        self.pointer_specs
            .iter()
            .find(|s| s.address_space == 0)
            .unwrap_or_else(|| unreachable!("DataLayout always has a pointer spec for AS 0"))
    }

    // ----- Type-size / alignment accessors -----

    /// Bit-size of `ty` when held as an SSA value. Mirrors
    /// `DataLayout::getTypeSizeInBits` (the inline definition in
    /// `DataLayout.h`).
    pub fn type_size_in_bits(&self, ty: Type<'_>) -> u64 {
        self.type_size_in_bits_inner(ty.module(), ty.id())
    }

    /// Mirrors `DataLayout::getTypeStoreSize`. Bytes.
    pub fn type_store_size(&self, ty: Type<'_>) -> u64 {
        let bits = self.type_size_in_bits(ty);
        align_to_power_of_two(bits, 8) / 8
    }

    /// Mirrors `DataLayout::getTypeStoreSizeInBits`.
    pub fn type_store_size_in_bits(&self, ty: Type<'_>) -> u64 {
        let bits = self.type_size_in_bits(ty);
        align_to_power_of_two(bits, 8)
    }

    /// Mirrors `DataLayout::typeSizeEqualsStoreSize`.
    pub fn type_size_equals_store_size(&self, ty: Type<'_>) -> bool {
        self.type_size_in_bits(ty) == self.type_store_size_in_bits(ty)
    }

    /// Mirrors `DataLayout::getTypeAllocSize`. Bytes including
    /// trailing alignment padding.
    pub fn type_alloc_size(&self, ty: Type<'_>) -> u64 {
        self.type_alloc_size_inner(ty.module(), ty.id())
    }

    /// Mirrors `DataLayout::getTypeAllocSizeInBits`.
    pub fn type_alloc_size_in_bits(&self, ty: Type<'_>) -> u64 {
        self.type_alloc_size(ty).saturating_mul(8)
    }

    /// Mirrors `DataLayout::getABITypeAlign`.
    pub fn abi_type_align(&self, ty: Type<'_>) -> Align {
        self.alignment(ty.module(), ty.id(), true)
    }

    /// Mirrors `DataLayout::getPrefTypeAlign`.
    pub fn pref_type_align(&self, ty: Type<'_>) -> Align {
        self.alignment(ty.module(), ty.id(), false)
    }

    /// Mirrors `DataLayout::getValueOrABITypeAlignment`. If
    /// `alignment` is set, returns it; otherwise the ABI alignment.
    pub fn value_or_abi_type_align(&self, alignment: Option<Align>, ty: Type<'_>) -> Align {
        alignment.unwrap_or_else(|| self.abi_type_align(ty))
    }

    /// Mirrors `DataLayout::getABIIntegerTypeAlignment(BitWidth)`.
    pub fn abi_integer_type_align(&self, bit_width: u32) -> Align {
        self.integer_alignment(bit_width, true)
    }
}

// --------------------------------------------------------------------------
// Internal: alignment + size walks
// --------------------------------------------------------------------------

impl DataLayout {
    fn type_size_in_bits_inner(&self, module: &Module<'_>, id: TypeId) -> u64 {
        match module.context().type_data(id) {
            TypeData::Label => u64::from(self.pointer_size_in_bits(0)),
            TypeData::Pointer { addr_space } => u64::from(self.pointer_size_in_bits(*addr_space)),
            TypeData::TypedPointer { addr_space, .. } => {
                u64::from(self.pointer_size_in_bits(*addr_space))
            }
            TypeData::Array { elem, n } => {
                n.saturating_mul(self.type_alloc_size_inner(module, *elem).saturating_mul(8))
            }
            TypeData::Struct(_) => self.struct_layout_inner(module, id).size_in_bits(),
            TypeData::Integer { bits } => u64::from(*bits),
            TypeData::Half | TypeData::BFloat => 16,
            TypeData::Float => 32,
            TypeData::Double => 64,
            TypeData::PpcFp128 | TypeData::Fp128 => 128,
            TypeData::X86Fp80 => 80,
            TypeData::X86Amx => 8192,
            TypeData::FixedVector { elem, n } => {
                u64::from(*n).saturating_mul(self.type_size_in_bits_inner(module, *elem))
            }
            TypeData::ScalableVector { elem, min } => {
                // Mirrors upstream: getKnownMinValue path.
                u64::from(*min).saturating_mul(self.type_size_in_bits_inner(module, *elem))
            }
            TypeData::TargetExt(_) => match target_ext_layout_type(module, id) {
                Some(layout) => self.type_size_in_bits_inner(module, layout),
                None => 0,
            },
            // Unsized cases: void, function, label-only, metadata, token.
            TypeData::Void | TypeData::Function { .. } | TypeData::Metadata | TypeData::Token => 0,
        }
    }

    fn type_alloc_size_inner(&self, module: &Module<'_>, id: TypeId) -> u64 {
        match module.context().type_data(id) {
            TypeData::Array { elem, n } => {
                n.saturating_mul(self.type_alloc_size_inner(module, *elem))
            }
            TypeData::Struct(_) => {
                let layout = self.struct_layout_inner(module, id);
                let size = layout.size_in_bytes();
                let is_packed = matches!(
                    module.context().type_data(id),
                    TypeData::Struct(s) if s.body.borrow().as_ref().is_some_and(|b| b.packed)
                );
                if is_packed {
                    size
                } else {
                    let a = std::cmp::max(self.struct_abi_align, layout.alignment);
                    align_to(size, a)
                }
            }
            TypeData::Integer { bits } => {
                let bytes = u64::from(div_ceil_u32(*bits, 8));
                let a = self.integer_alignment(*bits, true);
                align_to(bytes, a)
            }
            TypeData::Pointer { addr_space } => {
                let size = u64::from(self.pointer_size(*addr_space));
                align_to(size, self.pointer_abi_align(*addr_space))
            }
            TypeData::TypedPointer { addr_space, .. } => {
                let size = u64::from(self.pointer_size(*addr_space));
                align_to(size, self.pointer_abi_align(*addr_space))
            }
            TypeData::TargetExt(_) => match target_ext_layout_type(module, id) {
                Some(layout) => self.type_alloc_size_inner(module, layout),
                None => 0,
            },
            _ => {
                let store = self.type_store_size_inner(module, id);
                align_to(store, self.alignment(module, id, true))
            }
        }
    }

    fn type_store_size_inner(&self, module: &Module<'_>, id: TypeId) -> u64 {
        let bits = self.type_size_in_bits_inner(module, id);
        align_to_power_of_two(bits, 8) / 8
    }

    fn alignment(&self, module: &Module<'_>, id: TypeId, abi_or_pref: bool) -> Align {
        match module.context().type_data(id) {
            TypeData::Integer { bits } => self.integer_alignment(*bits, abi_or_pref),
            TypeData::Half
            | TypeData::BFloat
            | TypeData::Float
            | TypeData::Double
            | TypeData::Fp128
            | TypeData::PpcFp128
            | TypeData::X86Fp80 => {
                let bit_width = match module.context().type_data(id) {
                    TypeData::Half | TypeData::BFloat => 16,
                    TypeData::Float => 32,
                    TypeData::Double => 64,
                    TypeData::Fp128 | TypeData::PpcFp128 | TypeData::X86Fp80 => 128,
                    _ => unreachable!(),
                };
                if let Some(s) = self.float_specs.iter().find(|s| s.bit_width == bit_width) {
                    if abi_or_pref {
                        s.abi_align
                    } else {
                        s.pref_align
                    }
                } else {
                    pow2_ceil_align(bit_width / 8)
                }
            }
            TypeData::FixedVector { .. } | TypeData::ScalableVector { .. } => {
                let bit_width = self
                    .type_size_in_bits_inner(module, id)
                    .try_into()
                    .unwrap_or(u32::MAX);
                if let Some(s) = self.vector_specs.iter().find(|s| s.bit_width == bit_width) {
                    if abi_or_pref {
                        s.abi_align
                    } else {
                        s.pref_align
                    }
                } else {
                    let store_bits =
                        align_to_power_of_two(self.type_size_in_bits_inner(module, id), 8);
                    let store_bytes_u32: u32 = (store_bits / 8).try_into().unwrap_or(u32::MAX);
                    pow2_ceil_align(store_bytes_u32)
                }
            }
            TypeData::Pointer { addr_space } => {
                if abi_or_pref {
                    self.pointer_abi_align(*addr_space)
                } else {
                    self.pointer_pref_align(*addr_space)
                }
            }
            TypeData::TypedPointer { addr_space, .. } => {
                if abi_or_pref {
                    self.pointer_abi_align(*addr_space)
                } else {
                    self.pointer_pref_align(*addr_space)
                }
            }
            TypeData::Label => {
                if abi_or_pref {
                    self.pointer_abi_align(0)
                } else {
                    self.pointer_pref_align(0)
                }
            }
            TypeData::Array { elem, .. } => self.alignment(module, *elem, abi_or_pref),
            TypeData::Struct(s) => {
                if s.body.borrow().as_ref().is_some_and(|b| b.packed) && abi_or_pref {
                    Align::ONE
                } else {
                    let layout = self.struct_layout_inner(module, id);
                    if abi_or_pref {
                        std::cmp::max(self.struct_abi_align, layout.alignment)
                    } else {
                        std::cmp::max(self.struct_pref_align, layout.alignment)
                    }
                }
            }
            TypeData::X86Amx => const_align(64),
            TypeData::TargetExt(_) => match target_ext_layout_type(module, id) {
                Some(layout) => self.alignment(module, layout, abi_or_pref),
                None => Align::ONE,
            },
            // Unsized: void, function, metadata, token. Upstream
            // unreachable; we return ONE rather than panic.
            TypeData::Void | TypeData::Function { .. } | TypeData::Metadata | TypeData::Token => {
                Align::ONE
            }
        }
    }

    fn integer_alignment(&self, bit_width: u32, abi_or_pref: bool) -> Align {
        // Mirrors `DataLayout::getIntegerAlignment`. Walk forward,
        // pick the first spec >= bit_width; if none, use the last
        // (largest) spec.
        let mut chosen: Option<&PrimitiveSpec> = None;
        for spec in &self.int_specs {
            chosen = Some(spec);
            if spec.bit_width >= bit_width {
                break;
            }
        }
        let spec = chosen
            .unwrap_or_else(|| unreachable!("DataLayout default seeds at least one int spec"));
        if abi_or_pref {
            spec.abi_align
        } else {
            spec.pref_align
        }
    }

    /// Compute (without caching) the layout of an aggregate struct.
    /// Mirrors `StructLayout::StructLayout` in `DataLayout.cpp`.
    pub fn struct_layout(&self, ty: Type<'_>) -> StructLayoutInfo {
        self.struct_layout_inner(ty.module(), ty.id())
    }

    fn struct_layout_inner(&self, module: &Module<'_>, id: TypeId) -> StructLayoutInfo {
        let s = match module.context().type_data(id) {
            TypeData::Struct(s) => s,
            _ => unreachable!("struct_layout invariant: TypeData::Struct"),
        };
        let body = s.body.borrow();
        let body = body
            .as_ref()
            .unwrap_or_else(|| unreachable!("struct_layout invariant: opaque structs are unsized"));
        let elements: &[TypeId] = &body.elements;
        let packed = body.packed;

        let mut size_bytes: u64 = 0;
        let mut alignment = Align::ONE;
        let mut is_padded = false;
        let mut member_offsets = Vec::with_capacity(elements.len());

        for &elem in elements {
            let ty_align = if packed {
                Align::ONE
            } else {
                self.alignment(module, elem, true)
            };
            if size_bytes % ty_align.value() != 0 {
                is_padded = true;
                size_bytes = align_to(size_bytes, ty_align);
            }
            if ty_align.value() > alignment.value() {
                alignment = ty_align;
            }
            member_offsets.push(size_bytes);
            size_bytes = size_bytes.saturating_add(self.type_alloc_size_inner(module, elem));
        }
        if size_bytes % alignment.value() != 0 {
            is_padded = true;
            size_bytes = align_to(size_bytes, alignment);
        }

        StructLayoutInfo {
            size_bytes,
            alignment,
            is_padded,
            member_offsets,
        }
    }

    fn set_pointer_spec(&mut self, spec: PointerSpec) {
        match self
            .pointer_specs
            .binary_search_by_key(&spec.address_space, |s| s.address_space)
        {
            Ok(idx) => {
                // Mirrors the upstream `update existing` arm: keep
                // the existing `non_integral` flag because that is
                // applied later by the post-pass in
                // `parse_layout_string`.
                let kept_non_integral = self.pointer_specs[idx].non_integral;
                self.pointer_specs[idx] = PointerSpec {
                    non_integral: kept_non_integral || spec.non_integral,
                    ..spec
                };
            }
            Err(idx) => {
                self.pointer_specs.insert(idx, spec);
            }
        }
    }

    fn set_primitive_spec(
        &mut self,
        kind: char,
        bit_width: u32,
        abi_align: Align,
        pref_align: Align,
    ) {
        let specs = match kind {
            'i' => &mut self.int_specs,
            'f' => &mut self.float_specs,
            'v' => &mut self.vector_specs,
            _ => unreachable!("set_primitive_spec invariant: kind in {{i, f, v}}"),
        };
        match specs.binary_search_by_key(&bit_width, |s| s.bit_width) {
            Ok(idx) => {
                specs[idx].abi_align = abi_align;
                specs[idx].pref_align = pref_align;
            }
            Err(idx) => specs.insert(
                idx,
                PrimitiveSpec {
                    bit_width,
                    abi_align,
                    pref_align,
                },
            ),
        }
    }
}

// --------------------------------------------------------------------------
// Struct layout result
// --------------------------------------------------------------------------

/// Computed layout for a single struct type. Mirrors
/// `class StructLayout` in `DataLayout.h` for the values we expose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructLayoutInfo {
    pub size_bytes: u64,
    pub alignment: Align,
    pub is_padded: bool,
    pub member_offsets: Vec<u64>,
}

impl StructLayoutInfo {
    /// Mirrors `StructLayout::getSizeInBytes`.
    #[inline]
    pub fn size_in_bytes(&self) -> u64 {
        self.size_bytes
    }
    /// Mirrors `StructLayout::getSizeInBits`.
    #[inline]
    pub fn size_in_bits(&self) -> u64 {
        self.size_bytes.saturating_mul(8)
    }
    /// Mirrors `StructLayout::getAlignment`.
    #[inline]
    pub fn alignment(&self) -> Align {
        self.alignment
    }
    /// Mirrors `StructLayout::hasPadding`.
    #[inline]
    pub fn has_padding(&self) -> bool {
        self.is_padded
    }
    /// Mirrors `StructLayout::getElementOffset`.
    pub fn element_offset(&self, index: usize) -> u64 {
        self.member_offsets[index]
    }
    /// Mirrors `StructLayout::getElementOffsetInBits`.
    pub fn element_offset_in_bits(&self, index: usize) -> u64 {
        self.member_offsets[index].saturating_mul(8)
    }
    /// Mirrors `StructLayout::getElementContainingOffset`. Returns
    /// the index of the field that contains the byte offset, or
    /// `None` for an out-of-range offset.
    pub fn element_containing_offset(&self, byte_offset: u64) -> Option<usize> {
        if byte_offset >= self.size_bytes {
            return None;
        }
        // upper_bound: greatest index whose member_offset <= byte_offset.
        match self
            .member_offsets
            .binary_search_by(|m| m.cmp(&byte_offset))
        {
            Ok(idx) => Some(idx),
            Err(0) => None,
            Err(idx) => Some(idx - 1),
        }
    }
}

// --------------------------------------------------------------------------
// Display: emit string_representation
// --------------------------------------------------------------------------

impl fmt::Display for DataLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.string_representation)
    }
}

// --------------------------------------------------------------------------
// Parser
// --------------------------------------------------------------------------

impl DataLayout {
    /// Top-level parser. Mirrors
    /// `DataLayout::parseLayoutString(StringRef)`.
    fn parse_layout_string(&mut self, layout: &str) -> IrResult<()> {
        self.string_representation = layout.to_owned();
        if layout.is_empty() {
            return Ok(());
        }
        let mut non_integral_address_spaces: Vec<u32> = Vec::new();
        let mut addr_space_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for spec in layout.split('-') {
            if spec.is_empty() {
                return Err(invalid("empty specification is not allowed".into()));
            }
            self.parse_specification(
                spec,
                &mut non_integral_address_spaces,
                &mut addr_space_names,
            )?;
        }
        for as_id in non_integral_address_spaces {
            // Mirrors the post-pass: clone the existing spec for the
            // given AS (or AS 0 if there is none), then mark it
            // non-integral.
            let base = self.pointer_spec(as_id).clone();
            self.set_pointer_spec(PointerSpec {
                address_space: as_id,
                has_unstable_repr: true,
                non_integral: true,
                ..base
            });
        }
        Ok(())
    }

    /// Mirrors `DataLayout::parseSpecification`.
    fn parse_specification(
        &mut self,
        spec: &str,
        non_integral_address_spaces: &mut Vec<u32>,
        addr_space_names: &mut std::collections::HashSet<String>,
    ) -> IrResult<()> {
        // Two-character `ni` specifier first. Mirrors upstream's
        // `Spec.starts_with("ni")` arm.
        if let Some(rest) = spec.strip_prefix("ni") {
            let rest = rest
                .strip_prefix(':')
                .ok_or_else(|| spec_format("ni:<address space>[:<address space>]..."))?;
            for s in rest.split(':') {
                let addr = parse_addr_space(s)?;
                if addr == 0 {
                    return Err(invalid("address space 0 cannot be non-integral".into()));
                }
                non_integral_address_spaces.push(addr);
            }
            return Ok(());
        }

        // Single-character specifiers.
        let specifier = spec
            .chars()
            .next()
            .ok_or_else(|| invalid("empty specification".into()))?;
        match specifier {
            'i' | 'f' | 'v' => self.parse_primitive_spec(spec),
            'a' => self.parse_aggregate_spec(spec),
            'p' => self.parse_pointer_spec(spec, addr_space_names),
            's' => Ok(()), // Deprecated, ignored for backwards compat.
            'e' => {
                if spec.len() != 1 {
                    return Err(invalid(
                        "malformed specification, must be just 'e' or 'E'".into(),
                    ));
                }
                self.big_endian = false;
                Ok(())
            }
            'E' => {
                if spec.len() != 1 {
                    return Err(invalid(
                        "malformed specification, must be just 'e' or 'E'".into(),
                    ));
                }
                self.big_endian = true;
                Ok(())
            }
            'n' => {
                let rest = &spec[1..];
                for s in rest.split(':') {
                    let bits = parse_size(s, "size")?;
                    self.legal_int_widths.push(bits);
                }
                Ok(())
            }
            'S' => {
                let rest = &spec[1..];
                if rest.is_empty() {
                    return Err(spec_format("S<size>"));
                }
                let align = parse_alignment(rest, "stack natural", false)?;
                self.stack_natural_align = Some(align);
                Ok(())
            }
            'F' => {
                let rest = &spec[1..];
                if rest.is_empty() {
                    return Err(spec_format("F<type><abi>"));
                }
                let kind = rest.as_bytes()[0];
                let after = &rest[1..];
                self.function_ptr_align_type = match kind {
                    b'i' => FunctionPtrAlignType::Independent,
                    b'n' => FunctionPtrAlignType::MultipleOfFunctionAlign,
                    other => {
                        return Err(invalid(format!(
                            "unknown function pointer alignment type '{}'",
                            char::from(other)
                        )));
                    }
                };
                let align = parse_alignment(after, "ABI", false)?;
                self.function_ptr_align = Some(align);
                Ok(())
            }
            'P' => {
                let rest = &spec[1..];
                if rest.is_empty() {
                    return Err(spec_format("P<address space>"));
                }
                self.program_addr_space = parse_addr_space(rest)?;
                Ok(())
            }
            'A' => {
                let rest = &spec[1..];
                if rest.is_empty() {
                    return Err(spec_format("A<address space>"));
                }
                self.alloca_addr_space = parse_addr_space(rest)?;
                Ok(())
            }
            'G' => {
                let rest = &spec[1..];
                if rest.is_empty() {
                    return Err(spec_format("G<address space>"));
                }
                self.default_globals_addr_space = parse_addr_space(rest)?;
                Ok(())
            }
            'm' => {
                let rest = spec
                    .strip_prefix("m:")
                    .ok_or_else(|| spec_format("m:<mangling>"))?;
                if rest.is_empty() {
                    return Err(spec_format("m:<mangling>"));
                }
                if rest.len() > 1 {
                    return Err(invalid("unknown mangling mode".into()));
                }
                self.mangling_mode = match rest.as_bytes()[0] {
                    b'e' => ManglingMode::Elf,
                    b'l' => ManglingMode::Goff,
                    b'o' => ManglingMode::MachO,
                    b'm' => ManglingMode::Mips,
                    b'w' => ManglingMode::WinCoff,
                    b'x' => ManglingMode::WinCoffX86,
                    b'a' => ManglingMode::XCoff,
                    _ => return Err(invalid("unknown mangling mode".into())),
                };
                Ok(())
            }
            other => Err(invalid(format!("unknown specifier '{other}'"))),
        }
    }

    /// Mirrors `DataLayout::parsePrimitiveSpec`.
    fn parse_primitive_spec(&mut self, spec: &str) -> IrResult<()> {
        let kind = spec.as_bytes()[0];
        let components: Vec<&str> = spec[1..].split(':').collect();
        if components.len() < 2 || components.len() > 3 {
            return Err(spec_format("[ifv]<size>:<abi>[:<pref>]"));
        }
        let bit_width = parse_size(components[0], "size")?;
        let abi_align = parse_alignment(components[1], "ABI", false)?;
        if kind == b'i' && bit_width == 8 && abi_align.value() != 1 {
            return Err(invalid("i8 must be 8-bit aligned".into()));
        }
        let pref_align = if components.len() > 2 {
            parse_alignment(components[2], "preferred", false)?
        } else {
            abi_align
        };
        if pref_align.value() < abi_align.value() {
            return Err(invalid(
                "preferred alignment cannot be less than the ABI alignment".into(),
            ));
        }
        self.set_primitive_spec(char::from(kind), bit_width, abi_align, pref_align);
        Ok(())
    }

    /// Mirrors `DataLayout::parseAggregateSpec`.
    fn parse_aggregate_spec(&mut self, spec: &str) -> IrResult<()> {
        let components: Vec<&str> = spec[1..].split(':').collect();
        if components.len() < 2 || components.len() > 3 {
            return Err(spec_format("a:<abi>[:<pref>]"));
        }
        // <size> component must be empty or the literal "0".
        if !components[0].is_empty() {
            let bit_width: u32 = components[0]
                .parse()
                .map_err(|_| invalid("size must be zero".into()))?;
            if bit_width != 0 {
                return Err(invalid("size must be zero".into()));
            }
        }
        let abi_align = parse_alignment(components[1], "ABI", true)?;
        let pref_align = if components.len() > 2 {
            parse_alignment(components[2], "preferred", false)?
        } else {
            abi_align
        };
        if pref_align.value() < abi_align.value() {
            return Err(invalid(
                "preferred alignment cannot be less than the ABI alignment".into(),
            ));
        }
        self.struct_abi_align = abi_align;
        self.struct_pref_align = pref_align;
        Ok(())
    }

    /// Mirrors `DataLayout::parsePointerSpec`.
    fn parse_pointer_spec(
        &mut self,
        spec: &str,
        addr_space_names: &mut std::collections::HashSet<String>,
    ) -> IrResult<()> {
        let components: Vec<&str> = spec[1..].split(':').collect();
        if components.len() < 3 || components.len() > 5 {
            return Err(spec_format("p[<n>]:<size>:<abi>[:<pref>[:<idx>]]"));
        }

        // Address-space component: optional flags 'e' / 'u' followed
        // by an optional decimal AS, optionally followed by `(name)`.
        let mut addr_str = components[0];
        let mut has_external_state = false;
        let mut has_unstable_repr = false;
        while let Some(c) = addr_str.chars().next() {
            if c == 'e' {
                has_external_state = true;
                addr_str = &addr_str[1..];
            } else if c == 'u' {
                has_unstable_repr = true;
                addr_str = &addr_str[1..];
            } else if c.is_ascii_alphabetic() {
                return Err(invalid(format!(
                    "'{c}' is not a valid pointer specification flag"
                )));
            } else {
                break;
            }
        }
        let (addr_space, addr_space_name) = if addr_str.is_empty() {
            (0u32, String::new())
        } else {
            parse_addr_space_and_name(addr_str)?
        };
        if addr_space == 0 && (has_external_state || has_unstable_repr) {
            return Err(invalid(
                "address space 0 cannot be unstable or have external state".into(),
            ));
        }
        if !addr_space_name.is_empty() && !addr_space_names.insert(addr_space_name.clone()) {
            return Err(invalid(format!(
                "address space name `{addr_space_name}` already used"
            )));
        }

        let bit_width = parse_size(components[1], "pointer size")?;
        let abi_align = parse_alignment(components[2], "ABI", false)?;
        let pref_align = if components.len() > 3 {
            parse_alignment(components[3], "preferred", false)?
        } else {
            abi_align
        };
        if pref_align.value() < abi_align.value() {
            return Err(invalid(
                "preferred alignment cannot be less than the ABI alignment".into(),
            ));
        }
        let index_bit_width = if components.len() > 4 {
            parse_size(components[4], "index size")?
        } else {
            bit_width
        };
        if index_bit_width > bit_width {
            return Err(invalid(
                "index size cannot be larger than the pointer size".into(),
            ));
        }

        self.set_pointer_spec(PointerSpec {
            address_space: addr_space,
            bit_width,
            abi_align,
            pref_align,
            index_bit_width,
            has_unstable_repr,
            has_external_state,
            address_space_name: addr_space_name,
            non_integral: false,
        });
        Ok(())
    }
}

// --------------------------------------------------------------------------
// Static helpers (mirror the file-level helpers in DataLayout.cpp)
// --------------------------------------------------------------------------

/// Mirrors `parseSize`: 24-bit non-zero unsigned integer.
fn parse_size(s: &str, name: &str) -> IrResult<u32> {
    if s.is_empty() {
        return Err(invalid(format!("{name} component cannot be empty")));
    }
    let v: u64 = s
        .parse()
        .map_err(|_| invalid(format!("{name} must be a non-zero 24-bit integer")))?;
    if v == 0 || v > (1u64 << 24) - 1 {
        return Err(invalid(format!("{name} must be a non-zero 24-bit integer")));
    }
    let v: u32 = v
        .try_into()
        .unwrap_or_else(|_| unreachable!("v fits in 24 bits, hence u32"));
    Ok(v)
}

/// Mirrors `parseAlignment`. Returns the byte alignment (input is
/// the "in-bits" alignment; must be a power-of-two times the byte
/// width).
fn parse_alignment(s: &str, name: &str, allow_zero: bool) -> IrResult<Align> {
    if s.is_empty() {
        return Err(invalid(format!(
            "{name} alignment component cannot be empty"
        )));
    }
    let v: u64 = s
        .parse()
        .map_err(|_| invalid(format!("{name} alignment must be a 16-bit integer")))?;
    if v > u64::from(u16::MAX) {
        return Err(invalid(format!(
            "{name} alignment must be a 16-bit integer"
        )));
    }
    if v == 0 {
        if !allow_zero {
            return Err(invalid(format!("{name} alignment must be non-zero")));
        }
        return Ok(Align::ONE);
    }
    if v % 8 != 0 || !is_power_of_two(v / 8) {
        return Err(invalid(format!(
            "{name} alignment must be a power of two times the byte width"
        )));
    }
    Align::new(v / 8)
}

/// Mirrors `parseAddrSpace`.
fn parse_addr_space(s: &str) -> IrResult<u32> {
    if s.is_empty() {
        return Err(invalid("address space component cannot be empty".into()));
    }
    let v: u64 = s
        .parse()
        .map_err(|_| invalid("address space must be a 24-bit integer".into()))?;
    if v > (1u64 << 24) - 1 {
        return Err(invalid("address space must be a 24-bit integer".into()));
    }
    Ok(v.try_into()
        .unwrap_or_else(|_| unreachable!("AS fits in 24 bits, hence u32")))
}

/// Mirrors `parseAddrSpaceAndName`. Accepts `<digits>` or
/// `<digits>(<name>)` or `(<name>)`.
fn parse_addr_space_and_name(s: &str) -> IrResult<(u32, String)> {
    if s.is_empty() {
        return Err(invalid("address space component cannot be empty".into()));
    }
    let mut bytes = s;
    let mut addr_space: u32 = 0;
    if bytes.as_bytes()[0].is_ascii_digit() {
        // Take leading digits.
        let mut end = 0;
        while end < bytes.len() && bytes.as_bytes()[end].is_ascii_digit() {
            end += 1;
        }
        let digits = &bytes[..end];
        let v: u64 = digits
            .parse()
            .map_err(|_| invalid("address space must be a 24-bit integer".into()))?;
        if v > (1u64 << 24) - 1 {
            return Err(invalid("address space must be a 24-bit integer".into()));
        }
        addr_space = v
            .try_into()
            .unwrap_or_else(|_| unreachable!("AS fits in u32"));
        bytes = &bytes[end..];
    }
    if bytes.is_empty() {
        return Ok((addr_space, String::new()));
    }
    if !bytes.starts_with('(') {
        return Err(invalid("address space must be a 24-bit integer".into()));
    }
    if !bytes.ends_with(')') || bytes.len() == 2 {
        return Err(invalid("Expected `( address space name )`".into()));
    }
    let name = &bytes[1..bytes.len() - 1];
    if name.len() == 1 {
        let c = name.as_bytes()[0];
        if c == b'P' || c == b'G' || c == b'A' {
            return Err(invalid(
                "Cannot use predefined address space names P/G/A in data layout".into(),
            ));
        }
    }
    Ok((addr_space, name.to_owned()))
}

// --------------------------------------------------------------------------
// Target-extension type layout table
// --------------------------------------------------------------------------

/// Mirrors `getTargetTypeInfo(...).LayoutType` in `lib/IR/Type.cpp`.
/// Returns the type id of the in-memory layout type for the given
/// target-extension type, or `None` for unknown / void-equivalent
/// extensions. The full upstream table covers SPIR-V, AArch64
/// (`aarch64.svcount`), RISC-V (`riscv.vector.tuple`), DirectX
/// (`dx.*`), AMDGPU (`amdgcn.named.barrier`), and the test
/// extension (`llvm.test.vectorelement`).
fn target_ext_layout_type(module: &Module<'_>, id: TypeId) -> Option<TypeId> {
    let data = module.context().type_data(id);
    let TypeData::TargetExt(ext) = data else {
        return None;
    };
    let ctx = module.context();
    let name = ext.name.as_str();

    // SPIR-V images / signed images: opaque pointer in AS 0.
    if name == "spirv.Image" || name == "spirv.SignedImage" {
        return Some(ctx.ptr_type(0));
    }
    // SPIR-V typed reads with explicit size + alignment.
    if name == "spirv.Type" && ext.int_params.len() >= 3 {
        let size_bytes = u64::from(ext.int_params[1]);
        let align_bits = u64::from(ext.int_params[2]);
        if size_bytes > 0 && align_bits > 0 {
            // ArrayType::get(IntN, size * 8 / align).
            let int_bits: u32 = align_bits.try_into().unwrap_or(u32::MAX);
            let elem = ctx.int_type(int_bits);
            let n = (size_bytes.saturating_mul(8)) / align_bits;
            return Some(ctx.array_type(elem, n));
        }
        return Some(ctx.int_type(32));
    }
    // SPIR-V integral constants / literal: void.
    if name == "spirv.IntegralConstant" || name == "spirv.Literal" {
        return Some(ctx.void());
    }
    // SPIR-V padding: byte-array of N bytes.
    if name == "spirv.Padding" {
        let n = u64::from(*ext.int_params.first().unwrap_or(&0));
        return Some(ctx.array_type(ctx.int_type(8), n));
    }
    // Other SPIR-V types: opaque pointer.
    if name.starts_with("spirv.") {
        return Some(ctx.ptr_type(0));
    }
    // AArch64 svcount: <vscale x 16 x i1>.
    if name == "aarch64.svcount" {
        return Some(ctx.scalable_vector_type(ctx.int_type(1), 16));
    }
    // RISC-V vector tuple. Layout = <vscale x N x i8> where N is
    // computed from the tuple's first scalable-vector type parameter.
    if name == "riscv.vector.tuple"
        && let Some(first) = ext.type_params.first()
        && let TypeData::ScalableVector { min, .. } = ctx.type_data(*first)
    {
        // Upstream uses `RISCV::RVVBytesPerBlock` (== 8) as the floor.
        const RVV_BYTES_PER_BLOCK: u32 = 8;
        let lanes = core::cmp::max(*min, RVV_BYTES_PER_BLOCK);
        let count_factor = ext.int_params.first().copied().unwrap_or(0);
        let total = lanes.saturating_mul(count_factor);
        return Some(ctx.scalable_vector_type(ctx.int_type(8), total));
    }
    // DirectX padding.
    if name == "dx.Padding" {
        let n = u64::from(*ext.int_params.first().unwrap_or(&0));
        return Some(ctx.array_type(ctx.int_type(8), n));
    }
    // Other DirectX types: opaque pointer.
    if name.starts_with("dx.") {
        return Some(ctx.ptr_type(0));
    }
    // AMDGPU named.barrier: <4 x i32>.
    if name == "amdgcn.named.barrier" {
        return Some(ctx.fixed_vector_type(ctx.int_type(32), 4));
    }
    // Vector-element test extension: i32.
    if name == "llvm.test.vectorelement" {
        return Some(ctx.int_type(32));
    }
    // Default: void (zero-sized layout).
    Some(ctx.void())
}

// --------------------------------------------------------------------------
// Math helpers
// --------------------------------------------------------------------------

#[inline]
fn is_power_of_two(v: u64) -> bool {
    v != 0 && (v & (v - 1)) == 0
}

#[inline]
fn div_ceil_u32(value: u32, denom: u32) -> u32 {
    value.div_ceil(denom)
}

#[inline]
fn align_to(value: u64, align: Align) -> u64 {
    let a = align.value();
    (value + a - 1) & !(a - 1)
}

#[inline]
fn align_to_power_of_two(value: u64, align: u64) -> u64 {
    debug_assert!(is_power_of_two(align));
    (value + align - 1) & !(align - 1)
}

/// Mirrors `Align(PowerOf2Ceil(N))`. Rounds `bytes` up to the next
/// power-of-two and wraps in [`Align`].
fn pow2_ceil_align(bytes: u32) -> Align {
    let mut v: u64 = u64::from(bytes.max(1));
    if !is_power_of_two(v) {
        let mut p: u64 = 1;
        while p < v {
            p <<= 1;
        }
        v = p;
    }
    Align::new(v).unwrap_or(Align::ONE)
}

/// Crate-internal: build a guaranteed-power-of-two `Align` at compile
/// time. `n` MUST be a power of two; the helper panics in tests if
/// not (debug-only).
fn const_align(n: u64) -> Align {
    debug_assert!(is_power_of_two(n));
    Align::new(n).unwrap_or(Align::ONE)
}

// --------------------------------------------------------------------------
// Error helpers
// --------------------------------------------------------------------------

fn invalid(reason: String) -> IrError {
    IrError::InvalidDataLayout { reason }
}

fn spec_format(form: &str) -> IrError {
    invalid(format!("malformed specification, expected {form}"))
}
