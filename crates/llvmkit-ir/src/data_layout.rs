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
//! - `getGEPIndicesForOffset` / `getIndexedOffsetInType` (requires the
//!   remaining DataLayout offset-decomposition helpers beyond the current
//!   parser-needed ConstantExpr subset).
//! - `hasMicrosoftFastStdCallMangling` and friends (presentation-only
//!   helpers; reachable through [`DataLayout::mangling_mode`]).

use core::fmt;
use core::iter::FusedIterator;
use core::num::NonZeroU32;
use core::str::FromStr;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use crate::addr_space::AddressSpace;
use crate::align::{Align, MaybeAlign};
use crate::capability::Capability;
use crate::derived_types::{SizedType, StructType};
use crate::error::DataLayoutError;
use crate::module::{ModuleBrand, ModuleCore, ModuleId};
use crate::struct_body_state::StructBodyState;
use crate::r#type::{MAX_INT_BITS, Type, TypeData, TypeSlot, TypeSlotAccess, is_scalable};
use crate::type_components::lanes;
use crate::type_size::TypeSize;

// --------------------------------------------------------------------------
// Sub-records
// --------------------------------------------------------------------------

/// Per-bit-width primitive alignment (`i<N>:<abi>:<pref>` / `f<...>` /
/// `v<...>`). Mirrors `DataLayout::PrimitiveSpec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrimitiveSpec {
    bit_width: u32,
    abi_align: Align,
    pref_align: Align,
}

impl PrimitiveSpec {
    pub const fn new(bit_width: u32, abi_align: Align, pref_align: Align) -> Self {
        Self {
            bit_width,
            abi_align,
            pref_align,
        }
    }

    #[inline]
    pub const fn bit_width(&self) -> u32 {
        self.bit_width
    }

    #[inline]
    pub const fn abi_align(&self) -> Align {
        self.abi_align
    }

    #[inline]
    pub const fn pref_align(&self) -> Align {
        self.pref_align
    }

    fn set_alignments(&mut self, abi_align: Align, pref_align: Align) {
        self.abi_align = abi_align;
        self.pref_align = pref_align;
    }
}

/// Per-address-space pointer specification
/// (`p[<flags+as>]:<size>:<abi>:<pref>[:<idx>]`). Mirrors
/// `DataLayout::PointerSpec`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PointerSpec {
    address_space: AddressSpace,
    bit_width: u32,
    abi_align: Align,
    pref_align: Align,
    index_bit_width: u32,
    /// Pointer values in this address space have an unstable
    /// representation: bit-identical pointers may compare unequal,
    /// or `inttoptr(ptrtoint(p))` may not equal `p`. Mirrors the `u`
    /// flag in `parsePointerSpec`.
    has_unstable_repr: bool,
    /// Pointers in this address space carry external state stored
    /// outside the in-memory bit pattern (e.g. CHERI capability
    /// tags). Mirrors the `e` flag in `parsePointerSpec`.
    has_external_state: bool,
    /// Symbolic name (`p1(global):...` -> `"global"`). Empty string
    /// if no name is set.
    address_space_name: String,
}

impl PointerSpec {
    fn new(
        address_space: AddressSpace,
        bit_width: u32,
        abi_align: Align,
        pref_align: Align,
        index_bit_width: u32,
    ) -> Self {
        Self {
            address_space,
            bit_width,
            abi_align,
            pref_align,
            index_bit_width,
            has_unstable_repr: false,
            has_external_state: false,
            address_space_name: String::new(),
        }
    }

    fn with_pointer_flags(mut self, has_unstable_repr: bool, has_external_state: bool) -> Self {
        self.has_unstable_repr = has_unstable_repr;
        self.has_external_state = has_external_state;
        self
    }

    fn with_address_space_name(mut self, address_space_name: String) -> Self {
        self.address_space_name = address_space_name;
        self
    }

    #[inline]
    pub const fn address_space(&self) -> AddressSpace {
        self.address_space
    }

    #[inline]
    pub const fn bit_width(&self) -> u32 {
        self.bit_width
    }

    #[inline]
    pub const fn abi_align(&self) -> Align {
        self.abi_align
    }

    #[inline]
    pub const fn pref_align(&self) -> Align {
        self.pref_align
    }

    #[inline]
    pub const fn index_bit_width(&self) -> u32 {
        self.index_bit_width
    }

    #[inline]
    pub const fn has_unstable_representation(&self) -> bool {
        self.has_unstable_repr
    }

    #[inline]
    pub const fn has_unstable_repr(&self) -> bool {
        self.has_unstable_representation()
    }

    #[inline]
    pub const fn has_external_state(&self) -> bool {
        self.has_external_state
    }

    #[inline]
    pub fn address_space_name(&self) -> &str {
        &self.address_space_name
    }

    /// `DataLayout::isNonIntegralAddressSpace`'s condition: the pointer is more
    /// than an address, because its index is narrower than its bits, or
    /// because of either flag. `ni:<as>` is one way to set a flag, not the
    /// only way to answer `true`.
    #[inline]
    pub const fn is_non_integral(&self) -> bool {
        self.bit_width != self.index_bit_width || self.has_unstable_repr || self.has_external_state
    }

    #[inline]
    pub const fn non_integral(&self) -> bool {
        self.is_non_integral()
    }

    /// The post-pass of `parseLayoutString`: `ni:<as>` overrides any `p[e][u]`,
    /// so a `ni` space is unstable and has no external state.
    fn into_non_integral_address_space(mut self, address_space: AddressSpace) -> Self {
        self.address_space = address_space;
        self.has_unstable_repr = true;
        self.has_external_state = false;
        self
    }
}

/// Function-pointer alignment kind. Mirrors
/// `DataLayout::FunctionPtrAlignType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
    Xcoff,
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
            Self::Xcoff => Some("a"),
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
///
/// `Hash` alongside `Eq` because a layout is a natural cache key — target
/// size/alignment queries are pure functions of it, and every spec list is
/// stored in a canonical order, so two layouts that compare equal hash equal.
/// Neither reads the layout string: see the `PartialEq` impl.
#[derive(Debug, Clone)]
pub struct DataLayout {
    big_endian: bool,
    alloca_addr_space: AddressSpace,
    program_addr_space: AddressSpace,
    default_globals_addr_space: AddressSpace,
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
    /// The struct layouts this `DataLayout` has computed. Mirrors
    /// `DataLayout::LayoutMap`; not part of the layout's value, so equality
    /// and hashing leave it out, and a clone starts with an empty one, as
    /// upstream's `operator=` leaves the copy's (see [`StructLayoutMap`]).
    struct_layouts: StructLayoutMap,
}

/// Mirrors `DataLayout::operator==` (`DataLayout.cpp`), field for field.
/// The layout string is left out, as upstream leaves it out ("NOTE:
/// StringRepresentation might differ, it is not canonicalized"): `p00` and
/// `p0` describe one layout, and so do the default and `"e"`.
impl PartialEq for DataLayout {
    fn eq(&self, other: &Self) -> bool {
        // Destructured, so a field added to `DataLayout` must be placed here
        // (compared, or named as left out) before this compiles.
        let Self {
            big_endian,
            alloca_addr_space,
            program_addr_space,
            default_globals_addr_space,
            stack_natural_align,
            function_ptr_align,
            function_ptr_align_type,
            mangling_mode,
            legal_int_widths,
            int_specs,
            float_specs,
            vector_specs,
            pointer_specs,
            struct_abi_align,
            struct_pref_align,
            string_representation: _,
            struct_layouts: _,
        } = self;
        *big_endian == other.big_endian
            && *alloca_addr_space == other.alloca_addr_space
            && *program_addr_space == other.program_addr_space
            && *default_globals_addr_space == other.default_globals_addr_space
            && *stack_natural_align == other.stack_natural_align
            && *function_ptr_align == other.function_ptr_align
            && *function_ptr_align_type == other.function_ptr_align_type
            && *mangling_mode == other.mangling_mode
            && *legal_int_widths == other.legal_int_widths
            && *int_specs == other.int_specs
            && *float_specs == other.float_specs
            && *vector_specs == other.vector_specs
            && *pointer_specs == other.pointer_specs
            && *struct_abi_align == other.struct_abi_align
            && *struct_pref_align == other.struct_pref_align
    }
}

