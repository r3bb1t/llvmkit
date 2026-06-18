; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; unsupported `kw_select` diagnostic.

@x = global i32 select (i1 true, i32 1, i32 2)
