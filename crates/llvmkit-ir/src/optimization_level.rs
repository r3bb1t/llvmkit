//! Upstream-shaped optimization-level data for future pass-pipeline recipes.
//!
//! Mirrors the public data surface of LLVM's `OptimizationLevel` and LTO phase
//! enums without constructing or running optimization passes.

use core::fmt;
use core::str::FromStr;

use super::error::{IrError, IrResult};

/// LLVM's built-in high-level optimization levels.
///
/// This is a closed enum instead of LLVM's `(SpeedLevel, SizeLevel)` pair so
/// invalid numeric combinations are unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OptimizationLevel {
    /// Disable as many optimizations as possible.
    O0,
    /// Optimize quickly without destroying debuggability.
    O1,
    /// LLVM's default optimization level.
    #[default]
    O2,
    /// Optimize for fast execution as much as possible.
    O3,
    /// Optimize for small code size while retaining reasonable speed.
    Os,
    /// Optimize for code size at any cost.
    Oz,
}

impl OptimizationLevel {
    /// Parses LLVM's exact optimization-level spellings.
    ///
    /// Mirrors `PassBuilder.cpp::parseOptLevel`.
    pub fn parse_name(name: &str) -> IrResult<Self> {
        match name {
            "O0" => Ok(Self::O0),
            "O1" => Ok(Self::O1),
            "O2" => Ok(Self::O2),
            "O3" => Ok(Self::O3),
            "Os" => Ok(Self::Os),
            "Oz" => Ok(Self::Oz),
            _ => Err(IrError::InvalidOptimizationLevel {
                level: name.to_owned(),
            }),
        }
    }

    /// Upstream speed level from `OptimizationLevel.cpp`.
    pub const fn speed_level(self) -> u8 {
        match self {
            Self::O0 => 0,
            Self::O1 => 1,
            Self::O2 | Self::Os | Self::Oz => 2,
            Self::O3 => 3,
        }
    }

    /// Upstream size level from `OptimizationLevel.cpp`.
    pub const fn size_level(self) -> u8 {
        match self {
            Self::O0 | Self::O1 | Self::O2 | Self::O3 => 0,
            Self::Os => 1,
            Self::Oz => 2,
        }
    }

    /// Whether the level optimizes primarily for speed.
    pub const fn is_optimizing_for_speed(self) -> bool {
        self.size_level() == 0 && self.speed_level() > 0
    }

    /// Whether the level optimizes primarily for size.
    pub const fn is_optimizing_for_size(self) -> bool {
        self.size_level() > 0
    }
}

impl FromStr for OptimizationLevel {
    type Err = IrError;

    fn from_str(name: &str) -> IrResult<Self> {
        Self::parse_name(name)
    }
}

impl fmt::Display for OptimizationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::O0 => "O0",
            Self::O1 => "O1",
            Self::O2 => "O2",
            Self::O3 => "O3",
            Self::Os => "Os",
            Self::Oz => "Oz",
        })
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Sealed compile-time marker for a built-in optimization level.
pub trait OptimizationLevelMarker: sealed::Sealed + Copy + 'static {
    /// The runtime level represented by this marker.
    const LEVEL: OptimizationLevel;
}

/// Marker for [`OptimizationLevel::O0`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptLevelO0;

/// Marker for [`OptimizationLevel::O1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptLevelO1;

/// Marker for [`OptimizationLevel::O2`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptLevelO2;

/// Marker for [`OptimizationLevel::O3`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptLevelO3;

/// Marker for [`OptimizationLevel::Os`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptLevelOs;

/// Marker for [`OptimizationLevel::Oz`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OptLevelOz;

impl sealed::Sealed for OptLevelO0 {}
impl sealed::Sealed for OptLevelO1 {}
impl sealed::Sealed for OptLevelO2 {}
impl sealed::Sealed for OptLevelO3 {}
impl sealed::Sealed for OptLevelOs {}
impl sealed::Sealed for OptLevelOz {}

impl OptimizationLevelMarker for OptLevelO0 {
    const LEVEL: OptimizationLevel = OptimizationLevel::O0;
}
impl OptimizationLevelMarker for OptLevelO1 {
    const LEVEL: OptimizationLevel = OptimizationLevel::O1;
}
impl OptimizationLevelMarker for OptLevelO2 {
    const LEVEL: OptimizationLevel = OptimizationLevel::O2;
}
impl OptimizationLevelMarker for OptLevelO3 {
    const LEVEL: OptimizationLevel = OptimizationLevel::O3;
}
impl OptimizationLevelMarker for OptLevelOs {
    const LEVEL: OptimizationLevel = OptimizationLevel::Os;
}
impl OptimizationLevelMarker for OptLevelOz {
    const LEVEL: OptimizationLevel = OptimizationLevel::Oz;
}

/// LLVM full LTO / ThinLTO optimization phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ThinOrFullLtoPhase {
    /// No LTO/ThinLTO behavior needed.
    #[default]
    None,
    /// ThinLTO prelink summary phase.
    ThinLtoPreLink,
    /// ThinLTO postlink backend phase.
    ThinLtoPostLink,
    /// Full LTO prelink phase.
    FullLtoPreLink,
    /// Full LTO postlink backend phase.
    FullLtoPostLink,
}

impl ThinOrFullLtoPhase {
    /// Whether this phase is a ThinLTO or full-LTO pre-link phase.
    pub const fn is_lto_pre_link(self) -> bool {
        matches!(self, Self::ThinLtoPreLink | Self::FullLtoPreLink)
    }

    /// Whether this phase is a ThinLTO or full-LTO post-link phase.
    pub const fn is_lto_post_link(self) -> bool {
        matches!(self, Self::ThinLtoPostLink | Self::FullLtoPostLink)
    }
}
