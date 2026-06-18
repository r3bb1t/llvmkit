; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; `kw_no_cfi` accepted global-initializer shape.

declare void @f()
@p = global ptr no_cfi @f
