%struct.anon = type { i32, i32 }
%struct.test = type { i64, %struct.anon, ptr }

@TestArrayPtr = global ptr getelementptr inbounds ([10 x %struct.test], ptr @TestArray, i64 0, i64 3) ; <ptr> [#uses=1]
@TestArray = common global [10 x %struct.test] zeroinitializer, align 32 ; <ptr> [#uses=2]
