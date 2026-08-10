define ptr @take_self_named() {
L1:
  br label %L2
L2:
  ret ptr blockaddress(@take_self_named, %L3)
L3:
  unreachable
}
