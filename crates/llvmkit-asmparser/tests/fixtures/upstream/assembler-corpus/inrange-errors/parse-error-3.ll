
; PARSE-ERROR-3: error: expected integer
@g = external global i8
define ptr @test() {
  ret ptr getelementptr inrange(42, (i8, ptr @g, i64 8)
}

