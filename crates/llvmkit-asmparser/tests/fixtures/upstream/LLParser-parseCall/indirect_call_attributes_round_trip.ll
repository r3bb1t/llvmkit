define void @f(ptr %fp, i32 %v, ptr %p) {
  call void %fp(i32 noundef %v)
  call void %fp(ptr nonnull align 8 %p)
  call void %fp(i32 %v) #0
  %r = call zeroext i8 %fp(i32 %v)
  ret void
}

attributes #0 = { nounwind }
