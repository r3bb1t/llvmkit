//! Floating-point denormal handling mode.
//!
//! Mirrors the input/output pair carried by `llvm::DenormalMode` in
//! `llvm/include/llvm/IR/FPEnv.h`; analysis constant folding uses it when
//! deciding whether denormal FP constants may be flushed before or after a fold.

/// Denormal handling policy for one side of a floating-point operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DenormalModeKind {
    /// Preserve IEEE denormal values.
    Ieee,
    /// Flush denormals to signed zero.
    PreserveSign,
    /// Flush denormals to positive zero.
    PositiveZero,
    /// Runtime-selected mode; analysis must decline folds that need a choice.
    Dynamic,
}

impl DenormalModeKind {
    /// LLVM attribute spelling for this mode where one exists.
    #[inline]
    pub const fn keyword(self) -> Option<&'static str> {
        match self {
            Self::Ieee => Some("ieee"),
            Self::PreserveSign => Some("preserve-sign"),
            Self::PositiveZero => Some("positive-zero"),
            Self::Dynamic => Some("dynamic"),
        }
    }
}

fn parse_denormal_mode_kind(text: &str) -> Option<DenormalModeKind> {
    match text {
        "" | "ieee" => Some(DenormalModeKind::Ieee),
        "preserve-sign" => Some(DenormalModeKind::PreserveSign),
        "positive-zero" => Some(DenormalModeKind::PositiveZero),
        "dynamic" => Some(DenormalModeKind::Dynamic),
        _ => None,
    }
}

/// Side of a floating-point operation whose denormal mode is being queried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DenormalModeSide {
    Input,
    Output,
}

/// Input and output denormal modes for a floating-point instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DenormalMode {
    input: DenormalModeKind,
    output: DenormalModeKind,
}

impl DenormalMode {
    /// Construct an input/output denormal mode pair.
    #[inline]
    pub const fn new(input: DenormalModeKind, output: DenormalModeKind) -> Self {
        Self { input, output }
    }

    /// Fully IEEE mode.
    #[inline]
    pub const fn ieee() -> Self {
        Self::new(DenormalModeKind::Ieee, DenormalModeKind::Ieee)
    }

    /// Fully dynamic mode. Mirrors `DenormalMode::getDynamic()`.
    #[inline]
    pub const fn dynamic() -> Self {
        Self::new(DenormalModeKind::Dynamic, DenormalModeKind::Dynamic)
    }

    /// Input-side mode.
    #[inline]
    pub const fn input(self) -> DenormalModeKind {
        self.input
    }

    /// Output-side mode.
    #[inline]
    pub const fn output(self) -> DenormalModeKind {
        self.output
    }

    /// Mode selected for an input or output constant.
    #[inline]
    pub const fn for_side(self, side: DenormalModeSide) -> DenormalModeKind {
        match side {
            DenormalModeSide::Input => self.input,
            DenormalModeSide::Output => self.output,
        }
    }

    /// Parse a `denormal-fp-math` attribute value.
    ///
    /// Mirrors `parseDenormalFPAttribute` in
    /// `llvm/include/llvm/ADT/FloatingPointMode.h`: the textual order is
    /// `output,input`, and the legacy single-component form applies to both
    /// sides.
    pub fn from_attribute_value(text: &str) -> Option<Self> {
        let (output, input) = text.split_once(',').unwrap_or((text, ""));
        let output = parse_denormal_mode_kind(output)?;
        let input = if input.is_empty() {
            output
        } else {
            parse_denormal_mode_kind(input)?
        };
        Some(Self::new(input, output))
    }
}

impl Default for DenormalMode {
    #[inline]
    fn default() -> Self {
        Self::ieee()
    }
}
