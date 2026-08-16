; ONLYKEYWORD: error: expected '('
define void @nofpclass_only_keyword(float nofpclass %x) {
  ret void
}

; FIXME: Poor diagnostic