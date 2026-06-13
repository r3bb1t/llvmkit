^1 = module: (path: "mod.ll")
^2 = gv: (name: "func", summaries: (function: (module: ^1, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 1, dsoLocal: 0, canAutoHide: 0), insts: 3, funcFlags: (readNone: 0, readOnly: 0, noRecurse: 0, returnDoesNotAlias: 0, noInline: 0, alwaysInline: 0, noUnwind: 1, mayThrow: 0, hasUnknownCall: 0, mustBeUnreachable: 0), calls: ((callee: ^3, hotness: hot)), refs: (readonly ^3))))
^3 = gv: (name: "glob", summaries: (variable: (module: ^1, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 1, dsoLocal: 0, canAutoHide: 0), varFlags: (readonly: 1, writeonly: 0, constant: 1), refs: (writeonly ^2))))
^4 = gv: (guid: 99, summaries: (alias: (module: ^1, flags: (linkage: external, visibility: default, notEligibleToImport: 0, live: 1, dsoLocal: 0, canAutoHide: 0), aliasee: ^2)))
^5 = typeid: (name: "typeid")
^0 = flags: 7
^0 = blockcount: 9
