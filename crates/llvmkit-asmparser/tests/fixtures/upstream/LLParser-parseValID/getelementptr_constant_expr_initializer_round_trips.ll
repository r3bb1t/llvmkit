; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; `kw_getelementptr` global-initializer shape supported by llvmkit.

@data = global i8 0
@ptr = global ptr getelementptr inbounds (i8, ptr @data, i64 1)
