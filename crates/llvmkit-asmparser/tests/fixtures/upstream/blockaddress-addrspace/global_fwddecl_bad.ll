; Excerpted verbatim from llvm/test/Bitcode/blockaddress-addrspace.ll global-fwddecl-bad.ll.

; This forward declaration does not match the actual function type so we should get an error:
@global = constant ptr addrspace(2) blockaddress(@fwddecl_in_unexpected_as, %bb)
; CHECK: [[#@LINE-1]]:77: error: 'bb' defined with type 'ptr addrspace(1)' but expected 'ptr addrspace(2)'
define void @fwddecl_in_unexpected_as() addrspace(1) {
  unreachable
bb:
  ret void
}
