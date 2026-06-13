//! Module summary index parser and display support.
//!
//! Mirrors the textual summary-index grammar in LLVM 22.1.4
//! `llvm/lib/AsmParser/LLParser.cpp` (`parseSummaryEntry`, `parseModuleEntry`,
//! `parseGVEntry`, and the basic function/variable/alias summary forms).

use std::fmt;

use llvmkit_support::Spanned;

use crate::ll_lexer::{LexError, Lexer};
use crate::ll_token::{IntLit, Keyword, Sign, Token};
use crate::parse_error::{DiagLoc, ParseError, ParseResult};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleSummaryIndex {
    pub modules: Vec<ModuleEntry>,
    pub type_ids: Vec<TypeIdEntry>,
    pub globals: Vec<GvEntry>,
    pub flags: Option<SummaryFlags>,
    pub block_count: Option<u64>,
}

impl ModuleSummaryIndex {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
            && self.type_ids.is_empty()
            && self.globals.is_empty()
            && self.flags.is_none()
            && self.block_count.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleEntry {
    pub id: u32,
    pub path: String,
    pub hash: Option<[u32; 5]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeIdEntry {
    pub id: u32,
    pub name: String,
    pub summary: TypeIdSummary,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeIdSummary {
    pub type_tests: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GvEntry {
    pub id: u32,
    pub name: Option<String>,
    pub guid: Option<u64>,
    pub summaries: Vec<GlobalValueSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalValueSummary {
    Function(FunctionSummary),
    Variable(VariableSummary),
    Alias(AliasSummary),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSummary {
    pub module: u32,
    pub flags: GvFlags,
    pub insts: u32,
    pub func_flags: Option<FunctionFlags>,
    pub calls: Vec<CallEdge>,
    pub refs: Vec<GvReference>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariableSummary {
    pub module: u32,
    pub flags: GvFlags,
    pub var_flags: GVarFlags,
    pub refs: Vec<GvReference>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasSummary {
    pub module: u32,
    pub flags: GvFlags,
    pub aliasee: Option<GvReference>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryFlags {
    pub raw: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GvFlags {
    pub linkage: SummaryLinkage,
    pub visibility: SummaryVisibility,
    pub not_eligible_to_import: bool,
    pub live: bool,
    pub dso_local: bool,
    pub can_auto_hide: bool,
    pub import_type: SummaryImportType,
}

impl Default for GvFlags {
    fn default() -> Self {
        Self {
            linkage: SummaryLinkage::External,
            visibility: SummaryVisibility::Default,
            not_eligible_to_import: false,
            live: false,
            dso_local: false,
            can_auto_hide: false,
            import_type: SummaryImportType::Definition,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryLinkage {
    Private,
    Internal,
    AvailableExternally,
    LinkOnce,
    LinkOnceOdr,
    Weak,
    WeakOdr,
    Appending,
    Common,
    ExternWeak,
    External,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryVisibility {
    Default,
    Hidden,
    Protected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryImportType {
    Definition,
    Declaration,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FunctionFlags {
    pub read_none: bool,
    pub read_only: bool,
    pub no_recurse: bool,
    pub return_does_not_alias: bool,
    pub no_inline: bool,
    pub always_inline: bool,
    pub no_unwind: bool,
    pub may_throw: bool,
    pub has_unknown_call: bool,
    pub must_be_unreachable: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GVarFlags {
    pub read_only: bool,
    pub write_only: bool,
    pub constant: bool,
    pub vcall_visibility: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallEdge {
    pub callee: GvReference,
    pub hotness: SummaryHotness,
    pub relbf: Option<u32>,
    pub tail: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GvReference {
    pub id: u32,
    pub read_only: bool,
    pub write_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryHotness {
    Unknown,
    Cold,
    None,
    Hot,
    Critical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeTestResolutionKind {
    Unsat,
    ByteArray,
    Inline,
    Single,
    AllOnes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WholeProgramDevirtResolutionKind {
    Indir,
    SingleImpl,
    BranchFunnel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocType {
    NotCold,
    Cold,
    ColdWithType,
}

pub fn parse_summary_index(src: &[u8]) -> ParseResult<ModuleSummaryIndex> {
    SummaryParser::new(src)?.parse_index()
}

struct SummaryParser<'src> {
    lex: Lexer<'src>,
    current: Spanned<Token<'src>>,
}

impl<'src> SummaryParser<'src> {
    fn new(src: &'src [u8]) -> ParseResult<Self> {
        let mut lex = Lexer::new(src);
        lex.ignore_colon_in_idents = true;
        let current = lex.next_token().map_err(map_lex_error)?;
        Ok(Self { lex, current })
    }

    fn parse_index(mut self) -> ParseResult<ModuleSummaryIndex> {
        let mut index = ModuleSummaryIndex::default();
        while !matches!(&self.current.value, Token::Eof) {
            self.parse_summary_entry(&mut index)?;
        }
        Ok(index)
    }

    fn parse_summary_entry(&mut self, index: &mut ModuleSummaryIndex) -> ParseResult<()> {
        let id = match &self.current.value {
            Token::SummaryId(id) => *id,
            _ => return Err(self.expected("summary id")),
        };
        self.bump()?;
        self.expect(Token::Equal, "'=' here")?;
        match &self.current.value {
            Token::Kw(Keyword::Module) => index.modules.push(self.parse_module_entry(id)?),
            Token::Kw(Keyword::Gv) => index.globals.push(self.parse_gv_entry(id)?),
            Token::Kw(Keyword::Typeid) => index.type_ids.push(self.parse_type_id_entry(id)?),
            Token::Kw(Keyword::Flags) => index.flags = Some(self.parse_summary_flags()?),
            Token::Kw(Keyword::Blockcount) => index.block_count = Some(self.parse_block_count()?),
            _ => return Err(self.expected("summary kind")),
        }
        Ok(())
    }

    fn parse_module_entry(&mut self, id: u32) -> ParseResult<ModuleEntry> {
        self.expect_keyword(Keyword::Module, "'module'")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Path, "'path' here")?;
        self.expect(Token::Colon, "':' here")?;
        let path = self.parse_string("module path")?;
        let mut hash = None;
        if self.eat(Token::Comma)? {
            self.expect_keyword(Keyword::Hash_, "'hash' here")?;
            self.expect(Token::Colon, "':' here")?;
            self.expect(Token::LParen, "'(' here")?;
            let h0 = self.parse_uint32("hash word")?;
            self.expect(Token::Comma, "',' here")?;
            let h1 = self.parse_uint32("hash word")?;
            self.expect(Token::Comma, "',' here")?;
            let h2 = self.parse_uint32("hash word")?;
            self.expect(Token::Comma, "',' here")?;
            let h3 = self.parse_uint32("hash word")?;
            self.expect(Token::Comma, "',' here")?;
            let h4 = self.parse_uint32("hash word")?;
            self.expect(Token::RParen, "')' here")?;
            hash = Some([h0, h1, h2, h3, h4]);
        }
        self.expect(Token::RParen, "')' here")?;
        Ok(ModuleEntry { id, path, hash })
    }

    fn parse_type_id_entry(&mut self, id: u32) -> ParseResult<TypeIdEntry> {
        self.expect_keyword(Keyword::Typeid, "'typeid'")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        self.expect_keyword(Keyword::Name, "'name' here")?;
        self.expect(Token::Colon, "':' here")?;
        let name = self.parse_string("type id name")?;
        while self.eat(Token::Comma)? {
            self.skip_balanced_field()?;
        }
        self.expect(Token::RParen, "')' here")?;
        Ok(TypeIdEntry {
            id,
            name,
            summary: TypeIdSummary::default(),
        })
    }

    fn parse_summary_flags(&mut self) -> ParseResult<SummaryFlags> {
        self.expect_keyword(Keyword::Flags, "'flags'")?;
        self.expect(Token::Colon, "':' here")?;
        Ok(SummaryFlags {
            raw: self.parse_uint64("summary flags")?,
        })
    }

    fn parse_block_count(&mut self) -> ParseResult<u64> {
        self.expect_keyword(Keyword::Blockcount, "'blockcount'")?;
        self.expect(Token::Colon, "':' here")?;
        self.parse_uint64("block count")
    }

    fn parse_gv_entry(&mut self, id: u32) -> ParseResult<GvEntry> {
        self.expect_keyword(Keyword::Gv, "'gv'")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        let (name, guid) = match &self.current.value {
            Token::Kw(Keyword::Name) => {
                self.bump()?;
                self.expect(Token::Colon, "':' here")?;
                (Some(self.parse_string("global value name")?), None)
            }
            Token::Kw(Keyword::Guid) => {
                self.bump()?;
                self.expect(Token::Colon, "':' here")?;
                (None, Some(self.parse_uint64("global value guid")?))
            }
            _ => return Err(self.expected("name or guid tag")),
        };
        let mut summaries = Vec::new();
        if self.eat(Token::Comma)? {
            self.expect_keyword(Keyword::Summaries, "'summaries' here")?;
            self.expect(Token::Colon, "':' here")?;
            self.expect(Token::LParen, "'(' here")?;
            loop {
                summaries.push(match &self.current.value {
                    Token::Kw(Keyword::Function) => {
                        GlobalValueSummary::Function(self.parse_function_summary()?)
                    }
                    Token::Kw(Keyword::Variable) => {
                        GlobalValueSummary::Variable(self.parse_variable_summary()?)
                    }
                    Token::Kw(Keyword::Alias) => {
                        GlobalValueSummary::Alias(self.parse_alias_summary()?)
                    }
                    _ => return Err(self.expected("summary type")),
                });
                if !self.eat(Token::Comma)? {
                    break;
                }
            }
            self.expect(Token::RParen, "')' here")?;
        }
        self.expect(Token::RParen, "')' here")?;
        Ok(GvEntry {
            id,
            name,
            guid,
            summaries,
        })
    }

    fn parse_function_summary(&mut self) -> ParseResult<FunctionSummary> {
        self.expect_keyword(Keyword::Function, "'function'")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        let module = self.parse_module_reference()?;
        self.expect(Token::Comma, "',' here")?;
        let flags = self.parse_gv_flags()?;
        self.expect(Token::Comma, "',' here")?;
        self.expect_keyword(Keyword::Insts, "'insts' here")?;
        self.expect(Token::Colon, "':' here")?;
        let insts = self.parse_uint32("instruction count")?;
        let mut func_flags = None;
        let mut calls = Vec::new();
        let mut refs = Vec::new();
        while self.eat(Token::Comma)? {
            match &self.current.value {
                Token::Kw(Keyword::FuncFlags) => func_flags = Some(self.parse_func_flags()?),
                Token::Kw(Keyword::Calls) => calls = self.parse_calls()?,
                Token::Kw(Keyword::Refs) => refs = self.parse_refs()?,
                _ => return Err(self.expected("optional function summary field")),
            }
        }
        self.expect(Token::RParen, "')' here")?;
        Ok(FunctionSummary {
            module,
            flags,
            insts,
            func_flags,
            calls,
            refs,
        })
    }

    fn parse_variable_summary(&mut self) -> ParseResult<VariableSummary> {
        self.expect_keyword(Keyword::Variable, "'variable'")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        let module = self.parse_module_reference()?;
        self.expect(Token::Comma, "',' here")?;
        let flags = self.parse_gv_flags()?;
        self.expect(Token::Comma, "',' here")?;
        let var_flags = self.parse_var_flags()?;
        let mut refs = Vec::new();
        while self.eat(Token::Comma)? {
            match &self.current.value {
                Token::Kw(Keyword::Refs) => refs = self.parse_refs()?,
                _ => return Err(self.expected("optional variable summary field")),
            }
        }
        self.expect(Token::RParen, "')' here")?;
        Ok(VariableSummary {
            module,
            flags,
            var_flags,
            refs,
        })
    }

    fn parse_alias_summary(&mut self) -> ParseResult<AliasSummary> {
        self.expect_keyword(Keyword::Alias, "'alias'")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        let module = self.parse_module_reference()?;
        self.expect(Token::Comma, "',' here")?;
        let flags = self.parse_gv_flags()?;
        self.expect(Token::Comma, "',' here")?;
        self.expect_keyword(Keyword::Aliasee, "'aliasee' here")?;
        self.expect(Token::Colon, "':' here")?;
        let aliasee = if self.eat_keyword(Keyword::Null)? {
            None
        } else {
            Some(self.parse_gv_reference()?)
        };
        self.expect(Token::RParen, "')' here")?;
        Ok(AliasSummary {
            module,
            flags,
            aliasee,
        })
    }

    fn parse_module_reference(&mut self) -> ParseResult<u32> {
        self.expect_keyword(Keyword::Module, "'module' here")?;
        self.expect(Token::Colon, "':' here")?;
        match &self.current.value {
            Token::SummaryId(id) => {
                let id = *id;
                self.bump()?;
                Ok(id)
            }
            _ => Err(self.expected("module ID")),
        }
    }

    fn parse_gv_reference(&mut self) -> ParseResult<GvReference> {
        let read_only = self.eat_keyword(Keyword::Readonly)?;
        let write_only = if read_only {
            false
        } else {
            self.eat_keyword(Keyword::Writeonly)?
        };
        match &self.current.value {
            Token::SummaryId(id) => {
                let id = *id;
                self.bump()?;
                Ok(GvReference {
                    id,
                    read_only,
                    write_only,
                })
            }
            _ => Err(self.expected("GV ID")),
        }
    }

    fn parse_gv_flags(&mut self) -> ParseResult<GvFlags> {
        self.expect_keyword(Keyword::Flags, "'flags' here")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        let mut flags = GvFlags::default();
        loop {
            match &self.current.value {
                Token::Kw(Keyword::Linkage) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.linkage = self.parse_linkage()?;
                }
                Token::Kw(Keyword::Visibility) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.visibility = self.parse_visibility()?;
                }
                Token::Kw(Keyword::NotEligibleToImport) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.not_eligible_to_import = self.parse_flag()?;
                }
                Token::Kw(Keyword::Live) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.live = self.parse_flag()?;
                }
                Token::Kw(Keyword::DsoLocal_) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.dso_local = self.parse_flag()?;
                }
                Token::Kw(Keyword::CanAutoHide) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.can_auto_hide = self.parse_flag()?;
                }
                Token::Kw(Keyword::ImportType) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.import_type = self.parse_import_type()?;
                }
                _ => return Err(self.expected("gv flag type")),
            }
            if !self.eat(Token::Comma)? {
                break;
            }
        }
        self.expect(Token::RParen, "')' here")?;
        Ok(flags)
    }

    fn parse_func_flags(&mut self) -> ParseResult<FunctionFlags> {
        self.expect_keyword(Keyword::FuncFlags, "'funcFlags'")?;
        self.expect(Token::Colon, "':' in funcFlags")?;
        self.expect(Token::LParen, "'(' in funcFlags")?;
        let mut flags = FunctionFlags::default();
        loop {
            let slot = match &self.current.value {
                Token::Kw(Keyword::ReadNone) => &mut flags.read_none,
                Token::Kw(Keyword::ReadOnly) => &mut flags.read_only,
                Token::Kw(Keyword::NoRecurse) => &mut flags.no_recurse,
                Token::Kw(Keyword::ReturnDoesNotAlias) => &mut flags.return_does_not_alias,
                Token::Kw(Keyword::NoInline) => &mut flags.no_inline,
                Token::Kw(Keyword::AlwaysInline) => &mut flags.always_inline,
                Token::Kw(Keyword::NoUnwind) => &mut flags.no_unwind,
                Token::Kw(Keyword::MayThrow) => &mut flags.may_throw,
                Token::Kw(Keyword::HasUnknownCall) => &mut flags.has_unknown_call,
                Token::Kw(Keyword::MustBeUnreachable) => &mut flags.must_be_unreachable,
                _ => return Err(self.expected("function flag type")),
            };
            self.bump()?;
            self.expect(Token::Colon, "':'")?;
            *slot = self.parse_flag()?;
            if !self.eat(Token::Comma)? {
                break;
            }
        }
        self.expect(Token::RParen, "')' in funcFlags")?;
        Ok(flags)
    }

    fn parse_var_flags(&mut self) -> ParseResult<GVarFlags> {
        self.expect_keyword(Keyword::VarFlags, "'varFlags'")?;
        self.expect(Token::Colon, "':' here")?;
        self.expect(Token::LParen, "'(' here")?;
        let mut flags = GVarFlags::default();
        loop {
            match &self.current.value {
                Token::Kw(Keyword::Readonly) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.read_only = self.parse_flag()?;
                }
                Token::Kw(Keyword::Writeonly) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.write_only = self.parse_flag()?;
                }
                Token::Kw(Keyword::Constant) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.constant = self.parse_flag()?;
                }
                Token::Kw(Keyword::VcallVisibility) => {
                    self.bump()?;
                    self.expect(Token::Colon, "':'")?;
                    flags.vcall_visibility = self.parse_uint32("vcall visibility")?;
                }
                _ => return Err(self.expected("gvar flag type")),
            }
            if !self.eat(Token::Comma)? {
                break;
            }
        }
        self.expect(Token::RParen, "')' here")?;
        Ok(flags)
    }

    fn parse_calls(&mut self) -> ParseResult<Vec<CallEdge>> {
        self.expect_keyword(Keyword::Calls, "'calls'")?;
        self.expect(Token::Colon, "':' in calls")?;
        self.expect(Token::LParen, "'(' in calls")?;
        let mut calls = Vec::new();
        if !matches!(&self.current.value, Token::RParen) {
            loop {
                self.expect(Token::LParen, "'(' in call")?;
                self.expect_keyword(Keyword::Callee, "'callee' in call")?;
                self.expect(Token::Colon, "':' in call")?;
                let callee = self.parse_gv_reference()?;
                let mut hotness = SummaryHotness::Unknown;
                let mut relbf = None;
                let mut tail = false;
                while self.eat(Token::Comma)? {
                    match &self.current.value {
                        Token::Kw(Keyword::Hotness) => {
                            self.bump()?;
                            self.expect(Token::Colon, "':' in hotness")?;
                            hotness = self.parse_hotness()?;
                        }
                        Token::Kw(Keyword::Relbf) => {
                            self.bump()?;
                            self.expect(Token::Colon, "':' in relbf")?;
                            relbf = Some(self.parse_uint32("relative block frequency")?);
                        }
                        Token::Kw(Keyword::Tail) => {
                            self.bump()?;
                            self.expect(Token::Colon, "':' in tail")?;
                            tail = self.parse_flag()?;
                        }
                        _ => return Err(self.expected("call edge field")),
                    }
                }
                self.expect(Token::RParen, "')' in call")?;
                calls.push(CallEdge {
                    callee,
                    hotness,
                    relbf,
                    tail,
                });
                if !self.eat(Token::Comma)? {
                    break;
                }
            }
        }
        self.expect(Token::RParen, "')' in calls")?;
        Ok(calls)
    }

    fn parse_refs(&mut self) -> ParseResult<Vec<GvReference>> {
        self.expect_keyword(Keyword::Refs, "'refs'")?;
        self.expect(Token::Colon, "':' in refs")?;
        self.expect(Token::LParen, "'(' in refs")?;
        let mut refs = Vec::new();
        if !matches!(&self.current.value, Token::RParen) {
            loop {
                refs.push(self.parse_gv_reference()?);
                if !self.eat(Token::Comma)? {
                    break;
                }
            }
        }
        self.expect(Token::RParen, "')' in refs")?;
        Ok(refs)
    }

    fn parse_linkage(&mut self) -> ParseResult<SummaryLinkage> {
        let linkage = match &self.current.value {
            Token::Kw(Keyword::Private) => SummaryLinkage::Private,
            Token::Kw(Keyword::Internal) => SummaryLinkage::Internal,
            Token::Kw(Keyword::AvailableExternally) => SummaryLinkage::AvailableExternally,
            Token::Kw(Keyword::Linkonce) => SummaryLinkage::LinkOnce,
            Token::Kw(Keyword::LinkonceOdr) => SummaryLinkage::LinkOnceOdr,
            Token::Kw(Keyword::Weak) => SummaryLinkage::Weak,
            Token::Kw(Keyword::WeakOdr) => SummaryLinkage::WeakOdr,
            Token::Kw(Keyword::Appending) => SummaryLinkage::Appending,
            Token::Kw(Keyword::Common) => SummaryLinkage::Common,
            Token::Kw(Keyword::ExternWeak) => SummaryLinkage::ExternWeak,
            Token::Kw(Keyword::External) => SummaryLinkage::External,
            _ => return Err(self.expected("linkage")),
        };
        self.bump()?;
        Ok(linkage)
    }

    fn parse_visibility(&mut self) -> ParseResult<SummaryVisibility> {
        let visibility = match &self.current.value {
            Token::Kw(Keyword::Default) => SummaryVisibility::Default,
            Token::Kw(Keyword::Hidden) => SummaryVisibility::Hidden,
            Token::Kw(Keyword::Protected) => SummaryVisibility::Protected,
            _ => return Err(self.expected("visibility")),
        };
        self.bump()?;
        Ok(visibility)
    }

    fn parse_import_type(&mut self) -> ParseResult<SummaryImportType> {
        let import_type = match &self.current.value {
            Token::Kw(Keyword::Definition) => SummaryImportType::Definition,
            Token::Kw(Keyword::Declaration) => SummaryImportType::Declaration,
            _ => return Err(self.expected("import type")),
        };
        self.bump()?;
        Ok(import_type)
    }

    fn parse_hotness(&mut self) -> ParseResult<SummaryHotness> {
        let hotness = match &self.current.value {
            Token::Kw(Keyword::Unknown) => SummaryHotness::Unknown,
            Token::Kw(Keyword::Cold) => SummaryHotness::Cold,
            Token::Kw(Keyword::None) => SummaryHotness::None,
            Token::Kw(Keyword::Hot) => SummaryHotness::Hot,
            Token::Kw(Keyword::Critical) => SummaryHotness::Critical,
            _ => return Err(self.expected("hotness")),
        };
        self.bump()?;
        Ok(hotness)
    }

    fn skip_balanced_field(&mut self) -> ParseResult<()> {
        let mut depth = 0u32;
        loop {
            match &self.current.value {
                Token::Eof => return Err(self.expected("summary field")),
                Token::LParen | Token::LBrace | Token::LSquare | Token::Less => {
                    depth = depth.saturating_add(1);
                    self.bump()?;
                }
                Token::RParen | Token::RBrace | Token::RSquare | Token::Greater => {
                    if depth == 0 {
                        return Ok(());
                    }
                    depth -= 1;
                    self.bump()?;
                }
                Token::Comma if depth == 0 => return Ok(()),
                _ => self.bump()?,
            }
        }
    }

    fn parse_flag(&mut self) -> ParseResult<bool> {
        match self.parse_uint64("flag")? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ParseError::Expected {
                expected: "flag 0 or 1".into(),
                loc: DiagLoc::span(self.current.span),
            }),
        }
    }

    fn parse_uint32(&mut self, expected: &str) -> ParseResult<u32> {
        let n = self.parse_uint64(expected)?;
        u32::try_from(n).map_err(|_| ParseError::Expected {
            expected: expected.into(),
            loc: DiagLoc::span(self.current.span),
        })
    }

    fn parse_uint64(&mut self, expected: &str) -> ParseResult<u64> {
        let (sign, digits) = match &self.current.value {
            Token::IntegerLit(IntLit { sign, digits, .. }) => (*sign, (*digits).to_owned()),
            _ => return Err(self.expected(expected)),
        };
        if sign != Sign::Pos {
            return Err(self.expected(expected));
        }
        self.bump()?;
        digits.parse::<u64>().map_err(|_| ParseError::Expected {
            expected: expected.into(),
            loc: DiagLoc::span(self.current.span),
        })
    }

    fn parse_string(&mut self, expected: &str) -> ParseResult<String> {
        let s = match &self.current.value {
            Token::StringConstant(bytes) => std::str::from_utf8(bytes.as_ref())
                .map_err(|_| self.expected(expected))?
                .to_owned(),
            _ => return Err(self.expected(expected)),
        };
        self.bump()?;
        Ok(s)
    }

    fn expect_keyword(&mut self, kw: Keyword, expected: &str) -> ParseResult<()> {
        match &self.current.value {
            Token::Kw(k) if *k == kw => {
                self.bump()?;
                Ok(())
            }
            _ => Err(self.expected(expected)),
        }
    }

    fn eat_keyword(&mut self, kw: Keyword) -> ParseResult<bool> {
        match &self.current.value {
            Token::Kw(k) if *k == kw => {
                self.bump()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn expect(&mut self, tok: Token<'static>, expected: &str) -> ParseResult<()> {
        if same_token_kind(&self.current.value, &tok) {
            self.bump()?;
            Ok(())
        } else {
            Err(self.expected(expected))
        }
    }

    fn eat(&mut self, tok: Token<'static>) -> ParseResult<bool> {
        if same_token_kind(&self.current.value, &tok) {
            self.bump()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn bump(&mut self) -> ParseResult<()> {
        self.current = self.lex.next_token().map_err(map_lex_error)?;
        Ok(())
    }

    fn expected(&self, expected: impl Into<String>) -> ParseError {
        ParseError::Expected {
            expected: expected.into(),
            loc: DiagLoc::span(self.current.span),
        }
    }
}

fn map_lex_error(e: LexError) -> ParseError {
    match e {
        LexError::IntegerWidthOutOfRange { width, max, span } => {
            ParseError::IntegerWidthOutOfRange {
                width,
                max,
                loc: DiagLoc::span(span),
            }
        }
        other => ParseError::Lex(other),
    }
}

fn same_token_kind(lhs: &Token<'_>, rhs: &Token<'_>) -> bool {
    matches!(
        (lhs, rhs),
        (Token::Eof, Token::Eof)
            | (Token::Equal, Token::Equal)
            | (Token::Comma, Token::Comma)
            | (Token::LParen, Token::LParen)
            | (Token::RParen, Token::RParen)
            | (Token::LSquare, Token::LSquare)
            | (Token::RSquare, Token::RSquare)
            | (Token::LBrace, Token::LBrace)
            | (Token::RBrace, Token::RBrace)
            | (Token::Less, Token::Less)
            | (Token::Greater, Token::Greater)
            | (Token::Colon, Token::Colon)
    )
}

impl fmt::Display for ModuleSummaryIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for module in &self.modules {
            writeln!(f, "{module}")?;
        }
        for global in &self.globals {
            writeln!(f, "{global}")?;
        }
        for type_id in &self.type_ids {
            writeln!(f, "{type_id}")?;
        }
        if let Some(flags) = &self.flags {
            writeln!(f, "^0 = flags: {}", flags.raw)?;
        }
        if let Some(block_count) = self.block_count {
            writeln!(f, "^0 = blockcount: {block_count}")?;
        }
        Ok(())
    }
}

impl fmt::Display for ModuleEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "^{} = module: (path: ", self.id)?;
        write_quoted(f, &self.path)?;
        if let Some(hash) = self.hash {
            write!(
                f,
                ", hash: ({}, {}, {}, {}, {})",
                hash[0], hash[1], hash[2], hash[3], hash[4]
            )?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for TypeIdEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "^{} = typeid: (name: ", self.id)?;
        write_quoted(f, &self.name)?;
        write!(f, ")")
    }
}

impl fmt::Display for GvEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "^{} = gv: (", self.id)?;
        match (&self.name, self.guid) {
            (Some(name), _) => {
                write!(f, "name: ")?;
                write_quoted(f, name)?;
            }
            (None, Some(guid)) => write!(f, "guid: {guid}")?,
            (None, None) => write!(f, "guid: 0")?,
        }
        if !self.summaries.is_empty() {
            write!(f, ", summaries: (")?;
            for (idx, summary) in self.summaries.iter().enumerate() {
                if idx != 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{summary}")?;
            }
            write!(f, ")")?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for GlobalValueSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Function(summary) => write!(f, "{summary}"),
            Self::Variable(summary) => write!(f, "{summary}"),
            Self::Alias(summary) => write!(f, "{summary}"),
        }
    }
}

impl fmt::Display for FunctionSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "function: (module: ^{}, flags: {}, insts: {}",
            self.module, self.flags, self.insts
        )?;
        if let Some(flags) = &self.func_flags {
            write!(f, ", funcFlags: {flags}")?;
        }
        if !self.calls.is_empty() {
            write!(f, ", calls: (")?;
            for (idx, call) in self.calls.iter().enumerate() {
                if idx != 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{call}")?;
            }
            write!(f, ")")?;
        }
        if !self.refs.is_empty() {
            write_refs(f, &self.refs)?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for VariableSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "variable: (module: ^{}, flags: {}, varFlags: {}",
            self.module, self.flags, self.var_flags
        )?;
        if !self.refs.is_empty() {
            write_refs(f, &self.refs)?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for AliasSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "alias: (module: ^{}, flags: {}, aliasee: ",
            self.module, self.flags
        )?;
        match &self.aliasee {
            Some(aliasee) => write!(f, "{aliasee}")?,
            None => write!(f, "null")?,
        }
        write!(f, ")")
    }
}

impl fmt::Display for GvFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(linkage: {}, visibility: {}, notEligibleToImport: {}, live: {}, dsoLocal: {}, canAutoHide: {}",
            self.linkage,
            self.visibility,
            flag(self.not_eligible_to_import),
            flag(self.live),
            flag(self.dso_local),
            flag(self.can_auto_hide)
        )?;
        if self.import_type != SummaryImportType::Definition {
            write!(f, ", importType: {}", self.import_type)?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for FunctionFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(readNone: {}, readOnly: {}, noRecurse: {}, returnDoesNotAlias: {}, noInline: {}, alwaysInline: {}, noUnwind: {}, mayThrow: {}, hasUnknownCall: {}, mustBeUnreachable: {})",
            flag(self.read_none),
            flag(self.read_only),
            flag(self.no_recurse),
            flag(self.return_does_not_alias),
            flag(self.no_inline),
            flag(self.always_inline),
            flag(self.no_unwind),
            flag(self.may_throw),
            flag(self.has_unknown_call),
            flag(self.must_be_unreachable),
        )
    }
}

impl fmt::Display for GVarFlags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "(readonly: {}, writeonly: {}, constant: {}",
            flag(self.read_only),
            flag(self.write_only),
            flag(self.constant)
        )?;
        if self.vcall_visibility != 0 {
            write!(f, ", vcallVisibility: {}", self.vcall_visibility)?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for CallEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "(callee: {}", self.callee)?;
        if self.hotness != SummaryHotness::Unknown {
            write!(f, ", hotness: {}", self.hotness)?;
        }
        if let Some(relbf) = self.relbf {
            write!(f, ", relbf: {relbf}")?;
        }
        if self.tail {
            write!(f, ", tail: 1")?;
        }
        write!(f, ")")
    }
}

