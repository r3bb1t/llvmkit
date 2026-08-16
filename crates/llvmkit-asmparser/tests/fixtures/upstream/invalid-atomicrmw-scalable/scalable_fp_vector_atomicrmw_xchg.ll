define <vscale x 2 x half> @scalable_fp_vector_atomicrmw_xchg(ptr %x, <vscale x 2 x half> %val) {
  %atomic.xchg = atomicrmw xchg ptr %x, <vscale x 2 x half> %val seq_cst
  ret <vscale x 2 x half> %atomic.xchg
}
