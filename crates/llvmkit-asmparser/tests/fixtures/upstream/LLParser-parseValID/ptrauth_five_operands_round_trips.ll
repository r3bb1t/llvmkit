; Direct translation of llvm/lib/AsmParser/LLParser.cpp::LLParser::parseValID
; `kw_ptrauth` five-operand shape supported by llvmkit.

@g = global i8 0
@signed = global ptr ptrauth (ptr @g, i32 0, i64 1, ptr inttoptr (i64 1 to ptr), ptr @g)
