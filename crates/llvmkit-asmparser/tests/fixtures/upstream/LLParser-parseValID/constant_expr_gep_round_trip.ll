; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; general `kw_getelementptr` constant-expression shape supported by llvmkit.

@data = global i8 0
@ptr = global ptr getelementptr (i8, ptr @data, i64 1)
