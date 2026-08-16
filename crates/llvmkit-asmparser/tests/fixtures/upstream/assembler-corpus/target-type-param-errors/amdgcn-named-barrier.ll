declare target("amdgcn.named.barrier", i32) @amdgcn_named_barrier()
; CHECK-AMDGCN-NAMEDBARRIER: target extension type amdgcn.named.barrier should have no type parameters and one integer parameter
