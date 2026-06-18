; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; forward `dso_local_equivalent` and `no_cfi` placeholder resolution.

@d = global ptr dso_local_equivalent @f
@n = global ptr no_cfi @f

declare i32 @f(i32)
