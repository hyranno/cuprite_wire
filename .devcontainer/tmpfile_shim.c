// LD_PRELOAD shim: make glibc tmpfile() honor $TMPDIR.
//
// glibc's tmpfile() always creates its file in /tmp and ignores $TMPDIR.
// The ngspice CLI copies its input through tmpfile(), so it fails where /tmp is
// read-only (e.g. in the Claude Code sandbox). The `ngspice` wrapper in
// /usr/local/bin preloads this library so the file goes to $TMPDIR instead.
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static FILE *make_tmpfile(void) {
    const char *dir = getenv("TMPDIR");
    if (!dir || !*dir) dir = "/tmp";
    size_t size = strlen(dir) + sizeof("/tmpfXXXXXX");
    char *path = malloc(size);
    if (!path) return NULL;
    snprintf(path, size, "%s/tmpfXXXXXX", dir);
    int fd = mkstemp(path);
    if (fd < 0) {
        free(path);
        return NULL;
    }
    // Like tmpfile(), the file has no name and disappears when closed.
    unlink(path);
    free(path);
    FILE *file = fdopen(fd, "w+");
    if (!file) close(fd);
    return file;
}

FILE *tmpfile(void) { return make_tmpfile(); }
FILE *tmpfile64(void) { return make_tmpfile(); }