impl Eq for DataLayout {}

/// Hashes exactly the fields [`PartialEq`] compares, so equal layouts hash
/// equal; destructured for the same reason.
impl core::hash::Hash for DataLayout {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        let Self {
            big_endian,
            alloca_addr_space,
            program_addr_space,
            default_globals_addr_space,
            stack_natural_align,
            function_ptr_align,
            function_ptr_align_type,
            mangling_mode,
            legal_int_widths,
            int_specs,
            float_specs,
            vector_specs,
            pointer_specs,
            struct_abi_align,
            struct_pref_align,
            string_representation: _,
            struct_layouts: _,
        } = self;
        big_endian.hash(state);
        alloca_addr_space.hash(state);
        program_addr_space.hash(state);
        default_globals_addr_space.hash(state);
        stack_natural_align.hash(state);
        function_ptr_align.hash(state);
        function_ptr_align_type.hash(state);
        mangling_mode.hash(state);
        legal_int_widths.hash(state);
        int_specs.hash(state);
        float_specs.hash(state);
        vector_specs.hash(state);
        pointer_specs.hash(state);
        struct_abi_align.hash(state);
        struct_pref_align.hash(state);
    }
}

impl Default for DataLayout {
    /// Mirrors the default `DataLayout::DataLayout()` ctor: seeds the
    /// platform-agnostic primitive / pointer specs, then leaves the
    /// rest of the fields zero-default.
    fn default() -> Self {
        let mut layout = Self {
            big_endian: false,
            alloca_addr_space: AddressSpace::DEFAULT,
            program_addr_space: AddressSpace::DEFAULT,
            default_globals_addr_space: AddressSpace::DEFAULT,
            stack_natural_align: None,
            function_ptr_align: None,
            function_ptr_align_type: FunctionPtrAlignType::Independent,
            mangling_mode: ManglingMode::None,
            legal_int_widths: Vec::new(),
            int_specs: vec![
                PrimitiveSpec::new(8, Align::ONE, Align::ONE),
                PrimitiveSpec::new(16, const_align(2), const_align(2)),
                PrimitiveSpec::new(32, const_align(4), const_align(4)),
                PrimitiveSpec::new(64, const_align(4), const_align(8)),
            ],
            float_specs: vec![
                PrimitiveSpec::new(16, const_align(2), const_align(2)),
                PrimitiveSpec::new(32, const_align(4), const_align(4)),
                PrimitiveSpec::new(64, const_align(8), const_align(8)),
                PrimitiveSpec::new(128, const_align(16), const_align(16)),
            ],
            vector_specs: vec![
                PrimitiveSpec::new(64, const_align(8), const_align(8)),
                PrimitiveSpec::new(128, const_align(16), const_align(16)),
            ],
            pointer_specs: Vec::new(),
            struct_abi_align: Align::ONE,
            struct_pref_align: const_align(8),
            string_representation: String::new(),
            struct_layouts: StructLayoutMap::default(),
        };
        // Default pointer spec for AS 0: 64-bit pointers, 8-byte
        // ABI/pref alignment, 64-bit index width. Mirrors the trailing
        // `setPointerSpec(0, ...)` call in `DataLayout::DataLayout()`.
        layout.set_pointer_spec(PointerSpec::new(
            AddressSpace::DEFAULT,
            64,
            const_align(8),
            const_align(8),
            64,
        ));
        layout
    }
}

