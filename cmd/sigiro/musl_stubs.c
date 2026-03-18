// Stubs for glibc-specific functions that DuckDB's pre-compiled static library
// calls but musl doesn't provide. These are safe no-ops:
//
//   backtrace_symbols — debug stack traces, returns NULL (no backtraces in release)
//   malloc_trim       — hint to release memory, no-op (musl manages its own heap)
//   __res_init        — DNS resolver reinit, no-op (musl handles DNS differently)
//
// Only compiled into musl-targeted builds via CGO_CFLAGS=-DMUSL_STUBS.

#ifdef MUSL_STUBS

#include <stddef.h>

int backtrace(void **buffer, int size) {
    (void)buffer;
    (void)size;
    return 0;
}

char **backtrace_symbols(void *const *buffer, int size) {
    (void)buffer;
    (void)size;
    return NULL;
}

int malloc_trim(size_t pad) {
    (void)pad;
    return 0;
}

int __res_init(void) {
    return 0;
}

#endif
