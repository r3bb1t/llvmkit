; Excerpted verbatim from llvm/test/CodeGen/X86/dso_local_equivalent_errors.ll undefined_func.ll.

; UNDEFINED: error: unknown function 'undefined_func' referenced by dso_local_equivalent
define void @call_undefined() {
  call void dso_local_equivalent @undefined_func()
  ret void
}
