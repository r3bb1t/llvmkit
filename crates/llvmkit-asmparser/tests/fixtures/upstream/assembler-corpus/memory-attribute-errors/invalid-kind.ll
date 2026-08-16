; INVALID-KIND: error: expected memory location (argmem, inaccessiblemem, errnomem) or access kind (none, read, write, readwrite)
declare void @fn() memory(foo)