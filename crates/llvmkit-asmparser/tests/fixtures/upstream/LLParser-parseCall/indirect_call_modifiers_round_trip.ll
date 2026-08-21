define void @f(ptr %fp, i32 %v, float %fv) {
  tail call void %fp(i32 %v)
  notail call void %fp(i32 %v)
  call fastcc void %fp(i32 %v)
  call cc 99 void %fp(i32 %v)
  %a = call nnan ninf float %fp(float %fv)
  %b = call fast float %fp(float %fv)
  ret void
}