impl DataLayout {
    /// Parse a layout string. Mirrors
    /// `static Expected<DataLayout> DataLayout::parse(StringRef)`.
    ///
    /// # Errors
    ///
    /// [`DataLayoutError`] on the first parse failure.
    pub fn parse<S>(s: S) -> Result<Self, DataLayoutError>
    where
        S: AsRef<str>,
    {
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
    pub fn alloca_addr_space(&self) -> AddressSpace {
        self.alloca_addr_space
    }

    /// Mirrors `DataLayout::getProgramAddressSpace`.
    #[inline]
    pub fn program_addr_space(&self) -> AddressSpace {
        self.program_addr_space
    }

    /// Mirrors `DataLayout::getDefaultGlobalsAddressSpace`.
    #[inline]
    pub fn default_globals_addr_space(&self) -> AddressSpace {
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

    /// Mirrors `DataLayout::getNonStandardAddressSpaces`. Yields the
    /// address spaces whose pointer spec is non-integral, in spec order:
    /// `HasUnstableRepresentation`, `HasExternalState`, or a `BitWidth` other
    /// than the `IndexBitWidth`. A spec that is none of those is standard
    /// however its address space is numbered.
    ///
    /// Borrows the layout — a [`DataLayout`] has no interior mutability, so
    /// nothing has to be copied out. No [`ExactSizeIterator`]: the length is a
    /// *filtered* count, unknowable without walking the specs, and a `len()`
    /// that walks would be a lie about cost. Use `.count()` when the number is
    /// what you want.
    ///
    /// [`ExactSizeIterator`]: core::iter::ExactSizeIterator
    pub fn non_standard_address_spaces(
        &self,
    ) -> impl DoubleEndedIterator<Item = AddressSpace> + FusedIterator + Clone + '_ {
        self.pointer_specs
            .iter()
            .filter(|s| s.is_non_integral())
            .map(PointerSpec::address_space)
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

    /// Mirrors `DataLayout::isNonIntegralAddressSpace`.
    pub fn is_non_integral_address_space(&self, addr_space: AddressSpace) -> bool {
        self.pointer_spec(addr_space).is_non_integral()
    }

    /// Mirrors `DataLayout::isNonIntegralPointerType(Type *)`: `false` for
    /// anything but a pointer or a vector of them.
    pub fn is_non_integral_pointer_type<B: ModuleBrand, C: Capability>(
        &self,
        ty: Type<'_, B, C>,
    ) -> bool {
        scalar_pointer_address_space(ty)
            .is_some_and(|space| self.is_non_integral_address_space(space))
    }

    /// Mirrors `DataLayout::mustNotIntroduceIntToPtr(unsigned)`.
    pub fn must_not_introduce_int_to_ptr(&self, addr_space: AddressSpace) -> bool {
        self.has_unstable_representation(addr_space) || self.has_external_state(addr_space)
    }

    /// Mirrors `DataLayout::mustNotIntroducePtrToInt(unsigned)`.
    pub fn must_not_introduce_ptr_to_int(&self, addr_space: AddressSpace) -> bool {
        self.has_unstable_representation(addr_space)
    }

    /// Mirrors `DataLayout::mustNotIntroduceIntToPtr(Type *)`: `false` for
    /// anything but a pointer or a vector of them.
    pub fn must_not_introduce_int_to_ptr_for_type<B: ModuleBrand, C: Capability>(
        &self,
        ty: Type<'_, B, C>,
    ) -> bool {
        scalar_pointer_address_space(ty)
            .is_some_and(|space| self.must_not_introduce_int_to_ptr(space))
    }

    /// Mirrors `DataLayout::mustNotIntroducePtrToInt(Type *)`: `false` for
    /// anything but a pointer or a vector of them.
    pub fn must_not_introduce_ptr_to_int_for_type<B: ModuleBrand, C: Capability>(
        &self,
        ty: Type<'_, B, C>,
    ) -> bool {
        scalar_pointer_address_space(ty)
            .is_some_and(|space| self.must_not_introduce_ptr_to_int(space))
    }

    /// Mirrors `DataLayout::hasUnstableRepresentation(unsigned)`.
    pub fn has_unstable_representation(&self, addr_space: AddressSpace) -> bool {
        self.pointer_spec(addr_space).has_unstable_representation()
    }

    /// Mirrors `DataLayout::hasExternalState(unsigned)`.
    pub fn has_external_state(&self, addr_space: AddressSpace) -> bool {
        self.pointer_spec(addr_space).has_external_state()
    }

    /// Mirrors `DataLayout::getAddressSpaceName`.
    pub fn address_space_name(&self, addr_space: AddressSpace) -> &str {
        self.pointer_spec(addr_space).address_space_name()
    }

    /// Mirrors `DataLayout::getNamedAddressSpace`.
    pub fn named_address_space(&self, name: &str) -> Option<AddressSpace> {
        self.pointer_specs
            .iter()
            .find(|s| s.address_space_name() == name)
            .map(PointerSpec::address_space)
    }

    // ----- Pointer accessors -----

    /// Mirrors `DataLayout::getPointerSize(AS)`. Bytes.
    pub fn pointer_size(&self, addr_space: AddressSpace) -> u32 {
        div_ceil_u32(self.pointer_spec(addr_space).bit_width(), 8)
    }

    /// Mirrors `DataLayout::getPointerSizeInBits(AS)`.
    #[inline]
    pub fn pointer_size_in_bits(&self, addr_space: AddressSpace) -> u32 {
        self.pointer_spec(addr_space).bit_width()
    }

    /// Mirrors `DataLayout::getIndexSize(AS)`. Bytes.
    pub fn index_size(&self, addr_space: AddressSpace) -> u32 {
        div_ceil_u32(self.pointer_spec(addr_space).index_bit_width(), 8)
    }

    /// Mirrors `DataLayout::getIndexSizeInBits(AS)`.
    #[inline]
    pub fn index_size_in_bits(&self, addr_space: AddressSpace) -> u32 {
        self.pointer_spec(addr_space).index_bit_width()
    }

    /// Mirrors `DataLayout::getPointerABIAlignment(AS)`.
    #[inline]
    pub fn pointer_abi_align(&self, addr_space: AddressSpace) -> Align {
        self.pointer_spec(addr_space).abi_align()
    }

    /// Mirrors `DataLayout::getPointerPrefAlignment(AS)`.
    #[inline]
    pub fn pointer_pref_align(&self, addr_space: AddressSpace) -> Align {
        self.pointer_spec(addr_space).pref_align()
    }

    /// Mirrors `DataLayout::getPointerSpec`. Exposes the parsed
    /// pointer-layout record without exposing its representation.
    pub fn pointer_spec(&self, addr_space: AddressSpace) -> &PointerSpec {
        if addr_space != AddressSpace::DEFAULT
            && let Some(spec) = self
                .pointer_specs
                .iter()
                .find(|s| s.address_space() == addr_space)
        {
            return spec;
        }
        self.pointer_specs
            .iter()
            .find(|s| s.address_space() == 0)
            .unwrap_or_else(|| unreachable!("DataLayout always has a pointer spec for AS 0"))
    }

    // ----- Type-size / alignment accessors -----
    //
    // Each takes a [`SizedType`]: upstream's `getTypeSizeInBits`
    // (`DataLayout.h`) and `getAlignment` (`DataLayout.cpp`) assert
    // `Ty->isSized()`, and every other size query reaches one of the two, so
    // the precondition is the parameter's type. An integer, float or pointer
    // type converts into one; any other type goes through
    // `SizedType::try_from`, which answers `IrError::UnsizedType` for a type
    // with no size.

    /// The module and slot of a sized type, for the walks below.
    fn sized_parts<'t, B: ModuleBrand, C: Capability>(
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> (&'t ModuleCore, TypeSlot) {
        let ty = ty.into().as_type();
        (ty.module().core_ref(), ty.slot_trusting_same_module())
    }

    /// Bit-size of `ty` when held as an SSA value. Mirrors
    /// `DataLayout::getTypeSizeInBits` (the inline definition in
    /// `DataLayout.h`). Scalable for a type that is or holds a scalable
    /// vector; a size past `u64::MAX` saturates there, where upstream's
    /// arithmetic wraps (`docs/divergences.md` D42).
    pub fn type_size_in_bits<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> TypeSize {
        let (module, id) = Self::sized_parts(ty);
        self.type_size_in_bits_inner(module, id)
    }

    /// Mirrors `DataLayout::getTypeStoreSize`: the bytes a store of `ty`
    /// may overwrite. Saturates as [`Self::type_size_in_bits`] does.
    pub fn type_store_size<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> TypeSize {
        let (module, id) = Self::sized_parts(ty);
        self.type_store_size_inner(module, id)
    }

    /// Mirrors `DataLayout::getTypeStoreSizeInBits`.
    pub fn type_store_size_in_bits<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> TypeSize {
        let (module, id) = Self::sized_parts(ty);
        self.type_store_size_in_bits_inner(module, id)
    }

    /// Mirrors `DataLayout::typeSizeEqualsStoreSize`.
    pub fn type_size_equals_store_size<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> bool {
        let (module, id) = Self::sized_parts(ty);
        self.type_size_in_bits_inner(module, id) == self.type_store_size_in_bits_inner(module, id)
    }

    /// Mirrors `DataLayout::getTypeAllocSize`: bytes including trailing
    /// alignment padding, the offset between two adjacent array elements of
    /// `ty`. Saturates as [`Self::type_size_in_bits`] does.
    pub fn type_alloc_size<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> TypeSize {
        let (module, id) = Self::sized_parts(ty);
        self.type_alloc_size_inner(module, id)
    }

    /// Mirrors `DataLayout::getTypeAllocSizeInBits`.
    pub fn type_alloc_size_in_bits<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> TypeSize {
        self.type_alloc_size(ty).saturating_scaled(8)
    }

    /// Mirrors `DataLayout::getABITypeAlign`.
    pub fn abi_type_align<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> Align {
        let (module, id) = Self::sized_parts(ty);
        self.alignment(module, id, AlignmentKind::Abi)
    }

    /// Mirrors `DataLayout::getPrefTypeAlign`.
    pub fn pref_type_align<'t, B: ModuleBrand, C: Capability>(
        &self,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> Align {
        let (module, id) = Self::sized_parts(ty);
        self.alignment(module, id, AlignmentKind::Preferred)
    }

    /// Mirrors `DataLayout::getValueOrABITypeAlignment`: `alignment` if it is
    /// set, otherwise the ABI alignment of `ty`.
    pub fn value_or_abi_type_align<'t, B: ModuleBrand, C: Capability>(
        &self,
        alignment: MaybeAlign,
        ty: impl Into<SizedType<'t, B, C>>,
    ) -> Align {
        match alignment.align() {
            Some(align) => align,
            None => self.abi_type_align(ty),
        }
    }

    /// Mirrors `DataLayout::getABIIntegerTypeAlignment(BitWidth)`.
    pub fn abi_integer_type_align(&self, bit_width: u32) -> Align {
        self.integer_alignment(bit_width, AlignmentKind::Abi)
    }
}

/// Which of a type's two alignments a walk asks for. Upstream passes a
/// `bool abi_or_pref`; the two answers are different facts, so they get a
/// name each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlignmentKind {
    /// `getABITypeAlign`: the alignment the ABI requires.
    Abi,
    /// `getPrefTypeAlign`: the alignment the target prefers.
    Preferred,
}

