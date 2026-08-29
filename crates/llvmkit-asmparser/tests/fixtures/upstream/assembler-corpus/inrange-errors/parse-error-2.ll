
; PARSE-ERROR-2: error: expected ','
@g = external global i8
define ptr @test() {
  ret ptr getelementptr inrange(42 (i8, ptr @g, i64 8)
}

