; llvmkit corpus fixture derived from upstream `test/Assembler/dbg_declare_value.ll`.
define void @foo(double %x) !dbg !0 {
entry:
  #dbg_declare_value(double %x, !1, !DIExpression(), !2)
  ret void, !dbg !2
}

!0 = distinct !DISubprogram(name: "foo", type: !3, unit: !4)
!1 = !DILocalVariable(name: "x", scope: !0, type: !5)
!2 = !DILocation(line: 1, column: 17, scope: !0)
!3 = !DISubroutineType(types: !{null, !5})
!4 = !DICompileUnit(language: DW_LANG_C11, producer: "llvmkit")
!5 = !DIBasicType(name: "double", size: 64, encoding: DW_ATE_float)