impl AlignmentKind {
    /// The spec's alignment of this kind.
    fn of_primitive(self, spec: &PrimitiveSpec) -> Align {
        match self {
            Self::Abi => spec.abi_align(),
            Self::Preferred => spec.pref_align(),
        }
    }
}

// --------------------------------------------------------------------------
// Internal: the module's struct layouts
// --------------------------------------------------------------------------

/// A hasher for the small integer keys of the struct-layout maps (a
/// [`ModuleId`], a [`TypeSlot`]): one multiply per word. The keys are slots
/// the crate assigned in sequence, not values an input chooses, so the
/// collision resistance of the default SipHash buys nothing here, and the
/// lookups sit on every struct a size or alignment walk reaches.
#[derive(Default)]
struct SlotHasher(u64);

impl core::hash::Hasher for SlotHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(u64::from(*byte));
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        // Fibonacci hashing: odd multiplier, the high bits well mixed.
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type SlotMap<K, V> = HashMap<K, V, core::hash::BuildHasherDefault<SlotHasher>>;

/// One module's struct layouts under one [`DataLayout`], with the token that
/// says whether the module still exists.
struct ModuleLayouts {
    /// Dead once the module is dropped (`ModuleCore::liveness`).
    alive: Weak<()>,
    layouts: SlotMap<TypeSlot, Arc<StructLayoutInfo>>,
}

/// The struct layouts one [`DataLayout`] has computed. Mirrors
/// `DataLayout::LayoutMap` (`DataLayout.cpp`), the `StructLayoutMap` keyed by
/// `StructType *` that lets upstream lay each struct out once per layout:
/// created empty, filled by `getStructLayout`, freed with its `DataLayout`.
///
/// A `DataLayout` here is usable with any module's types, and a [`TypeSlot`]
/// names a type only within one module, so the layouts are kept per module,
/// under its [`ModuleId`] (never reused). Upstream's map holds pointers to
/// types that die with their context; here a module's entry is dropped once
/// the module is (its `liveness` token is dead): the map sweeps out dead
/// modules after as many operations as it holds modules, plus
/// [`SWEEP_FLOOR`], so a sweep costs amortised constant time per operation
/// and a dead module's layouts outlive it by a bounded number of lookups,
/// whatever the map is then asked.
///
/// A clone starts empty, as upstream's `operator=` empties the copy's map
/// (`TEST(DataLayoutTest, CopyAssignmentInvalidatesStructLayout)`). Code
/// that wants one layout's cache shares the layout itself: a module holds its
/// own in an `Arc` (`Module::data_layout_shared`), which the analyses take
/// instead of a clone. The map is no part of the layout's value:
/// [`DataLayout`]'s `==` and `Hash` leave it out. A `Mutex`, so a
/// `DataLayout` stays `Sync`.
///
/// A cached layout cannot go stale. Only a struct whose fields are all sized
/// is laid out ([`DataLayout::struct_layout`] answers `None` otherwise, and
/// every other walk runs under a [`SizedType`]), so no layout here reaches an
/// opaque struct; and a struct's body is set once
/// (`Context::set_named_struct_body` refuses a second).
///
/// Without it, a struct reached twice was laid out twice — once by
/// `alignment` and once by `type_alloc_size_inner`, for every field — so
/// `%tK = type { %t(K-1), %t(K-1) }` cost four times as much per level, and
/// a struct asked about once per instruction was laid out once per
/// instruction.
#[derive(Default)]
struct StructLayoutMap(Mutex<ModuleLayoutMaps>);

/// The fewest operations between two sweeps of dead modules.
const SWEEP_FLOOR: usize = 64;

#[derive(Default)]
struct ModuleLayoutMaps {
    modules: SlotMap<ModuleId, ModuleLayouts>,
    /// Lookups and insertions since the last sweep of dead modules.
    since_sweep: usize,
}

impl ModuleLayoutMaps {
    /// Count one operation, and drop the dead modules' entries once the
    /// count passes the number of modules held plus [`SWEEP_FLOOR`]: the
    /// sweep's cost is spread over at least as many operations as it visits.
    fn tick(&mut self) {
        self.since_sweep += 1;
        if self.since_sweep > self.modules.len() + SWEEP_FLOOR {
            self.modules
                .retain(|_, entry| entry.alive.strong_count() > 0);
            self.since_sweep = 0;
        }
    }
}

impl Clone for StructLayoutMap {
    /// Empty, as upstream's `DataLayout::operator=` leaves the copy's map.
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl StructLayoutMap {
    /// The map, locked. No lock is held across a layout, so a panic cannot
    /// leave one poisoned mid-update; a poisoned lock is read through.
    fn lock(&self) -> MutexGuard<'_, ModuleLayoutMaps> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn get(&self, module: &ModuleCore, id: TypeSlot) -> Option<Arc<StructLayoutInfo>> {
        let mut maps = self.lock();
        maps.tick();
        maps.modules
            .get(&module.id())
            .and_then(|entry| entry.layouts.get(&id))
            .cloned()
    }

    /// Keep `layout` as `module`'s struct `id`'s.
    fn insert(&self, module: &ModuleCore, id: TypeSlot, layout: Arc<StructLayoutInfo>) {
        let mut maps = self.lock();
        maps.tick();
        maps.modules
            .entry(module.id())
            .or_insert_with(|| ModuleLayouts {
                alive: module.liveness(),
                layouts: SlotMap::default(),
            })
            .layouts
            .insert(id, layout);
    }
}

impl fmt::Debug for StructLayoutMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StructLayoutMap").finish_non_exhaustive()
    }
}

// --------------------------------------------------------------------------
// Internal: alignment + size walks
// --------------------------------------------------------------------------

/// The arm of a walk under a [`SizedType`] for a type with no size. A sized
/// type reaches only sized types through its elements, fields and layout
/// type (`Type::isSizedDerivedType`, `StructType::isSized`), and every walk
/// below starts from a `SizedType` or from a field of a struct whose fields
/// [`DataLayout::struct_layout_of`] found sized, so the arm is dead by
/// construction.
fn reached_unsized() -> ! {
    unreachable!("a DataLayout walk under a SizedType reaches only sized types")
}

// The walks below are entered only from a `SizedType` (the public queries)
// or from a field of a struct whose fields are all sized
// (`struct_layout_of`), so every type they reach is sized; see
// `reached_unsized`.
impl DataLayout {
    fn type_size_in_bits_inner(&self, module: &ModuleCore, id: TypeSlot) -> TypeSize {
        // Upstream's `getTypeSizeInBits(VTy->getElementType()).getFixedValue()`:
        // an element is never a vector, so its size is fixed.
        let element_bits =
            |elem: TypeSlot| self.type_size_in_bits_inner(module, elem).known_min_value();
        match module.context().type_data(id) {
            TypeData::Pointer { addr_space } => {
                TypeSize::fixed(u64::from(self.pointer_size_in_bits(*addr_space)))
            }
            TypeData::Array { elem, n } => self
                .type_alloc_size_inner(module, *elem)
                .saturating_scaled(8)
                .saturating_scaled(*n),
            TypeData::Struct(_) => self.layout_of(module, id).size_in_bits(),
            TypeData::Integer { bits } => TypeSize::fixed(u64::from(*bits)),
            TypeData::Half | TypeData::Bfloat => TypeSize::fixed(16),
            TypeData::Float => TypeSize::fixed(32),
            TypeData::Double => TypeSize::fixed(64),
            TypeData::PpcFp128 | TypeData::Fp128 => TypeSize::fixed(128),
            TypeData::X86Fp80 => TypeSize::fixed(80),
            TypeData::X86Amx => TypeSize::fixed(8192),
            TypeData::FixedVector { elem, n } => {
                TypeSize::fixed(u64::from(n.get()).saturating_mul(element_bits(*elem)))
            }
            TypeData::ScalableVector { elem, min } => {
                TypeSize::scalable(u64::from(min.get()).saturating_mul(element_bits(*elem)))
            }
            TypeData::TargetExt(_) => match target_ext_layout_type(module, id) {
                Some(layout) => self.type_size_in_bits_inner(module, layout),
                None => reached_unsized(),
            },
            // No size: upstream's `isSized` is false for each, so a sized type
            // reaches none (a label too, though `getTypeSizeInBits` has an
            // arm for it behind its `isSized` assert).
            TypeData::Label
            | TypeData::Void
            | TypeData::Function { .. }
            | TypeData::Metadata
            | TypeData::Token
            | TypeData::WasmExnRef => reached_unsized(),
        }
    }

