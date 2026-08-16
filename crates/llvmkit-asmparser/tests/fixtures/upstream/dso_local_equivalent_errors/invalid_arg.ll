; Excerpted verbatim from llvm/test/CodeGen/X86/dso_local_equivalent_errors.ll invalid_arg.ll.

; INVALID: error: expected a function, alias to function, or ifunc in dso_local_equivalent
define void @call_global_var() {
  call void dso_local_equivalent @glob()
  ret void
}

@glob = constant i32 1
