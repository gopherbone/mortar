// SPDX-License-Identifier: MIT
// Allocation and logging hooks shared by every module (see ps.h).
#include "ps.h"
#include <string.h>

#ifdef TARGET_PLAYDATE
void *(*ps_realloc_hook)(void *ptr, size_t size) = NULL;   // set by main.c to pd->system->realloc
#else
#include <stdlib.h>
static void *libc_realloc(void *p, size_t n)
{
    if (n == 0) { free(p); return NULL; }
    return realloc(p, n);
}
void *(*ps_realloc_hook)(void *ptr, size_t size) = libc_realloc;
#endif

void (*ps_log_hook)(const char *msg) = NULL;
bool (*ps_over_budget_hook)(void) = NULL;

void *ps_malloc(size_t n)
{
    void *p = ps_realloc_hook(NULL, n ? n : 1);
    if (p) memset(p, 0, n ? n : 1);
    return p;
}

void *ps_malloc_raw(size_t n) { return ps_realloc_hook(NULL, n ? n : 1); }

void *ps_realloc(void *p, size_t n) { return ps_realloc_hook(p, n); }

void ps_free(void *p)
{
    if (p) ps_realloc_hook(p, 0);
}

char *ps_strdup(const char *s)
{
    if (!s) return NULL;
    size_t n = strlen(s) + 1;
    char *d = ps_realloc_hook(NULL, n);
    if (d) memcpy(d, s, n);
    return d;
}
