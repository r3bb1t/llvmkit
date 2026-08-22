; llvmkit-authored: no upstream .ll spells a bare call to an alias, a global
; variable or a numbered global. `test/Feature/aliases.ll` has
; `%tmp4 = call %FunTy @bar_f()` on an alias, but it is written in typed-pointer
; syntax that LLVM 22.1.4 no longer parses. The rule anchor is
; `LLParser::getGlobalVal`, whose symbol-table lookup accepts any `GlobalValue`.

@gv = global i32 0
@a = alias void (), ptr @f
@0 = alias void (), ptr @f

define void @f() {
  ret void
}

define void @caller() {
  ; CHECK: call void @a()
  call void @a()
  ; CHECK-NEXT: call void @gv()
  call void @gv()
  ; CHECK-NEXT: call void @0()
  call void @0()
  ret void
}
