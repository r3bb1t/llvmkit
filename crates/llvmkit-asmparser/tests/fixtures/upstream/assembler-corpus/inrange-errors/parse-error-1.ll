
; PARSE-ERROR-1: error: expected integer
@g = external global i8
define ptr @test() {
  ret ptr getelementptr inrange (i8, ptr @g, i64 8)
}

