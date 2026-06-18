; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; forward `dso_local_equivalent` and `no_cfi` placeholder resolution inside aggregate initializers.

@d = global [1 x ptr] [ptr dso_local_equivalent @f]
@n = global [1 x ptr] [ptr no_cfi @f]

declare i32 @f(i32)