    fn type_alloc_size_inner(&self, module: &ModuleCore, id: TypeSlot) -> TypeSize {
        match module.context().type_data(id) {
            TypeData::Array { elem, n } => self
                .type_alloc_size_inner(module, *elem)
                .saturating_scaled(*n),
            TypeData::Struct(_) => {
                let layout = self.layout_of(module, id);
                let (size, alignment) = (layout.size_in_bytes(), layout.alignment());
                let is_packed = matches!(
                    module.context().type_data(id),
                    TypeData::Struct(s) if s.body.borrow().as_ref().is_some_and(|b| b.packed)
                );
                if is_packed {
                    size
                } else {
                    let a = std::cmp::max(self.struct_abi_align, alignment);
                    align_type_size(size, a)
                }
            }
            TypeData::Integer { bits } => {
                let bytes = TypeSize::fixed(u64::from(div_ceil_u32(*bits, 8)));
                let a = self.integer_alignment(*bits, AlignmentKind::Abi);
                align_type_size(bytes, a)
            }
            TypeData::Pointer { addr_space } => {
                let size = TypeSize::fixed(u64::from(self.pointer_size(*addr_space)));
                align_type_size(size, self.pointer_abi_align(*addr_space))
            }
            TypeData::TargetExt(_) => match target_ext_layout_type(module, id) {
                Some(layout) => self.type_alloc_size_inner(module, layout),
                None => reached_unsized(),
            },
            _ => {
                let store = self.type_store_size_inner(module, id);
                align_type_size(store, self.alignment(module, id, AlignmentKind::Abi))
            }
        }
    }

    fn type_store_size_in_bits_inner(&self, module: &ModuleCore, id: TypeSlot) -> TypeSize {
        let base_size = self.type_size_in_bits_inner(module, id);
        TypeSize::new(
            align_to_power_of_two(base_size.known_min_value(), 8),
            base_size.is_scalable(),
        )
    }

    fn type_store_size_inner(&self, module: &ModuleCore, id: TypeSlot) -> TypeSize {
        // Upstream divides the bit size by 8, rounding up. An aggregate's bit
        // size is its byte size times 8, so it is read in bytes here: from
        // 2^61 bytes up there is no bit count in a `u64`, and dividing the
        // saturated one would answer 2^61 - 1 bytes, a plausible size for a
        // type far larger (D42). A scalar or vector is at most 2^55 bits.
        match module.context().type_data(id) {
            TypeData::Struct(_) => self.layout_of(module, id).size_in_bytes(),
            TypeData::Array { elem, n } => self
                .type_alloc_size_inner(module, *elem)
                .saturating_scaled(*n),
            TypeData::TargetExt(_) => match target_ext_layout_type(module, id) {
                Some(layout) => self.type_store_size_inner(module, layout),
                None => reached_unsized(),
            },
            _ => {
                let store_size_in_bits = self.type_store_size_in_bits_inner(module, id);
                TypeSize::new(
                    store_size_in_bits.known_min_value() / 8,
                    store_size_in_bits.is_scalable(),
                )
            }
        }
    }

    fn alignment(&self, module: &ModuleCore, id: TypeSlot, kind: AlignmentKind) -> Align {
        match module.context().type_data(id) {
            TypeData::Integer { bits } => self.integer_alignment(*bits, kind),
            TypeData::Half | TypeData::Bfloat => self.float_alignment(16, kind),
            TypeData::Float => self.float_alignment(32, kind),
            TypeData::Double => self.float_alignment(64, kind),
            TypeData::Fp128 | TypeData::PpcFp128 | TypeData::X86Fp80 => {
                self.float_alignment(128, kind)
            }
            TypeData::FixedVector { .. } | TypeData::ScalableVector { .. } => {
                let bit_width = self
                    .type_size_in_bits_inner(module, id)
                    .known_min_value()
                    .try_into()
                    .unwrap_or(u32::MAX);
                match self
                    .vector_specs
                    .iter()
                    .find(|s| s.bit_width() == bit_width)
                {
                    Some(spec) => kind.of_primitive(spec),
                    None => {
                        // "It doesn't have to be based on the full size for
                        // scalable vectors": `getKnownMinValue`, in `getAlignment`.
                        let store_bytes = self.type_store_size_inner(module, id).known_min_value();
                        pow2_ceil_align(store_bytes.try_into().unwrap_or(u32::MAX))
                    }
                }
            }
            TypeData::Pointer { addr_space } => match kind {
                AlignmentKind::Abi => self.pointer_abi_align(*addr_space),
                AlignmentKind::Preferred => self.pointer_pref_align(*addr_space),
            },
            TypeData::Array { elem, .. } => self.alignment(module, *elem, kind),
            TypeData::Struct(s) => {
                let packed = s.body.borrow().as_ref().is_some_and(|b| b.packed);
                match kind {
                    // Upstream's `if (cast<StructType>(Ty)->isPacked() && abi_or_pref)`.
                    AlignmentKind::Abi if packed => Align::ONE,
                    AlignmentKind::Abi => std::cmp::max(
                        self.struct_abi_align,
                        self.layout_of(module, id).alignment(),
                    ),
                    AlignmentKind::Preferred => std::cmp::max(
                        self.struct_pref_align,
                        self.layout_of(module, id).alignment(),
                    ),
                }
            }
            TypeData::X86Amx => const_align(64),
            TypeData::TargetExt(_) => match target_ext_layout_type(module, id) {
                Some(layout) => self.alignment(module, layout, kind),
                None => reached_unsized(),
            },
            // No alignment: upstream's `getAlignment` asserts `isSized`, which
            // each of these fails.
            TypeData::Label
            | TypeData::Void
            | TypeData::Function { .. }
            | TypeData::Metadata
            | TypeData::Token
            | TypeData::WasmExnRef => reached_unsized(),
        }
    }

    /// The alignment of a floating-point type of `bit_width` bits: its spec's,
    /// or the width in bytes rounded up to a power of two when no spec names
    /// it. Mirrors the `HalfTyID ... X86_FP80TyID` arm of `getAlignment`.
    fn float_alignment(&self, bit_width: u32, kind: AlignmentKind) -> Align {
        match self.float_specs.iter().find(|s| s.bit_width() == bit_width) {
            Some(spec) => kind.of_primitive(spec),
            None => pow2_ceil_align(bit_width / 8),
        }
    }

    fn integer_alignment(&self, bit_width: u32, kind: AlignmentKind) -> Align {
        // Mirrors `DataLayout::getIntegerAlignment`. Walk forward,
        // pick the first spec >= bit_width; if none, use the last
        // (largest) spec.
        let mut chosen: Option<&PrimitiveSpec> = None;
        for spec in &self.int_specs {
            chosen = Some(spec);
            if spec.bit_width() >= bit_width {
                break;
            }
        }
        let spec = chosen
            .unwrap_or_else(|| unreachable!("DataLayout default seeds at least one int spec"));
        kind.of_primitive(spec)
    }

