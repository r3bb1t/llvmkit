; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; `kw_dso_local_equivalent` accepted global-initializer shape.

declare void @f()
@p = global ptr dso_local_equivalent @f
