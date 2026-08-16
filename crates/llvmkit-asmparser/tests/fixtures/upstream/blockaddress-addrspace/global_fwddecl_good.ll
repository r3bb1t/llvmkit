; Excerpted verbatim from llvm/test/Bitcode/blockaddress-addrspace.ll global-fwddecl-good.ll.

; Check that a global blockaddress of a forward-declared function
; uses the type of the global variable address space for the forward declaration
@global = constant ptr addrspace(2) blockaddress(@fwddecl_in_prog_as, %bb)
define void @fwddecl_in_prog_as() addrspace(2) {
  unreachable
bb:
  ret void
}
