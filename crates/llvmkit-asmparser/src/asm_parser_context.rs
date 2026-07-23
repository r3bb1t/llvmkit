//! Optional file-location registry produced by `LLParser`.
//!
//! Direct port of `llvm/include/llvm/AsmParser/AsmParserContext.h` and
//! `llvm/lib/AsmParser/AsmParserContext.cpp`. The parser populates one of
//! these as it walks textual IR; downstream tooling (debuggers, IDE
//! integrations, IR-level instrumentation) queries it for "what IR is at
//! this `(line, col)`?" and "where in the source is this IR construct?".
//!
//! Storage shape:
//!
//! - Forward maps record one [`FileLocRange`] per IR handle. Mirrors the
//!   `DenseMap<*, FileLocRange>` tables in the upstream implementation.
//! - Reverse lookups (`*_at`, `*_at_range`) walk a sorted vector. Upstream
//!   uses `IntervalMap` for `O(log n)` range queries; this Rust port keeps
//!   the same external semantics with a simpler `Vec` + binary-search
//!   backing because llvmkit does not yet ship an `IntervalMap` analogue.
//!   The asymptotic difference does not matter for typical IR module
//!   sizes; the map can be swapped for a tree later without changing the
//!   public API.
//!
//! Identity model: the registry keys on the erased
//! [`llvmkit_ir::Value`] view of every handle. This dodges three
//! complications that pure-typed keys would carry:
//!
//! 1. `BasicBlock<'ctx, Dyn, Terminated>` and `BasicBlock<'ctx, Dyn, Unterminated>`
//!    are distinct Rust types but refer to the same IR object — the parser
//!    inserts an unterminated block, downstream tooling may hold the
//!    terminated form.
//! 2. `InstructionView<'ctx, B>` is the copyable read-only instruction
//!    identity; storing lifecycle handles would break Doctrine D2.
//! 3. `FunctionValue<'ctx, R, B>` carries a return marker that the parser
//!    cannot pin statically.
//!
//! Each typed accessor lifts to the erased identity through `into_erased()`.

use std::collections::HashMap;

use llvmkit_ir::{
    BasicBlock, BasicBlockLabel, BlockTerminationState, Brand, Dyn, FunctionValue, InstructionView,
    ModuleBrand, ReturnMarker, Value,
};

use super::file_loc::{FileLoc, FileLocRange};

/// Errors that surface from the location registry. Mirrors the boolean
/// return of upstream's `add*Location` methods (`true` = inserted, `false`
/// = already present); we use a typed result so callers can write `?`-style
/// chains and discriminate the failure mode.
#[derive(Clone, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum LocationError {
    /// `add_*_location` was called twice for the same IR handle. Mirrors
    /// `Functions.insert(...).second == false` in upstream.
    #[error("duplicate location for the supplied IR handle")]
    DuplicateHandle,
}

/// Forward / reverse location maps for one IR-handle category, keyed on
/// the erased [`Value`] identity.
#[derive(Debug)]
struct LocMap<'ctx, B: ModuleBrand = Brand<'ctx>> {
    forward: HashMap<Value<'ctx, B>, FileLocRange>,
    /// Reverse map kept sorted by `range.start` for binary-search queries.
    reverse: Vec<(FileLocRange, Value<'ctx, B>)>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for LocMap<'ctx, B> {
    fn default() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: Vec::new(),
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> LocMap<'ctx, B> {
    fn add(&mut self, value: Value<'ctx, B>, loc: FileLocRange) -> Result<(), LocationError> {
        if self.forward.contains_key(&value) {
            return Err(LocationError::DuplicateHandle);
        }
        self.forward.insert(value, loc);
        let pos = self
            .reverse
            .binary_search_by(|(existing, _)| existing.start.cmp(&loc.start))
            .unwrap_or_else(|e| e);
        self.reverse.insert(pos, (loc, value));
        Ok(())
    }

    fn location_of(&self, value: Value<'ctx, B>) -> Option<FileLocRange> {
        self.forward.get(&value).copied()
    }

    fn handle_at(&self, loc: FileLoc) -> Option<Value<'ctx, B>> {
        let pos = self
            .reverse
            .partition_point(|(range, _)| range.start <= loc);
        if pos == 0 {
            return None;
        }
        let (range, value) = self.reverse[pos - 1];
        if range.contains_loc(loc) {
            Some(value)
        } else {
            None
        }
    }

    fn handle_at_range(&self, query: FileLocRange) -> Option<Value<'ctx, B>> {
        let pos = self
            .reverse
            .partition_point(|(range, _)| range.start <= query.start);
        if pos == 0 {
            return None;
        }
        let (range, value) = self.reverse[pos - 1];
        if range.start == query.start && range.end <= query.end {
            Some(value)
        } else {
            None
        }
    }
}