impl fmt::Display for GvReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.read_only {
            write!(f, "readonly ")?;
        } else if self.write_only {
            write!(f, "writeonly ")?;
        }
        write!(f, "^{}", self.id)
    }
}

impl fmt::Display for SummaryLinkage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Private => "private",
            Self::Internal => "internal",
            Self::AvailableExternally => "available_externally",
            Self::LinkOnce => "linkonce",
            Self::LinkOnceOdr => "linkonce_odr",
            Self::Weak => "weak",
            Self::WeakOdr => "weak_odr",
            Self::Appending => "appending",
            Self::Common => "common",
            Self::ExternWeak => "extern_weak",
            Self::External => "external",
        })
    }
}

impl fmt::Display for SummaryVisibility {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Default => "default",
            Self::Hidden => "hidden",
            Self::Protected => "protected",
        })
    }
}

impl fmt::Display for SummaryImportType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Definition => "definition",
            Self::Declaration => "declaration",
        })
    }
}

impl fmt::Display for SummaryHotness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unknown => "unknown",
            Self::Cold => "cold",
            Self::None => "none",
            Self::Hot => "hot",
            Self::Critical => "critical",
        })
    }
}

fn write_refs(f: &mut fmt::Formatter<'_>, refs: &[GvReference]) -> fmt::Result {
    write!(f, ", refs: (")?;
    for (idx, reference) in refs.iter().enumerate() {
        if idx != 0 {
            write!(f, ", ")?;
        }
        write!(f, "{reference}")?;
    }
    write!(f, ")")
}

fn write_quoted(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    write!(f, "\"")?;
    for byte in value.bytes() {
        match byte {
            b'\\' => write!(f, "\\5C")?,
            b'\"' => write!(f, "\\22")?,
            0x20..=0x7e => write!(f, "{}", byte as char)?,
            _ => write!(f, "\\{byte:02X}")?,
        }
    }
    write!(f, "\"")
}

#[inline]
fn flag(value: bool) -> u8 {
    u8::from(value)
}
