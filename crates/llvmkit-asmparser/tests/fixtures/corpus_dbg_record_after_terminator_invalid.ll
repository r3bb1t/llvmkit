; llvmkit corpus fixture derived from upstream `test/Assembler/dbg-record-invalid-0.ll`.
define i32 @f(i32 %a) !dbg !0 {
entry:
  ret i32 %a, !dbg !2
  #dbg_value(i32 %a, !1, !DIExpression(), !2)
}

!0 = distinct !DISubprogram(name: "f", type: !3, unit: !4)
!1 = !DILocalVariable(name: "a", scope: !0, type: !5)
!2 = !DILocation(line: 1, column: 1, scope: !0)
!3 = !DISubroutineType(types: !{!5, !5})
!4 = !DICompileUnit(language: DW_LANG_C99, producer: "llvmkit")
!5 = !DIBasicType(name: "int", size: 32, encoding: DW_ATE_signed)
