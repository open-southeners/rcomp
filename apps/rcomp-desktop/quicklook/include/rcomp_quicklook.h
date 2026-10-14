// C interface of the rcomp-quicklook static library (see src/lib.rs).
#ifndef RCOMP_QUICKLOOK_H
#define RCOMP_QUICKLOOK_H

/// List the archive at the UTF-8 `path` as a JSON document:
///   {"ok":true,"format":"zip","entries":[{"path":"a/b.txt","size":3,"is_dir":false},...]}
///   {"ok":false,"kind":"unsupported","message":"..."}
/// Never returns NULL. Free the result with rcomp_ql_free.
char *rcomp_ql_list_json(const char *path);

/// Free a string returned by rcomp_ql_list_json. NULL is ignored.
void rcomp_ql_free(char *json);

#endif