/// File-location registry for parser-produced IR.
///
/// Mirrors the three tables of upstream `AsmParserContext` (functions,
/// blocks, instructions). Lifetime brand `'ctx` ties every entry to a
/// single [`llvmkit_ir::Module`] (Doctrine D7).
#[derive(Debug)]
pub struct AsmParserContext<'ctx, B: ModuleBrand = Brand<'ctx>> {
    functions: LocMap<'ctx, B>,
    blocks: LocMap<'ctx, B>,
    instructions: LocMap<'ctx, B>,
}

impl<'ctx, B: ModuleBrand + 'ctx> Default for AsmParserContext<'ctx, B> {
    fn default() -> Self {
        Self {
            functions: LocMap::default(),
            blocks: LocMap::default(),
            instructions: LocMap::default(),
        }
    }
}

impl<'ctx, B: ModuleBrand + 'ctx> AsmParserContext<'ctx, B> {
    /// Empty registry.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    // ── Forward queries ────────────────────────────────────────────────

    /// Source range of a recorded function. Mirrors upstream
    /// `getFunctionLocation(const Function *)`.
    #[inline]
    pub fn function_location<R: ReturnMarker>(
        &self,
        f: FunctionValue<'ctx, R, B>,
    ) -> Option<FileLocRange> {
        self.functions.location_of(f.into_erased())
    }

    /// Source range of a recorded basic block. Mirrors
    /// `getBlockLocation(const BasicBlock *)`.
    #[inline]
    pub fn block_location<R: ReturnMarker, S: BlockTerminationState>(
        &self,
        b: &BasicBlock<'ctx, R, S, B>,
    ) -> Option<FileLocRange> {
        self.blocks.location_of(b.to_erased())
    }

    /// Source range of a recorded instruction. Mirrors
    /// `getInstructionLocation(const Instruction *)`.
    #[inline]
    pub fn instruction_location(&self, i: &InstructionView<'ctx, B>) -> Option<FileLocRange> {
        self.instructions.location_of(i.to_erased())
    }

    // ── Reverse queries ────────────────────────────────────────────────

    /// Function (erased identity) containing `loc`. Mirrors
    /// `getFunctionAtLocation(const FileLoc &)`.
    #[inline]
    pub fn function_at(&self, loc: FileLoc) -> Option<FunctionValue<'ctx, Dyn, B>> {
        self.functions
            .handle_at(loc)
            .and_then(|v| FunctionValue::try_from(v).ok())
    }

    /// Function whose recorded range matches `query`. Mirrors
    /// `getFunctionAtLocation(const FileLocRange &)`.
    #[inline]
    pub fn function_at_range(&self, query: FileLocRange) -> Option<FunctionValue<'ctx, Dyn, B>> {
        self.functions
            .handle_at_range(query)
            .and_then(|v| FunctionValue::try_from(v).ok())
    }

    /// Block label containing `loc`. Mirrors `getBlockAtLocation(const FileLoc &)`.
    /// The reverse lookup returns a copyable [`BasicBlockLabel`] instead of a
    /// fresh insertion-capability handle; callers that need source locations for
    /// a held block should use [`AsmParserContext::block_location`].
    #[inline]
    pub fn block_at(&self, loc: FileLoc) -> Option<BasicBlockLabel<'ctx, Dyn, B>> {
        self.blocks
            .handle_at(loc)
            .and_then(|v| BasicBlockLabel::try_from(v).ok())
    }

    /// Block label whose recorded range matches `query`.
    #[inline]
    pub fn block_at_range(&self, query: FileLocRange) -> Option<BasicBlockLabel<'ctx, Dyn, B>> {
        self.blocks
            .handle_at_range(query)
            .and_then(|v| BasicBlockLabel::try_from(v).ok())
    }

    /// Instruction (erased identity) containing `loc`. Mirrors
    /// `getInstructionAtLocation(const FileLoc &)`. Returns the erased
    /// [`Value`] so callers can re-narrow via the typed handle they
    /// already hold; reconstructing a fresh `Instruction<'ctx, state::Attached, B>` here would
    /// violate Doctrine D2 (linear-typed mutation handle).
    #[inline]
    pub fn instruction_at(&self, loc: FileLoc) -> Option<Value<'ctx, B>> {
        self.instructions.handle_at(loc)
    }

    /// Instruction whose recorded range matches `query`.
    #[inline]
    pub fn instruction_at_range(&self, query: FileLocRange) -> Option<Value<'ctx, B>> {
        self.instructions.handle_at_range(query)
    }

    // ── Insertion ──────────────────────────────────────────────────────

    /// Record `f`'s location. Mirrors `addFunctionLocation`.
    #[inline]
    pub fn add_function_location<R: ReturnMarker>(
        &mut self,
        f: FunctionValue<'ctx, R, B>,
        loc: FileLocRange,
    ) -> Result<(), LocationError> {
        self.functions.add(f.into_erased(), loc)
    }

    /// Record `b`'s location. Mirrors `addBlockLocation`.
    #[inline]
    pub fn add_block_location<R: ReturnMarker, S: BlockTerminationState>(
        &mut self,
        b: &BasicBlock<'ctx, R, S, B>,
        loc: FileLocRange,
    ) -> Result<(), LocationError> {
        self.blocks.add(b.to_erased(), loc)
    }

    /// Record `i`'s location. Mirrors `addInstructionLocation`.
    #[inline]
    pub fn add_instruction_location(
        &mut self,
        i: &InstructionView<'ctx, B>,
        loc: FileLocRange,
    ) -> Result<(), LocationError> {
        self.instructions.add(i.to_erased(), loc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ports the round-trip semantics declared by `addFunctionLocation` /
    /// `getFunctionLocation` in `lib/AsmParser/AsmParserContext.cpp`. The
    /// upstream methods return `bool` for "inserted vs already-present";
    /// the Rust analogue surfaces the same outcome via `Result`.
    #[test]
    fn locmap_round_trip() {
        // Exercises the inner table directly; the typed wrapper test waits
        // on parser integration.
        llvmkit_ir::Module::with_new("locmap_round_trip", |m| {
            let i32_ty = m.i32_type();
            let g = m
                .add_external_global("g", i32_ty.as_type())
                .expect("fresh global");

            let mut map: LocMap<'_> = LocMap::default();
            let r = FileLocRange::new(FileLoc::new(2, 0), FileLoc::new(4, 0));
            map.add(g.into_erased(), r).unwrap();
            assert_eq!(map.location_of(g.into_erased()), Some(r));
            assert_eq!(
                map.add(g.into_erased(), r),
                Err(LocationError::DuplicateHandle)
            );
        });
    }

    /// Ports the half-open semantics of `getXAtLocation(FileLoc)` in
    /// `AsmParserContext.cpp`: a query inside `[Start, End)` returns the
    /// handle, a query at `End` returns `None`.
    #[test]
    fn locmap_reverse_lookup_is_half_open() {
        llvmkit_ir::Module::with_new("locmap_reverse_lookup_is_half_open", |m| {
            let i32_ty = m.i32_type();
            let g = m
                .add_external_global("g", i32_ty.as_type())
                .expect("fresh global");

            let mut map: LocMap<'_> = LocMap::default();
            let r = FileLocRange::new(FileLoc::new(1, 0), FileLoc::new(1, 5));
            map.add(g.into_erased(), r).unwrap();

            assert_eq!(map.handle_at(FileLoc::new(1, 0)), Some(g.into_erased()));
            assert_eq!(map.handle_at(FileLoc::new(1, 4)), Some(g.into_erased()));
            assert_eq!(map.handle_at(FileLoc::new(1, 5)), None);
            assert_eq!(map.handle_at(FileLoc::new(0, 0)), None);
        });
    }

    /// Ports the range-equality semantics of
    /// `getXAtLocation(FileLocRange)`: only entries whose range *starts at*
    /// `query.start` and ends at-or-before `query.end` match.
    #[test]
    fn locmap_reverse_range_lookup() {
        llvmkit_ir::Module::with_new("locmap_reverse_range_lookup", |m| {
            let i32_ty = m.i32_type();
            let g_inner = m
                .add_external_global("g_inner", i32_ty.as_type())
                .expect("fresh global");
            let g_far = m
                .add_external_global("g_far", i32_ty.as_type())
                .expect("fresh global");

            let mut map: LocMap<'_> = LocMap::default();
            let inner = FileLocRange::new(FileLoc::new(1, 0), FileLoc::new(2, 0));
            let far = FileLocRange::new(FileLoc::new(5, 0), FileLoc::new(6, 0));
            map.add(g_inner.into_erased(), inner).unwrap();
            map.add(g_far.into_erased(), far).unwrap();

            let outer = FileLocRange::new(FileLoc::new(1, 0), FileLoc::new(3, 0));
            assert_eq!(map.handle_at_range(outer), Some(g_inner.into_erased()));

            // Mismatched start — no hit.
            let shifted = FileLocRange::new(FileLoc::new(1, 1), FileLoc::new(3, 0));
            assert_eq!(map.handle_at_range(shifted), None);
        });
    }
}
