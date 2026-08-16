; Excerpted verbatim from llvm/test/Bitcode/blockaddress-addrspace.ll bad-type-not-ptr.ll.

@global = constant i8 blockaddress(@unknown_fn, %bb)
; CHECK: [[#@LINE-1]]:23: error: type of blockaddress must be a pointer and not 'i8'
