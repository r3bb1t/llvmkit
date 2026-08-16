
; END-NOT-LARGER-START: error: expected end to be larger than start
@g = external global i8
define ptr @test() {
  ret ptr getelementptr inrange(42, 42) (i8, ptr @g, i64 8)
}
