define ptr @take_self_numbered() {
  br label %1
1:
  ret ptr blockaddress(@take_self_numbered, %2)
2:
  unreachable
}