    /// The layout of an aggregate struct, or `None` when a field is unsized.
    /// Mirrors `DataLayout::getStructLayout`, which returns a pointer into the
    /// `DataLayout`'s cache: each struct is laid out once per `DataLayout`,
    /// and every later call shares that layout. The layout itself is
    /// `StructLayout::StructLayout` in `DataLayout.cpp`, which asks each
    /// field's `getTypeAllocSize` and `getABITypeAlign` (both asserting
    /// `isSized`), and asserts `!ST->isOpaque()` itself: those preconditions
    /// are the `None` here.
    ///
    /// The struct itself need not be sized: one mixing a scalable vector with
    /// other fields is unsized but has a layout, as it has upstream.
    ///
    /// Takes a [`StructType`], as upstream's takes a `StructType *`: the
    /// layout of any other type is not a question.
    pub fn struct_layout<Body: StructBodyState, B: ModuleBrand, C: Capability>(
        &self,
        ty: StructType<'_, Body, B, C>,
    ) -> Option<Arc<StructLayoutInfo>> {
        self.struct_layout_of(ty.as_type())
    }

    /// [`Self::struct_layout`] for a type the caller has already found to be
    /// a struct (a GEP step's struct, a `TypeData::Struct` arm), which the
    /// crate reads without re-narrowing it.
    pub(crate) fn struct_layout_of<B: ModuleBrand, C: Capability>(
        &self,
        ty: Type<'_, B, C>,
    ) -> Option<Arc<StructLayoutInfo>> {
        let module = ty.module().core_ref();
        let id = ty.slot_trusting_same_module();
        let TypeData::Struct(data) = module.context().type_data(id) else {
            unreachable!("struct_layout_of is asked only of a struct type")
        };
        let fields_sized = data
            .body
            .borrow()
            .as_ref()?
            .elements
            .iter()
            .all(|field| SizedType::try_from(Type::new(*field, ty.module())).is_ok());
        fields_sized.then(|| self.layout_of(module, id))
    }

    /// Struct `id`'s layout, from this `DataLayout`'s map or laid out and kept
    /// there. Mirrors `DataLayout::getStructLayout`'s `StructLayoutMap`
    /// lookup, and the `new StructLayout` on a miss. Every caller holds a
    /// struct whose fields are sized (see `reached_unsized`). Laid out with no
    /// lock held: the fields' own layouts are looked up and kept along the
    /// way.
    fn layout_of(&self, module: &ModuleCore, id: TypeSlot) -> Arc<StructLayoutInfo> {
        if let Some(layout) = self.struct_layouts.get(module, id) {
            return layout;
        }
        let layout = Arc::new(self.struct_layout_inner(module, id));
        self.struct_layouts.insert(module, id, Arc::clone(&layout));
        layout
    }

    fn struct_layout_inner(&self, module: &ModuleCore, id: TypeSlot) -> StructLayoutInfo {
        let TypeData::Struct(s) = module.context().type_data(id) else {
            unreachable!("struct_layout_inner is asked only of a struct type")
        };
        let body = s.body.borrow();
        // `StructLayout::StructLayout` asserts `!ST->isOpaque()`; a struct
        // whose fields are sized has a body.
        let Some(body) = body.as_ref() else {
            reached_unsized()
        };
        let elements: &[TypeSlot] = &body.elements;
        let packed = body.packed;

        let mut size = TypeSize::fixed(0);
        let mut alignment = Align::ONE;
        let mut is_padded = false;
        let mut member_offsets = Vec::with_capacity(elements.len());

        for (index, &elem) in elements.iter().enumerate() {
            if index == 0 && is_scalable(module, elem, &mut std::collections::HashSet::new()) {
                size = TypeSize::scalable(0);
            }

            let ty_align = if packed {
                Align::ONE
            } else {
                self.alignment(module, elem, AlignmentKind::Abi)
            };
            // Only a homogeneous struct of scalable vectors has a scalable
            // size, and its members need no padding.
            if !size.is_scalable() && !size.known_min_value().is_multiple_of(ty_align.value()) {
                is_padded = true;
                size = TypeSize::fixed(align_to(size.known_min_value(), ty_align));
            }
            if ty_align.value() > alignment.value() {
                alignment = ty_align;
            }
            member_offsets.push(size);
            size = size.saturating_add(self.type_alloc_size_inner(module, elem));
        }
        if !size.is_scalable() && !size.known_min_value().is_multiple_of(alignment.value()) {
            is_padded = true;
            size = TypeSize::fixed(align_to(size.known_min_value(), alignment));
        }

        StructLayoutInfo::new(size, alignment, is_padded, member_offsets)
    }

