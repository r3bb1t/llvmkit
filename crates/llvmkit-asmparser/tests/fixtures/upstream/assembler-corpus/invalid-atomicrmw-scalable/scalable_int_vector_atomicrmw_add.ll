



































define <vscale x 2 x i16> @scalable_int_vector_atomicrmw_add(ptr %x, <vscale x 2 x i16> %val) {
; ERR4: :39: error: atomicrmw operand may not be scalable
  %atomic.add = atomicrmw add ptr %x, <vscale x 2 x i16> %val seq_cst
  ret <vscale x 2 x i16> %atomic.add
}