    fn set_pointer_spec(&mut self, spec: PointerSpec) {
        match self
            .pointer_specs
            .binary_search_by_key(&spec.address_space(), |s| s.address_space())
        {
            Ok(idx) => {
                self.pointer_specs[idx] = spec;
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
        match specs.binary_search_by_key(&bit_width, |s| s.bit_width()) {
            Ok(idx) => {
                specs[idx].set_alignments(abi_align, pref_align);
            }
            Err(idx) => specs.insert(idx, PrimitiveSpec::new(bit_width, abi_align, pref_align)),
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
    size: TypeSize,
    alignment: Align,
    is_padded: bool,
    member_offsets: Vec<TypeSize>,
}

impl StructLayoutInfo {
    /// Construct computed struct-layout information. `size` is in bytes.
    pub fn new<Offsets>(
        size: TypeSize,
        alignment: Align,
        is_padded: bool,
        member_offsets: Offsets,
    ) -> Self
    where
        Offsets: Into<Vec<TypeSize>>,
    {
        let member_offsets = member_offsets.into();
        Self {
            size,
            alignment,
            is_padded,
            member_offsets,
        }
    }

    /// Mirrors `StructLayout::getSizeInBytes`.
    #[inline]
    pub fn size_in_bytes(&self) -> TypeSize {
        self.size
    }
    /// Mirrors `StructLayout::getSizeInBits`.
    #[inline]
    pub fn size_in_bits(&self) -> TypeSize {
        self.size.saturating_scaled(8)
    }
    /// Mirrors `StructLayout::getAlignment`.
    #[inline]
    pub fn alignment(&self) -> Align {
        self.alignment
    }
    /// Mirrors `StructLayout::hasPadding`.
    #[inline]
    pub fn is_padded(&self) -> bool {
        self.is_padded
    }
    /// Mirrors `StructLayout::hasPadding`.
    #[inline]
    pub fn has_padding(&self) -> bool {
        self.is_padded()
    }
    /// All member byte offsets in declaration order.
    #[inline]
    pub fn member_offsets(&self) -> &[TypeSize] {
        &self.member_offsets
    }
    /// Mirrors `StructLayout::getElementOffset`.
    pub fn element_offset(&self, index: usize) -> TypeSize {
        self.member_offsets()[index]
    }
    /// Mirrors `StructLayout::getElementOffsetInBits`.
    pub fn element_offset_in_bits(&self, index: usize) -> TypeSize {
        self.member_offsets()[index].saturating_scaled(8)
    }
    /// Mirrors `StructLayout::getElementContainingOffset`. Returns the index
    /// of the field that contains the byte offset, or `None` for an
    /// out-of-range offset, and for a struct of scalable size, which upstream
    /// asserts on: a byte offset names no field of one.
    pub fn element_containing_offset(&self, byte_offset: u64) -> Option<usize> {
        if byte_offset >= self.size.fixed_value()? {
            return None;
        }
        // Multiple fields can have the same offset if any of them are zero
        // sized. For example, in { i32, [0 x i32], i32 }, searching for offset
        // 4 will stop at the i32 in the end.
        self.member_offsets()
            .partition_point(|offset| offset.known_min_value() <= byte_offset)
            .checked_sub(1)
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

impl FromStr for DataLayout {
    type Err = DataLayoutError;

    /// Delegates to [`DataLayout::parse`], which stays the named entry point
    /// (it is the mirror of `static Expected<DataLayout>
    /// DataLayout::parse(StringRef)`, and it takes `impl AsRef<str>`).
    /// `FromStr` exists so `"e-p:64:64".parse()` and `str::parse` in generic
    /// code work; the error is still [`DataLayoutError`], naming the specific
    /// parse failure rather than a bare "invalid keyword".
    #[inline]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

// --------------------------------------------------------------------------
// Parser
// --------------------------------------------------------------------------

impl DataLayout {
    /// Top-level parser. Mirrors
    /// `DataLayout::parseLayoutString(StringRef)`.
    fn parse_layout_string(&mut self, layout: &str) -> Result<(), DataLayoutError> {
        self.string_representation = layout.to_owned();
        if layout.is_empty() {
            return Ok(());
        }
        let mut non_integral_address_spaces: Vec<AddressSpace> = Vec::new();
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
            self.set_pointer_spec(base.into_non_integral_address_space(as_id));
        }
        Ok(())
    }

    /// Mirrors `DataLayout::parseSpecification`.
    fn parse_specification(
        &mut self,
        spec: &str,
        non_integral_address_spaces: &mut Vec<AddressSpace>,
        addr_space_names: &mut std::collections::HashSet<String>,
    ) -> Result<(), DataLayoutError> {
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
                    b'a' => ManglingMode::Xcoff,
                    _ => return Err(invalid("unknown mangling mode".into())),
                };
                Ok(())
            }
            other => Err(invalid(format!("unknown specifier '{other}'"))),
        }
    }

    /// Mirrors `DataLayout::parsePrimitiveSpec`.
    fn parse_primitive_spec(&mut self, spec: &str) -> Result<(), DataLayoutError> {
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
    fn parse_aggregate_spec(&mut self, spec: &str) -> Result<(), DataLayoutError> {
        let components: Vec<&str> = spec[1..].split(':').collect();
        if components.len() < 2 || components.len() > 3 {
            return Err(spec_format("a:<abi>[:<pref>]"));
        }
        // <size> component must be empty or the literal "0".
        if !components[0].is_empty() {
            let bit_width: u32 =
                to_integer(components[0]).ok_or_else(|| invalid("size must be zero".into()))?;
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
    ) -> Result<(), DataLayoutError> {
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
            (AddressSpace::DEFAULT, String::new())
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

        self.set_pointer_spec(
            PointerSpec::new(
                addr_space,
                bit_width,
                abi_align,
                pref_align,
                index_bit_width,
            )
            .with_pointer_flags(has_unstable_repr, has_external_state)
            .with_address_space_name(addr_space_name),
        );
        Ok(())
    }
}

// --------------------------------------------------------------------------
// Static helpers (mirror the file-level helpers in DataLayout.cpp)
// --------------------------------------------------------------------------

/// Mirrors `parseSize`: 24-bit non-zero unsigned integer.
fn parse_size(s: &str, name: &str) -> Result<u32, DataLayoutError> {
    if s.is_empty() {
        return Err(invalid(format!("{name} component cannot be empty")));
    }
    let v: u64 = to_integer(s)
        .ok_or_else(|| invalid(format!("{name} must be a non-zero 24-bit integer")))?;
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
fn parse_alignment(s: &str, name: &str, allow_zero: bool) -> Result<Align, DataLayoutError> {
    if s.is_empty() {
        return Err(invalid(format!(
            "{name} alignment component cannot be empty"
        )));
    }
    let v: u64 = to_integer(s)
        .ok_or_else(|| invalid(format!("{name} alignment must be a 16-bit integer")))?;
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
    if !v.is_multiple_of(8) || !is_power_of_two(v / 8) {
        return Err(invalid(format!(
            "{name} alignment must be a power of two times the byte width"
        )));
    }
    // `v / 8` just passed `is_power_of_two`, hence non-zero, and `v <= u16::MAX`
    // (checked above) keeps it far below the `1 << 63` ceiling `Align::new`
    // otherwise rejects, so its `IrError` half is unreachable here.
    Ok(Align::new(v / 8)
        .unwrap_or_else(|_| unreachable!("v / 8 is a non-zero power of two within Align's range")))
}

/// Mirrors `to_integer(Str, N, 10)`: all of `s`, in base 10.
/// `consumeUnsignedInteger` consumes digits and nothing else, where `str::parse`
/// also takes a leading `+`.
fn to_integer<N: FromStr>(s: &str) -> Option<N> {
    if s.bytes().all(|byte| byte.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

/// The digits `s` as an address space. `parseAddrSpace`'s number and its
/// `isUInt<24>` check, which [`AddressSpace::new`] is.
fn parse_addr_space_digits(s: &str) -> Result<AddressSpace, DataLayoutError> {
    to_integer(s)
        .and_then(AddressSpace::new)
        .ok_or_else(|| invalid("address space must be a 24-bit integer".into()))
}

/// Mirrors `parseAddrSpace`.
fn parse_addr_space(s: &str) -> Result<AddressSpace, DataLayoutError> {
    if s.is_empty() {
        return Err(invalid("address space component cannot be empty".into()));
    }
    parse_addr_space_digits(s)
}

/// Mirrors `parseAddrSpaceAndName`. Accepts `<digits>` or
/// `<digits>(<name>)` or `(<name>)`.
fn parse_addr_space_and_name(s: &str) -> Result<(AddressSpace, String), DataLayoutError> {
    if s.is_empty() {
        return Err(invalid("address space component cannot be empty".into()));
    }
    let mut bytes = s;
    let mut addr_space = AddressSpace::DEFAULT;
    if bytes.as_bytes()[0].is_ascii_digit() {
        // Take leading digits.
        let mut end = 0;
        while end < bytes.len() && bytes.as_bytes()[end].is_ascii_digit() {
            end += 1;
        }
        addr_space = parse_addr_space_digits(&bytes[..end])?;
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
/// target-extension type. `None` for a type that is not a target extension
/// type, and for one `getTargetTypeInfo` can only assert on — a parameter it
/// reads that the type lacks, or a layout type `IntegerType::get` or
/// `ScalableVectorType::get` would refuse (`docs/divergences.md` D22). An
/// extension upstream's table does not name gets its `void` default, as
/// upstream does. The full upstream table covers SPIR-V, AArch64
/// (`aarch64.svcount`), RISC-V (`riscv.vector.tuple`), DirectX
/// (`dx.*`), AMDGPU (`amdgcn.named.barrier`), and the test
/// extension (`llvm.test.vectorelement`).
pub(crate) fn target_ext_layout_type(module: &ModuleCore, id: TypeSlot) -> Option<TypeSlot> {
    let data = module.context().type_data(id);
    let TypeData::TargetExt(ext) = data else {
        return None;
    };
    let ctx = module.context();
    let name = ext.name.as_str();

    // SPIR-V images / signed images: opaque pointer in AS 0.
    if name == "spirv.Image" || name == "spirv.SignedImage" {
        return Some(ctx.ptr_type(AddressSpace::DEFAULT));
    }
    // SPIR-V typed reads with explicit size + alignment. Upstream asserts the
    // three integer parameters.
    if name == "spirv.Type" {
        let &[_, size, alignment] = ext.int_params.as_ref() else {
            return None;
        };
        if size > 0 && alignment > 0 {
            // `Type::getIntNTy` reaches `IntegerType::get`, which asserts
            // `MAX_INT_BITS`.
            if alignment > MAX_INT_BITS {
                return None;
            }
            // ArrayType::get(IntN, Size * 8 / Alignment), on `unsigned`s: the
            // product wraps before the division.
            let elem = ctx.int_type(alignment);
            return Some(ctx.array_type(elem, u64::from(size.wrapping_mul(8) / alignment)));
        }
        return Some(ctx.int_type(32));
    }
    // SPIR-V integral constants / literal: void.
    if name == "spirv.IntegralConstant" || name == "spirv.Literal" {
        return Some(ctx.void());
    }
    // SPIR-V padding: byte-array of N bytes. `getIntParameter(0)` asserts the
    // parameter exists.
    if name == "spirv.Padding" {
        let n = u64::from(*ext.int_params.first()?);
        return Some(ctx.array_type(ctx.int_type(8), n));
    }
    // Other SPIR-V types: opaque pointer.
    if name.starts_with("spirv.") {
        return Some(ctx.ptr_type(AddressSpace::DEFAULT));
    }
    // AArch64 svcount: <vscale x 16 x i1>.
    if name == "aarch64.svcount" {
        return Some(ctx.scalable_vector_type(ctx.int_type(1), lanes::<16>()));
    }
    // RISC-V vector tuple. Layout = <vscale x N x i8> where N is
    // computed from the tuple's first scalable-vector type parameter, which
    // upstream's `cast<ScalableVectorType>` asserts it is.
    if name == "riscv.vector.tuple" {
        let TypeData::ScalableVector { min, .. } = ctx.type_data(*ext.type_params.first()?) else {
            return None;
        };
        // Upstream uses `RISCV::RVVBytesPerBlock` (== 8) as the floor.
        const RVV_BYTES_PER_BLOCK: u32 = 8;
        let lanes = core::cmp::max(min.get(), RVV_BYTES_PER_BLOCK);
        let count_factor = *ext.int_params.first()?;
        // Upstream's `getTargetTypeInfo` passes this `unsigned` product to
        // `ScalableVectorType::get`, which asserts `MinNumElts > 0`; llvmkit
        // has no zero-lane vector, so a zero-count tuple has no layout type
        // and is unsized (`Type::isSizedDerivedType` asks the layout type).
        let total = NonZeroU32::new(lanes.wrapping_mul(count_factor))?;
        return Some(ctx.scalable_vector_type(ctx.int_type(8), total));
    }
    // DirectX padding. `getIntParameter(0)` asserts the parameter exists.
    if name == "dx.Padding" {
        let n = u64::from(*ext.int_params.first()?);
        return Some(ctx.array_type(ctx.int_type(8), n));
    }
    // Other DirectX types: opaque pointer.
    if name.starts_with("dx.") {
        return Some(ctx.ptr_type(AddressSpace::DEFAULT));
    }
    // AMDGPU named.barrier: <4 x i32>.
    if name == "amdgcn.named.barrier" {
        return Some(ctx.fixed_vector_type(ctx.int_type(32), lanes::<4>()));
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
/// `dyn_cast<PointerType>(Ty->getScalarType())`'s address space.
fn scalar_pointer_address_space<B: ModuleBrand, C: Capability>(
    ty: Type<'_, B, C>,
) -> Option<AddressSpace> {
    match ty.scalar_type().data() {
        TypeData::Pointer { addr_space } => Some(*addr_space),
        _ => None,
    }
}

fn is_power_of_two(v: u64) -> bool {
    v != 0 && (v & (v - 1)) == 0
}

#[inline]
fn div_ceil_u32(value: u32, denom: u32) -> u32 {
    value.div_ceil(denom)
}

/// Mirrors `llvm::alignTo(uint64_t, Align)` (`Support/Alignment.h`): `value`
/// rounded up to a multiple of `align`.
///
/// Saturating, as every size computation in this file is (see
/// `TypeSize::saturating_scaled`): a value within `align - 1` of `u64::MAX`
/// has no aligned `u64` above it, and answers `u64::MAX`, where upstream's
/// `(Value + Align - 1) / Align * Align` wraps to a small number. A size of
/// `u64::MAX` bytes is not a size any type can have in memory, so it reads as
/// "too large to represent", never as a plausible small size.
#[inline]
fn align_to(value: u64, align: Align) -> u64 {
    align_to_power_of_two(value, align.value())
}

/// Mirrors `llvm::alignTo(TypeSize, uint64_t)` (`Support/TypeSize.h`): the
/// quantity aligned, the scalability kept.
#[inline]
fn align_type_size(size: TypeSize, align: Align) -> TypeSize {
    TypeSize::new(align_to(size.known_min_value(), align), size.is_scalable())
}

#[inline]
fn align_to_power_of_two(value: u64, align: u64) -> u64 {
    debug_assert!(is_power_of_two(align));
    value
        .checked_add(align - 1)
        .map_or(u64::MAX, |bumped| bumped & !(align - 1))
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

fn invalid(reason: String) -> DataLayoutError {
    DataLayoutError { reason }
}

fn spec_format(form: &str) -> DataLayoutError {
    invalid(format!("malformed specification, expected {form}"))
}

#[cfg(test)]
mod tests {
    use super::{DataLayout, SWEEP_FLOOR};
    use crate::Module;

    /// The number of modules `layout`'s struct-layout map holds.
    fn modules_held(layout: &DataLayout) -> usize {
        layout.struct_layouts.lock().modules.len()
    }

    /// llvmkit-specific, no upstream counterpart: upstream's `StructLayoutMap`
    /// is keyed by `StructType *` and dies with its `DataLayout`. One
    /// `DataLayout` used with the types of module after module keeps only the
    /// live modules' layouts once it has swept: a dropped module's entry goes
    /// within `SWEEP_FLOOR` plus as many operations as modules held, even
    /// when every later operation is a hit on a live module, and a live
    /// module's entry stays although nothing asks about it during the sweep.
    /// Without the sweep, a long-lived layout grew by every module it ever
    /// measured; a sweep that also dropped live entries would leave
    /// `untouched` out, which one live module alone could not show, since
    /// the module asked about is laid out again on its next lookup.
    #[test]
    fn a_dropped_modules_layouts_leave_the_map() {
        let layout = DataLayout::default();
        for name in ["first", "second", "third"] {
            let module = Module::dynamic(name);
            let ty = module
                .struct_type([module.i32_type().as_type()])
                .expect("valid struct elements");
            assert!(layout.struct_layout(ty).is_some());
        }
        let untouched = Module::dynamic("untouched");
        let untouched_ty = untouched
            .struct_type([untouched.i64_type().as_type()])
            .expect("valid struct elements");
        assert!(layout.struct_layout(untouched_ty).is_some());
        let asked = Module::dynamic("asked");
        let ty = asked
            .struct_type([asked.i32_type().as_type()])
            .expect("valid struct elements");
        assert_eq!(
            modules_held(&layout),
            4,
            "every module measured so far is held"
        );
        for _ in 0..=(SWEEP_FLOOR + 6) {
            assert!(layout.struct_layout(ty).is_some());
        }
        assert_eq!(
            modules_held(&layout),
            2,
            "both live modules' layouts are held, and only theirs"
        );
    }

    /// Mirrors upstream's copy semantics (`DataLayout::operator=` empties the
    /// copy's map), and pins the sharing that replaces copying: a clone
    /// starts with no layouts, and the module's own layout, taken through
    /// `Module::data_layout_shared`, is the module's, cache and all.
    #[test]
    fn a_clone_starts_empty_and_the_module_layout_is_shared() {
        let module = Module::dynamic("shared");
        let ty = module
            .struct_type([module.i32_type().as_type()])
            .expect("valid struct elements");
        assert!(module.data_layout().struct_layout(ty).is_some());
        let clone = module.data_layout().clone();
        assert_eq!(modules_held(&clone), 0, "a clone starts empty");
        let shared = module.data_layout_shared();
        assert_eq!(
            modules_held(&shared),
            1,
            "the shared layout is the module's"
        );
    }
}
