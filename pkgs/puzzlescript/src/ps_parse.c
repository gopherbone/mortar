// SPDX-License-Identifier: MIT
// Port of parser.js (+ the bits of codemirror/stringstream.js and languageConstants.js it needs),
// plus small runtime helpers shared with ps_compile.c (arena, vectors, atoms, JS string semantics).
#include "ps_compile_internal.h"
#include <string.h>
#include <math.h>
#ifndef TARGET_PLAYDATE
#include <stdio.h>
#endif

// =============================================================================================
// arena / vectors / string builder

#define AR_BLOCK 65536

static void *ar_alloc_(Arena *a, size_t n, bool zero)
{
    size_t hdr = (sizeof(ArenaBlock) + 7) & ~(size_t)7;
#ifdef PS_ARENA_DEBUG
    {   // one malloc per allocation with a canary tail, checked in ar_free_all
        ArenaBlock *nb = ps_malloc(hdr + n + 16 + sizeof(void *));
        nb->cap = nb->used = n; nb->next = a->head; a->head = nb; a->total += hdr + n;
        unsigned char *p = (unsigned char *)nb + hdr;
        memset(p + n, 0xAB, 16);
        void *ra = __builtin_return_address(0);
        memcpy(p + n + 16, &ra, sizeof ra);
        return p;
    }
#endif
    n = (n + 7) & ~(size_t)7;
    if (n == 0) n = 8;
    ArenaBlock *b = a->head;
    if (!b || b->used + n > b->cap) {
        size_t cap = n > AR_BLOCK / 2 ? n : AR_BLOCK;
        ArenaBlock *nb = ps_malloc_raw(hdr + cap);   // allocations are zeroed individually below
        if (!nb) return NULL;
        nb->cap = cap; nb->used = 0;
        a->total += hdr + cap;
        if (b && n > AR_BLOCK / 2) {      // big block: keep the current block as head
            nb->next = b->next; b->next = nb;
            nb->used = n;
            if (zero) memset((char *)nb + hdr, 0, n);
            return (char *)nb + hdr;
        }
        nb->next = b; a->head = nb; b = nb;
    }
    void *p = (char *)b + hdr + b->used;
    b->used += n;
    if (zero) memset(p, 0, n);
    return p;
}
void *ar_alloc(Arena *a, size_t n) { return ar_alloc_(a, n, true); }
void *ar_alloc_raw(Arena *a, size_t n) { return ar_alloc_(a, n, false); }

void ar_free_all(Arena *a)
{
    ArenaBlock *b = a->head;
#if defined(PS_ARENA_DEBUG) && !defined(TARGET_PLAYDATE)
    size_t hdr = (sizeof(ArenaBlock) + 7) & ~(size_t)7;
    for (ArenaBlock *q = b; q; q = q->next) {
        unsigned char *p = (unsigned char *)q + hdr + q->cap;
        for (int i = 0; i < 16; i++) if (p[i] != 0xAB) { void *ra; memcpy(&ra, p + 16, sizeof ra); fprintf(stderr, "arena overrun: alloc size %zu from %p\n", q->cap, ra); break; }
    }
#endif
    while (b) { ArenaBlock *n = b->next; ps_free(b); b = n; }
    a->head = NULL; a->total = 0;
}

void ar_reset(Arena *a)
{
#ifdef PS_ARENA_DEBUG
    ar_free_all(a);
#else
    ArenaBlock *keep = NULL, *b = a->head;
    size_t hdr = (sizeof(ArenaBlock) + 7) & ~(size_t)7;
    while (b) {
        ArenaBlock *n = b->next;
        if (!keep && b->cap == AR_BLOCK) keep = b; else ps_free(b);
        b = n;
    }
    a->head = keep; a->total = 0;
    if (keep) { keep->next = NULL; keep->used = 0; a->total = hdr + keep->cap; }
#endif
}

static void copy_to_words(char *o, const char *s, int n, bool lower);
char *ar_strndup(Arena *a, const char *s, size_t n)
{
    char *d = ar_alloc_raw(a, n + 1);
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
    copy_to_words(d, s, (int)n, false);
#else
    memcpy(d, s, n); d[n] = 0;
#endif
    return d;
}
char *ar_strdup(Arena *a, const char *s) { return s ? ar_strndup(a, s, strlen(s)) : NULL; }

void iv_push(Arena *a, IVec *v, int x)
{
    if (v->n == v->cap) {
        int nc = v->cap ? v->cap * 2 : 4;
        int *nv = ar_alloc_raw(a, sizeof(int) * nc);
        if (v->n) memcpy(nv, v->v, sizeof(int) * v->n);
        v->v = nv; v->cap = nc;
    }
    v->v[v->n++] = x;
}
void pv_push(Arena *a, PVec *v, void *x)
{
    if (v->n == v->cap) {
        int nc = v->cap ? v->cap * 2 : 4;
        void **nv = ar_alloc_raw(a, sizeof(void *) * nc);
        if (v->n) memcpy(nv, v->v, sizeof(void *) * v->n);
        v->v = nv; v->cap = nc;
    }
    v->v[v->n++] = x;
}
bool iv_has(const IVec *v, int x) { return iv_index(v, x, 0) >= 0; }
int iv_index(const IVec *v, int x, int from)
{
    for (int i = from < 0 ? 0 : from; i < v->n; i++) if (v->v[i] == x) return i;
    return -1;
}
void iv_splice(IVec *v, int at, int count)
{
    if (at < 0 || at >= v->n) return;
    if (at + count > v->n) count = v->n - at;
    memmove(v->v + at, v->v + at + count, sizeof(int) * (v->n - at - count));
    v->n -= count;
}
void pv_splice(PVec *v, int at, int count)
{
    if (at < 0 || at >= v->n) return;
    if (at + count > v->n) count = v->n - at;
    memmove(v->v + at, v->v + at + count, sizeof(void *) * (v->n - at - count));
    v->n -= count;
}
IVec iv_clone(Arena *a, const IVec *v)
{
    IVec r; r.n = v->n; r.cap = v->n;
    r.v = v->n ? ar_alloc_raw(a, sizeof(int) * v->n) : NULL;
    if (v->n) memcpy(r.v, v->v, sizeof(int) * v->n);
    return r;
}

void sb_init(SB *b, Arena *a) { b->a = a; b->s = NULL; b->n = 0; b->cap = 0; }
static void sb_reserve(SB *b, int extra)
{
    if (b->n + extra + 1 > b->cap) {
        int nc = b->cap ? b->cap * 2 : 64;
        while (nc < b->n + extra + 1) nc *= 2;
        char *ns = ar_alloc_raw(b->a, nc);
        if (b->n) memcpy(ns, b->s, b->n);
        b->s = ns; b->cap = nc;
    }
}
void sb_putc(SB *b, char c) { sb_reserve(b, 1); b->s[b->n++] = c; }
void sb_putn(SB *b, const char *s, int n) { sb_reserve(b, n); memcpy(b->s + b->n, s, n); b->n += n; }
void sb_puts(SB *b, const char *s) { if (s) sb_putn(b, s, (int)strlen(s)); }
static void sb_putint(SB *b, long long v)
{
    char tmp[24]; int k = 0;
    unsigned long long u = v < 0 ? (unsigned long long)(-v) : (unsigned long long)v;
    do { tmp[k++] = (char)('0' + u % 10); u /= 10; } while (u);
    if (v < 0) sb_putc(b, '-');
    while (k) sb_putc(b, tmp[--k]);
}
void sb_vprintf(SB *b, const char *fmt, va_list ap)
{
    for (const char *p = fmt; *p; p++) {
        if (*p != '%') { sb_putc(b, *p); continue; }
        p++;
        switch (*p) {
        case 's': sb_puts(b, va_arg(ap, const char *)); break;
        case 'S': { const char *s = va_arg(ap, const char *); if (s) sb_puts(b, js_upper(b->a, s)); break; }
        case 'd': sb_putint(b, va_arg(ap, int)); break;
        case 'c': sb_putc(b, (char)va_arg(ap, int)); break;
        case '%': sb_putc(b, '%'); break;
        case 0: p--; break;
        default: sb_putc(b, '%'); sb_putc(b, *p); break;
        }
    }
}
void sb_printf(SB *b, const char *fmt, ...) { va_list ap; va_start(ap, fmt); sb_vprintf(b, fmt, ap); va_end(ap); }
char *sb_done(SB *b) { sb_reserve(b, 0); b->s[b->n] = 0; return b->s; }
char *tfmt(Arena *a, const char *fmt, ...)
{
    SB b; sb_init(&b, a);
    va_list ap; va_start(ap, fmt); sb_vprintf(&b, fmt, ap); va_end(ap);
    return sb_done(&b);
}

// =============================================================================================
// UTF-8 and JS character semantics

int u8_decode(const char *str, int len, int pos, int *cp)
{
    const unsigned char *s = (const unsigned char *)str;
    unsigned c = s[pos];
    if (c < 0x80) { *cp = (int)c; return 1; }
    int n = c >= 0xF0 ? 4 : c >= 0xE0 ? 3 : c >= 0xC0 ? 2 : 1;
    if (n == 1 || pos + n > len) { *cp = (int)c; return 1; }
    unsigned v = c & (0x3F >> (n - 1));
    for (int i = 1; i < n; i++) {
        if ((s[pos + i] & 0xC0) != 0x80) { *cp = (int)c; return 1; }
        v = (v << 6) | (s[pos + i] & 0x3F);
    }
    *cp = (int)v;
    return n;
}

int u8_encode(uint32_t cp, char *o)
{
    if (cp < 0x80) { o[0] = (char)cp; return 1; }
    if (cp < 0x800) { o[0] = (char)(0xC0 | (cp >> 6)); o[1] = (char)(0x80 | (cp & 0x3F)); return 2; }
    if (cp < 0x10000) { o[0] = (char)(0xE0 | (cp >> 12)); o[1] = (char)(0x80 | ((cp >> 6) & 0x3F)); o[2] = (char)(0x80 | (cp & 0x3F)); return 3; }
    o[0] = (char)(0xF0 | (cp >> 18)); o[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
    o[2] = (char)(0x80 | ((cp >> 6) & 0x3F)); o[3] = (char)(0x80 | (cp & 0x3F)); return 4;
}

bool js_is_Z(int c)
{
    return c == 0x20 || c == 0xA0 || c == 0x1680 || (c >= 0x2000 && c <= 0x200A) ||
           c == 0x2028 || c == 0x2029 || c == 0x202F || c == 0x205F || c == 0x3000;
}
bool js_is_space(int c) { return (c >= 9 && c <= 13) || c == 0xFEFF || js_is_Z(c); }
bool js_is_wordchar(int c)
{
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_';
}
// approximation of [\p{L}\p{N}_]
bool js_is_namechar(int c)
{
    if (c < 0x80) return js_is_wordchar(c);
    if (c < 0xC0) return c == 0xAA || c == 0xB5 || c == 0xBA || c == 0xB2 || c == 0xB3 || c == 0xB9 ||
                         (c >= 0xBC && c <= 0xBE);
    if (c == 0xD7 || c == 0xF7) return false;
    if (c < 0x300) return true;
    if (c < 0x370) return false;                        // combining marks
    if (c < 0x400) return !(c == 0x375 || c == 0x37E || c == 0x384 || c == 0x385 || c == 0x387);
    if (c < 0x530) return !(c >= 0x482 && c <= 0x489);
    if (c >= 0x2000 && c < 0x2C00) {
        return (c >= 0x2070 && c <= 0x209F) || (c >= 0x2150 && c <= 0x2189) ||
               (c >= 0x2460 && c <= 0x249B) || (c >= 0x24EA && c <= 0x24FF);
    }
    if (c >= 0x2E00 && c < 0x2E80) return false;
    if (c >= 0x3000 && c < 0x3040)
        return (c >= 0x3005 && c <= 0x3007) || (c >= 0x3021 && c <= 0x3029) ||
               (c >= 0x3031 && c <= 0x3035) || (c >= 0x3038 && c <= 0x303C);
    if (c >= 0xD800 && c < 0xF900) return false;        // surrogates, private use
    if (c >= 0xFE00 && c < 0xFE70) return false;
    if (c >= 0xFF00 && c < 0xFF66)
        return (c >= 0xFF10 && c <= 0xFF19) || (c >= 0xFF21 && c <= 0xFF3A) || (c >= 0xFF41 && c <= 0xFF5A);
    if (c >= 0xFFF0 && c <= 0xFFFF) return false;
    if (c >= 0x1F000 && c < 0x1FB00) return false;      // emoji & symbols
    if (c >= 0xE0000) return false;
    return true;
}

int js_lower_cp(int c)
{
    if (c < 0x80) return (c >= 'A' && c <= 'Z') ? c + 32 : c;
    if (c >= 0xC0 && c <= 0xDE && c != 0xD7) return c + 32;
    if (c >= 0x100 && c <= 0x137) return (c & 1) ? c : c + 1;
    if (c >= 0x139 && c <= 0x148) return (c & 1) ? c + 1 : c;
    if (c >= 0x14A && c <= 0x177) return (c & 1) ? c : c + 1;
    if (c == 0x178) return 0xFF;
    if (c >= 0x179 && c <= 0x17E) return (c & 1) ? c + 1 : c;
    if (c >= 0x391 && c <= 0x3A9 && c != 0x3A2) return c + 32;
    if (c == 0x386) return 0x3AC;
    if (c >= 0x388 && c <= 0x38A) return c + 37;
    if (c == 0x38C) return 0x3CC;
    if (c == 0x38E || c == 0x38F) return c + 63;
    if (c >= 0x410 && c <= 0x42F) return c + 32;
    if (c >= 0x400 && c <= 0x40F) return c + 80;
    return c;
}
int js_upper_cp(int c)
{
    if (c < 0x80) return (c >= 'a' && c <= 'z') ? c - 32 : c;
    if (c >= 0xE0 && c <= 0xFE && c != 0xF7) return c - 32;
    if (c >= 0x100 && c <= 0x137) return (c & 1) ? c - 1 : c;
    if (c >= 0x139 && c <= 0x148) return (c & 1) ? c : c - 1;
    if (c >= 0x14A && c <= 0x177) return (c & 1) ? c - 1 : c;
    if (c >= 0x179 && c <= 0x17E) return (c & 1) ? c : c - 1;
    if (c >= 0x3B1 && c <= 0x3C9 && c != 0x3C2) return c - 32;
    if (c >= 0x430 && c <= 0x44F) return c - 32;
    if (c >= 0x450 && c <= 0x45F) return c - 80;
    return c;
}
// store n bytes from s to the (8-aligned) arena buffer o a word at a time: heap stores are what
// costs on the device, and newlib's memcpy goes byte by byte for unaligned sources
static void copy_to_words(char *o, const char *s, int n, bool lower)
{
    int i = 0;
    for (; i + 4 <= n; i += 4) {
        uint32_t w = 0;
        for (int k = 3; k >= 0; k--) {
            uint32_t b = (unsigned char)s[i + k];
            if (lower && b - 'A' < 26u) b |= 0x20;
            w = (w << 8) | b;
        }
        memcpy(o + i, &w, 4);   // (little endian: byte k is bits 8k..8k+7)
    }
    for (; i < n; i++) { unsigned char b = (unsigned char)s[i]; if (lower && b - 'A' < 26u) b |= 0x20; o[i] = (char)b; }
    o[n] = 0;
}
static char *js_mapcase(Arena *a, const char *s, int (*f)(int))
{
    int len = (int)strlen(s);
#if defined(__BYTE_ORDER__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
    if (f == js_lower_cp) {   // ASCII only: js_lower_cp maps just A-Z
        int i = 0;
        while (i < len && (unsigned char)s[i] < 0x80) i++;
        if (i == len) { char *o = ar_alloc_raw(a, len + 1); copy_to_words(o, s, len, true); return o; }
    }
#endif
    char *o = ar_alloc(a, len + 1);
    int pos = 0, k = 0;
    while (pos < len) {
        int cp, n = u8_decode(s, len, pos, &cp);
        int m = f(cp);
        char tmp[4];
        if (m != cp && u8_encode((uint32_t)m, tmp) == n) memcpy(o + k, tmp, n);
        else memcpy(o + k, s + pos, n);
        k += n; pos += n;
    }
    o[k] = 0;
    return o;
}
char *js_lower(Arena *a, const char *s) { return js_mapcase(a, s, js_lower_cp); }
char *js_upper(Arena *a, const char *s) { return js_mapcase(a, s, js_upper_cp); }

static int cp_before(const char *s, int pos)      // start of the code point ending at pos
{
    int p = pos - 1;
    while (p > 0 && (((unsigned char)s[p]) & 0xC0) == 0x80) p--;
    return p;
}
char *js_trim(Arena *a, const char *s, int n)
{
    int b = 0, e = n, cp;
    while (b < e) { int k = u8_decode(s, e, b, &cp); if (!js_is_space(cp)) break; b += k; }
    while (e > b) { int p = cp_before(s, e); u8_decode(s, e, p, &cp); if (!js_is_space(cp)) break; e = p; }
    return ar_strndup(a, s + b, e - b);
}
int js_utf16_len_n(const char *s, int len)
{
    int pos = 0, n = 0;
    while (pos < len) { int cp; pos += u8_decode(s, len, pos, &cp); n += cp >= 0x10000 ? 2 : 1; }
    return n;
}
int js_utf16_len(const char *s) { return js_utf16_len_n(s, (int)strlen(s)); }

// Number(string)
static const char *skip_js_space(const char *s, const char *e)
{
    while (s < e) { int cp; int k = u8_decode(s, (int)(e - s), 0, &cp); if (!js_is_space(cp)) break; s += k; }
    return s;
}
static double parse_decimal(const char *p, const char *e, const char **endp)
{
    const char *start = p;
    double sign = 1;
    if (p < e && (*p == '+' || *p == '-')) { if (*p == '-') sign = -1; p++; }
    if (e - p >= 8 && memcmp(p, "Infinity", 8) == 0) { *endp = p + 8; return sign * (double)INFINITY; }
    double v = 0; int digits = 0;
    while (p < e && *p >= '0' && *p <= '9') { v = v * 10 + (*p - '0'); p++; digits++; }
    if (p < e && *p == '.') {
        p++;
        double f = 0.1;
        while (p < e && *p >= '0' && *p <= '9') { v += (*p - '0') * f; f *= 0.1; p++; digits++; }
    }
    if (!digits) { *endp = start; return NAN; }
    if (p < e && (*p == 'e' || *p == 'E')) {
        const char *q = p + 1; int es = 1, ev = 0, ed = 0;
        if (q < e && (*q == '+' || *q == '-')) { if (*q == '-') es = -1; q++; }
        while (q < e && *q >= '0' && *q <= '9') { ev = ev * 10 + (*q - '0'); q++; ed++; if (ev > 400) ev = 400; }
        if (ed) { v *= pow(10, es * ev); p = q; }
    }
    *endp = p;
    return sign * v;
}
double js_number(const char *s)
{
    const char *e = s + strlen(s);
    const char *p = skip_js_space(s, e);
    while (e > p) { const char *q = e - 1; while (q > p && (((unsigned char)*q) & 0xC0) == 0x80) q--; int cp; u8_decode(q, (int)(e - q), 0, &cp); if (!js_is_space(cp)) break; e = q; }
    if (p == e) return 0;
    if (e - p > 2 && p[0] == '0' && (p[1] == 'x' || p[1] == 'X')) {
        double v = 0;
        for (const char *q = p + 2; q < e; q++) {
            int d = (*q >= '0' && *q <= '9') ? *q - '0' : (*q >= 'a' && *q <= 'f') ? *q - 'a' + 10 : (*q >= 'A' && *q <= 'F') ? *q - 'A' + 10 : -1;
            if (d < 0) return NAN;
            v = v * 16 + d;
        }
        return v;
    }
    const char *end;
    double v = parse_decimal(p, e, &end);
    if (end != e) return NAN;
    return v;
}
double js_parsefloat(const char *s)
{
    const char *e = s + strlen(s);
    const char *p = skip_js_space(s, e), *end;
    return parse_decimal(p, e, &end);
}
double js_parseint(const char *s)
{
    const char *e = s + strlen(s);
    const char *p = skip_js_space(s, e);
    double sign = 1;
    if (p < e && (*p == '+' || *p == '-')) { if (*p == '-') sign = -1; p++; }
    double v = 0; int d = 0;
    while (p < e && *p >= '0' && *p <= '9') { v = v * 10 + (*p - '0'); p++; d++; }
    return d ? sign * v : (double)NAN;
}
bool js_is_array_index(const char *s)
{
    if (!s[0]) return false;
    if (s[0] == '0') return s[1] == 0;
    unsigned long long v = 0;
    for (const char *p = s; *p; p++) {
        if (*p < '0' || *p > '9') return false;
        v = v * 10 + (unsigned)(*p - '0');
        if (v >= 4294967295ull) return false;
    }
    return true;
}

// =============================================================================================
// atoms & dictionaries

static unsigned str_hash(const char *s, int n)
{
    unsigned h = 2166136261u;
    for (int i = 0; i < n; i++) { h ^= (unsigned char)s[i]; h *= 16777619u; }
    return h;
}
// recently looked-up short atoms, strings inline: source text repeats the same few names, and the
// bucket chains + strings are cache misses on the device
static inline AtomRecent *atom_recent_slot(Atoms *t, unsigned h) { return &t->recent[(h ^ (h >> 11)) & (ATOM_RECENT - 1)]; }
static int atom_lookup(Atoms *t, const char *s, int n, unsigned h)
{
    if (!t->nbuckets) return -1;
    AtomRecent *r = atom_recent_slot(t, h);
    if (r->id > 0 && r->hash == h && r->len == n && memcmp(r->s, s, n) == 0) return r->id - 1;
    for (int i = t->buckets[h & (t->nbuckets - 1)]; i >= 0; i = t->hash_next[i])
        if (t->hashes[i] == h && t->lens[i] == n && memcmp(t->str[i], s, n) == 0) {
            if (n <= (int)sizeof r->s) { r->id = i + 1; r->hash = h; r->len = n; memcpy(r->s, s, n); }
            return i;
        }
    return -1;
}
int atom_n(Atoms *t, const char *s, int n)
{
    unsigned h = str_hash(s, n);
    int f = atom_lookup(t, s, n, h);
    if (f >= 0) return f;
    if (t->n == t->cap) {
        int nc = t->cap ? t->cap * 2 : 256;
        char **ns = ar_alloc(t->a, sizeof(char *) * nc);
        int *nn = ar_alloc(t->a, sizeof(int) * nc);
        unsigned *nh = ar_alloc_raw(t->a, sizeof(unsigned) * nc);
        int *nl = ar_alloc_raw(t->a, sizeof(int) * nc);
        if (t->n) { memcpy(ns, t->str, sizeof(char *) * t->n); memcpy(nh, t->hashes, sizeof(unsigned) * t->n); memcpy(nl, t->lens, sizeof(int) * t->n); }
        t->str = ns; t->hash_next = nn; t->hashes = nh; t->lens = nl; t->cap = nc;
        // rehash
        t->nbuckets = nc * 2;
        t->buckets = ar_alloc(t->a, sizeof(int) * t->nbuckets);
        for (int i = 0; i < t->nbuckets; i++) t->buckets[i] = -1;
        for (int i = 0; i < t->n; i++) {
            unsigned hh = t->hashes[i] & (t->nbuckets - 1);
            t->hash_next[i] = t->buckets[hh]; t->buckets[hh] = i;
        }
    }
    int id = t->n++;
    t->str[id] = ar_strndup(t->a, s, n);
    t->hashes[id] = h; t->lens[id] = n;
    unsigned b = h & (t->nbuckets - 1);
    t->hash_next[id] = t->buckets[b]; t->buckets[b] = id;
    return id;
}
int atom(Atoms *t, const char *s) { return atom_n(t, s, (int)strlen(s)); }
int atom_find(Atoms *t, const char *s) { return atom_lookup(t, s, (int)strlen(s), str_hash(s, (int)strlen(s))); }

void ad_init(ADict *d, Arena *a) { memset(d, 0, sizeof *d); d->a = a; }
static void ad_grow(ADict *d, int key)
{
    if (key < d->cap) return;
    int nc = d->cap ? d->cap : 64;
    while (nc <= key) nc *= 2;
    void **nv = ar_alloc(d->a, sizeof(void *) * nc);
    bool *nh = ar_alloc(d->a, sizeof(bool) * nc);
    if (d->cap) { memcpy(nv, d->val, sizeof(void *) * d->cap); memcpy(nh, d->has, sizeof(bool) * d->cap); }
    d->val = nv; d->has = nh; d->cap = nc;
}
bool ad_has(const ADict *d, int key) { return key >= 0 && key < d->cap && d->has[key]; }
void *ad_get(const ADict *d, int key) { return ad_has(d, key) ? d->val[key] : NULL; }
void ad_set(ADict *d, int key, void *val)
{
    ad_grow(d, key);
    if (!d->has[key]) { d->has[key] = true; iv_push(d->a, &d->order, key); }
    d->val[key] = val;
}
void ad_del(ADict *d, int key)
{
    if (!ad_has(d, key)) return;
    d->has[key] = false; d->val[key] = NULL;
    int i = iv_index(&d->order, key, 0);
    if (i >= 0) iv_splice(&d->order, i, 1);
}
int *ad_keys(PSComp *c, const ADict *d, int *count)
{
    int n = d->order.n;
    int *r = ar_alloc(&c->tmp, sizeof(int) * (n + 1));
    int k = 0;
    // array-index keys ascending
    int *idx = ar_alloc(&c->tmp, sizeof(int) * (n + 1)); int ni = 0;
    for (int i = 0; i < n; i++) if (js_is_array_index(ATOM_STR(c, d->order.v[i]))) idx[ni++] = d->order.v[i];
    for (int i = 1; i < ni; i++) {   // insertion sort by numeric value
        int x = idx[i]; double xv = js_number(ATOM_STR(c, x)); int j = i - 1;
        while (j >= 0 && js_number(ATOM_STR(c, idx[j])) > xv) { idx[j + 1] = idx[j]; j--; }
        idx[j + 1] = x;
    }
    for (int i = 0; i < ni; i++) r[k++] = idx[i];
    for (int i = 0; i < n; i++) if (!js_is_array_index(ATOM_STR(c, d->order.v[i]))) r[k++] = d->order.v[i];
    *count = k;
    return r;
}

// =============================================================================================
// logging (logIssue & co)

#define MAX_ERRORS_FOR_REAL 100

static void log_issue(PSComp *c, const char *str, int line, bool urgent, bool warning, bool countsAsError)
{
    const char *text = line >= 0 ? tfmt(&c->tmp, "line %d : %s", line, str) : str;
    text = js_trim(&c->tmp, text, (int)strlen(text));
    const char *key = tfmt(&c->tmp, "%c%s", warning ? 'W' : 'E', text);
    bool dup = false;
    for (int i = 0; i < c->errKeys.n; i++) if (strcmp(c->errKeys.v[i], key) == 0) { dup = true; break; }
    if (dup && !urgent) return;
    if (ps_log_hook) ps_log_hook(text);
    if (!dup || c->compiling) {
        pv_push(&c->perm, &c->errs, ar_strdup(&c->perm, text));
        pv_push(&c->perm, &c->errKeys, ar_strdup(&c->perm, key));
        if (countsAsError) c->errorCount++;
    }
    if (c->errs.n > MAX_ERRORS_FOR_REAL) {
        if (ps_log_hook) ps_log_hook("Too many errors/warnings; aborting compilation.");
        longjmp(c->abort_jb, 1);
    }
}

void ps_log_error(PSComp *c, int line, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt);
    SB b; sb_init(&b, &c->tmp); sb_vprintf(&b, fmt, ap); va_end(ap);
    log_issue(c, sb_done(&b), line, false, false, true);
}
void ps_log_error_urgent(PSComp *c, int line, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt);
    SB b; sb_init(&b, &c->tmp); sb_vprintf(&b, fmt, ap); va_end(ap);
    log_issue(c, sb_done(&b), line, true, false, true);
}
void ps_log_warning(PSComp *c, int line, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt);
    SB b; sb_init(&b, &c->tmp); sb_vprintf(&b, fmt, ap); va_end(ap);
    // logWarningNoLine counts as an error (sic, as in the JS)
    log_issue(c, sb_done(&b), line, false, true, line < 0);
}
void ps_log_warning_noline(PSComp *c, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt);
    SB b; sb_init(&b, &c->tmp); sb_vprintf(&b, fmt, ap); va_end(ap);
    log_issue(c, sb_done(&b), -1, false, true, true);
}

// =============================================================================================
// language constants

const char *const ps_sectionNames[7] = { "objects", "legend", "sounds", "collisionlayers", "rules", "winconditions", "levels" };
enum { SEC_PRELUDE = -1, SEC_OBJECTS = 0, SEC_LEGEND, SEC_SOUNDS, SEC_COLLISIONLAYERS, SEC_RULES, SEC_WINCONDITIONS, SEC_LEVELS };

const char *const ps_color_names[24] = {
    "black", "white", "grey", "darkgrey", "lightgrey", "gray", "darkgray", "lightgray", "red", "darkred",
    "lightred", "brown", "darkbrown", "lightbrown", "orange", "yellow", "green", "darkgreen", "lightgreen",
    "blue", "lightblue", "darkblue", "purple", "pink"
};
int ps_color_name_index(const char *s)
{
    for (int i = 0; i < 24; i++) if (strcmp(s, ps_color_names[i]) == 0) return i;
    return -1;
}

static const char *const keyword_array[] = {
    "objects", "collisionlayers", "legend", "sounds", "rules", "...", "winconditions", "levels", "|", "[", "]",
    "up", "down", "left", "right", "late", "rigid", "^", "v", ">", "<", "no", "randomdir", "random",
    "horizontal", "vertical", "any", "all", "no", "some", "moving", "stationary", "parallel", "perpendicular",
    "action", "message", "move", "action", "create", "destroy", "cantmove", "sfx0", "sfx1", "sfx2", "sfx3",
    "sfx4", "sfx5", "sfx6", "sfx7", "sfx8", "sfx9", "sfx10", "cancel", "restart", "win", "message", "again",
    "undo", "restart", "titlescreen", "startgame", "cancel", "endgame", "startlevel", "endlevel",
    "showmessage", "closemessage", NULL
};
static const char *const commandwords[] = {
    "sfx0", "sfx1", "sfx2", "sfx3", "sfx4", "sfx5", "sfx6", "sfx7", "sfx8", "sfx9", "sfx10", "cancel",
    "checkpoint", "restart", "win", "message", "again", NULL
};
static const char *const metadata_with_value[] = {
    "title", "author", "homepage", "background_color", "text_color", "key_repeat_interval",
    "realtime_interval", "again_interval", "flickscreen", "zoomscreen", "color_palette", "youtube", NULL
};
static const char *const metadata_no_value[] = {
    "run_rules_on_level_start", "norepeat_action", "require_player_movement", "debug", "verbose_logging",
    "throttle_movement", "noundo", "noaction", "nohack", "norestart", "scanline", NULL
};
static const char *const soundevents[] = {
    "undo", "restart", "titlescreen", "startgame", "cancel", "endgame", "startlevel", "endlevel",
    "showmessage", "closemessage", "sfx0", "sfx1", "sfx2", "sfx3", "sfx4", "sfx5", "sfx6", "sfx7", "sfx8",
    "sfx9", "sfx10", NULL
};
static const char *const soundverbs[] = { "move", "action", "create", "destroy", "cantmove", NULL };
static const char *const sounddirs[] = { "up", "down", "left", "right", "horizontal", "vertical", "orthogonal", NULL };
static const char *const directions_tok[] = {   // reg_directions
    "action", "up", "down", "left", "right", "^", "v", "<", ">", "moving", "stationary", "parallel",
    "perpendicular", "horizontal", "orthogonal", "vertical", "no", "randomdir", "random", NULL
};
static const char *const ruledirectionindicators[] = { "up", "down", "left", "right", "horizontal", "vertical", "orthogonal", "late", "rigid", NULL };
static const char *const colorregex[] = {
    "black", "white", "gray", "darkgray", "lightgray", "grey", "darkgrey", "lightgrey", "red", "darkred",
    "lightred", "brown", "darkbrown", "lightbrown", "orange", "yellow", "green", "darkgreen", "lightgreen",
    "blue", "lightblue", "darkblue", "purple", "pink", "transparent", NULL
};

static bool in_list(const char *const *l, const char *s)
{
    for (; *l; l++) if ((*l)[0] == s[0] && strcmp(*l, s) == 0) return true;   // (first char inline: most entries differ there)
    return false;
}

// =============================================================================================
// StringStream (positions are byte offsets in UTF-8; lower/mixed have identical byte layout)

typedef struct {
    const char *s, *lower, *mixed;
    int len, pos;
} St;

static inline int st_cp(const St *st, int pos, int *cp)
{
    unsigned char b = (unsigned char)st->s[pos];
    if (b < 0x80) { *cp = b; return 1; }
    return u8_decode(st->s, st->len, pos, cp);
}
static inline bool is_space_cp(int cp) { return cp < 0x80 ? cp == ' ' || (cp >= 9 && cp <= 13) : js_is_space(cp); }
static bool st_eol(const St *st) { return st->pos >= st->len; }
static int st_peek(const St *st) { if (st_eol(st)) return -1; int cp; st_cp(st, st->pos, &cp); return cp; }
static void st_next(St *st) { if (!st_eol(st)) { int cp; st->pos += st_cp(st, st->pos, &cp); } }
static void st_eatspace(St *st)
{
    while (!st_eol(st)) { int cp, k = st_cp(st, st->pos, &cp); if (!is_space_cp(cp)) break; st->pos += k; }
}
static int span_space(const St *st, int p)
{
    while (p < st->len) { int cp, k = st_cp(st, p, &cp); if (!is_space_cp(cp)) break; p += k; }
    return p;
}
static int span_name(const St *st, int p)
{
    while (p < st->len) { int cp, k = st_cp(st, p, &cp); if (!js_is_namechar(cp)) break; p += k; }
    return p;
}
static bool word_boundary_after(const St *st, int p)     // \b after a word char
{
    if (p >= st->len) return true;
    int cp; st_cp(st, p, &cp);
    return !js_is_wordchar(cp);
}
static bool not_namechar_at(const St *st, int p)
{
    if (p >= st->len) return true;
    int cp; st_cp(st, p, &cp);
    return !js_is_namechar(cp);
}
static bool prefix_at(const St *st, int p, const char *w)
{
    int n = (int)strlen(w);
    return p + n <= st->len && memcmp(st->s + p, w, n) == 0;
}

// each m_* returns the end position of a match starting at pos, or -1 (stream.match semantics)
static int m_name(const St *st)            // reg_name
{
    int e = span_name(st, st->pos);
    if (e == st->pos) return -1;
    return span_space(st, e);
}
static int m_notspaceparen(const St *st)   // reg_objectname_midline / reg_match_until_commentstart_or_whitespace
{
    int p = st->pos;
    while (p < st->len) { int cp, k = st_cp(st, p, &cp); if (is_space_cp(cp) || cp == '(' || cp == ')') break; p += k; }
    if (p == st->pos) return -1;
    return span_space(st, p);
}
static int m_notcommentstart(const St *st) // reg_notcommentstart
{
    int p = st->pos;
    while (p < st->len && st->s[p] != '(') p++;
    return p == st->pos ? -1 : p;
}
static int m_alts_wb(const St *st, int p, const char *const *alts, int *which)   // (a|b|..)\b
{
    for (int i = 0; alts[i]; i++)
        if (prefix_at(st, p, alts[i]) && word_boundary_after(st, p + (int)strlen(alts[i]))) {
            if (which) *which = i;
            return p + (int)strlen(alts[i]);
        }
    return -1;
}
static int m_soundevents(const St *st) { int e = m_alts_wb(st, st->pos, soundevents, NULL); return e < 0 ? -1 : span_space(st, e); }
static int m_soundverbs(const St *st) { int e = m_alts_wb(st, st->pos, soundverbs, NULL); return e < 0 ? -1 : span_space(st, e); }
static int m_sounddirs(const St *st)
{
    int p = span_space(st, st->pos);
    for (int i = 0; sounddirs[i]; i++) {
        int n = (int)strlen(sounddirs[i]);
        if (prefix_at(st, p, sounddirs[i]) && not_namechar_at(st, p + n)) return span_space(st, p + n);
    }
    return -1;
}
static int m_soundseed(const St *st)
{
    int p = st->pos;
    while (p < st->len && st->s[p] >= '0' && st->s[p] <= '9') p++;
    if (p == st->pos || !word_boundary_after(st, p)) return -1;
    return p;
}
static int m_sectionname(const St *st, int *which)
{
    static const char *const order[] = { "objects", "collisionlayers", "legend", "sounds", "rules", "winconditions", "levels", NULL };
    if (st->pos >= st->len || !strchr("oclsrw", st->s[st->pos]) || !st->s[st->pos]) return -1;   // no section name starts here
    for (int i = 0; order[i]; i++) {
        int n = (int)strlen(order[i]);
        if (prefix_at(st, st->pos, order[i]) && not_namechar_at(st, st->pos + n)) {
            for (int k = 0; k < 7; k++) if (strcmp(ps_sectionNames[k], order[i]) == 0) *which = k;
            return span_space(st, st->pos + n);
        }
    }
    return -1;
}
static int m_color(const St *st)
{
    int p = st->pos, e = -1;
    for (int i = 0; colorregex[i]; i++) if (prefix_at(st, p, colorregex[i])) { e = p + (int)strlen(colorregex[i]); break; }
    if (e < 0 && p < st->len && st->s[p] == '#') {
        int k = 0;
        while (p + 1 + k < st->len && k < 6) {
            char ch = st->s[p + 1 + k];
            if (!((ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f'))) break;
            k++;
        }
        if (k >= 6) e = p + 7; else if (k >= 3) e = p + 4;
    }
    if (e < 0) return -1;
    return span_space(st, e);
}
static int m_legend_lhs_name(const St *st)  // [^=\p{Z}\s\(]*(\p{Z}\s)*
{
    int p = st->pos;
    while (p < st->len) { int cp, k = st_cp(st, p, &cp); if (cp == '=' || cp == '(' || is_space_cp(cp)) break; p += k; }
    while (p < st->len) {
        int cp1, k1 = st_cp(st, p, &cp1);
        if (!js_is_Z(cp1) || p + k1 >= st->len) break;
        int cp2, k2 = st_cp(st, p + k1, &cp2);
        if (!js_is_space(cp2)) break;
        p += k1 + k2;
    }
    return p;
}
static int m_rule_arrow(const St *st)
{
    int p = span_space(st, st->pos);
    if (p + 2 <= st->len && st->s[p] == '-' && st->s[p + 1] == '>') return span_space(st, p + 2);
    return -1;
}
static int m_rule_part(const St *st)
{
    int p = st->pos;
    while (p < st->len) { int cp, k = st_cp(st, p, &cp); if (cp == '[' || cp == '|' || cp == ']' || is_space_cp(cp)) break; p += k; }
    return p;
}
static int m_single_word(const St *st)
{
    int p = span_space(st, st->pos);
    int e = span_name(st, p);
    if (e == p) return -1;
    return span_space(st, e);
}
static int m_message_verb(const St *st, bool needBoundary)
{
    int p = span_space(st, st->pos);
    if (!prefix_at(st, p, "message")) return -1;
    if (needBoundary && !word_boundary_after(st, p + 7)) return -1;
    return span_space(st, p + 7);
}
static int m_csv(const St *st)
{
    int p = st->pos;
    while (p < st->len && (st->s[p] == ' ' || st->s[p] == ',')) p++;
    return p;
}

static char *st_trimmed(PSComp *c, const St *st, int from, int to) { return js_trim(&c->tmp, st->s + from, to - from); }

// =============================================================================================
// parser.js helpers

PObj *ps_obj(PSComp *c, int name) { return (PObj *)ad_get(&c->objects, name); }

static PLegend *find_legend(PVec *v, int name)
{
    for (int i = 0; i < v->n; i++) { PLegend *l = v->v[i]; if (l->name == name) return l; }
    return NULL;
}

// wordAlreadyDeclared: returns the declaration's line number via *line, false if undeclared
static bool word_declared(PSComp *c, int n, int *line)
{
    PObj *o = ps_obj(c, n);
    if (o) { if (line) *line = o->lineNumber; return true; }
    PLegend *l = find_legend(&c->legend_aggregates, n);
    if (!l) l = find_legend(&c->legend_properties, n);
    if (!l) l = find_legend(&c->legend_synonyms, n);
    if (l) { if (line) *line = l->lineNumber; return true; }
    return false;
}
bool ps_word_already_declared(PSComp *c, int name) { return word_declared(c, name, NULL); }

enum { LK_OBJECT, LK_AGGREGATE, LK_PROPERTY, LK_UNDECLARED };
// lookupLegendName
static int lookup_legend_name(PSComp *c, int n, int *outName, PLegend **entry)
{
    if (ps_obj(c, n)) { *outName = n; return LK_OBJECT; }
    PLegend *l = find_legend(&c->legend_synonyms, n);
    if (l) return lookup_legend_name(c, l->vals.v[0], outName, entry);
    l = find_legend(&c->legend_aggregates, n);
    if (l) { *outName = n; *entry = l; return LK_AGGREGATE; }
    l = find_legend(&c->legend_properties, n);
    if (l) { *outName = n; *entry = l; return LK_PROPERTY; }
    *outName = n;
    return LK_UNDECLARED;
}

// registerOriginalCaseName (only the line number part matters to the compiler)
static void register_original_case_name(PSComp *c, int candname, const char *mixedCase, int lineNumber)
{
    const char *name = ATOM_STR(c, candname);
    int n = (int)strlen(name);
    if (!n) return;
    const char *hay = js_lower(&c->tmp, mixedCase);
    int hl = (int)strlen(hay);
    for (int i = 0; i + n <= hl; i++) {
        if (memcmp(hay + i, name, n) != 0) continue;
        // \b before and after (ASCII word chars)
        bool wb1 = js_is_wordchar((unsigned char)name[0]);
        bool before = i > 0 && js_is_wordchar((unsigned char)hay[i - 1]);
        bool wb2 = js_is_wordchar((unsigned char)name[n - 1]);
        bool after = i + n < hl && js_is_wordchar((unsigned char)hay[i + n]);
        if (wb1 != before && wb2 != after) {
            ad_set(&c->original_line_numbers, candname, (void *)(intptr_t)(lineNumber + 1));
            return;
        }
    }
}

static PLegend *new_legend(PSComp *c, int name, int line)
{
    PLegend *l = ar_alloc(&c->tmp, sizeof *l);
    l->name = name; l->lineNumber = line;
    return l;
}

// substitutor used by aggregate legend entries
static void subst_aggregate(PSComp *c, int n, IVec *out, bool *ok)
{
    int name; PLegend *e = NULL;
    switch (lookup_legend_name(c, n, &name, &e)) {
    case LK_AGGREGATE:
        for (int i = 0; i < e->vals.n; i++) subst_aggregate(c, e->vals.v[i], out, ok);
        return;
    case LK_PROPERTY:
        ps_log_error(c, c->lineNumber, "Cannot define an aggregate (using 'and') in terms of properties (something that uses 'or').");
        *ok = false;
        iv_push(&c->tmp, out, name);
        return;
    default:
        iv_push(&c->tmp, out, name);
    }
}
// substitutor used by property legend entries; flattenPropertyEntry inlined
static void subst_property(PSComp *c, int n, IVec *out, int candname0)
{
    int name; PLegend *e = NULL;
    switch (lookup_legend_name(c, n, &name, &e)) {
    case LK_AGGREGATE:
        ps_log_error(c, c->lineNumber, "Cannot define a property (something defined in terms of 'or') in terms of an aggregate (something that uses 'and').  In this case, you can't define \"%S\" in terms of \"%S\".",
                     ATOM_STR(c, candname0), ATOM_STR(c, name));
        return;
    case LK_PROPERTY:
        for (int j = 0; j < e->vals.n; j++) if (e->vals.v[j] != name) subst_property(c, e->vals.v[j], out, candname0);
        return;
    default:
        iv_push(&c->tmp, out, name);
    }
}

// processLegendLine
static void process_legend_line(PSComp *c, const char *mixedCase)
{
    IVec *splits = &c->wip;
    if (splits->n == 0) return;
    bool ok = true;
    if (splits->n == 1) {
        ps_log_error(c, c->lineNumber, "Incorrect format of legend - should be one of \"A = B\", \"A = B or C [ or D ...]\", \"A = B and C [ and D ...]\".");
        ok = false;
    } else if (splits->n % 2 == 0) {
        ps_log_error(c, c->lineNumber, "Incorrect format of legend - should be one of \"A = B\", \"A = B or C [ or D ...]\", \"A = B and C [ and D ...]\", but it looks like you have a dangling \"%S\"?",
                     ATOM_STR(c, splits->v[splits->n - 1]));
        ok = false;
    } else {
        int candname = splits->v[0];
        int dline;
        if (word_declared(c, candname, &dline)) {
            ps_log_error(c, c->lineNumber, "Name \"%S\" already in use (on line line %d).", ATOM_STR(c, candname), dline);
            ok = false;
        }
        if (js_utf16_len(ATOM_STR(c, candname)) > 1 && in_list(keyword_array, ATOM_STR(c, candname)))
            ps_log_warning(c, c->lineNumber, "You named an object \"%S\", but this is a keyword. Don't do that!", ATOM_STR(c, candname));

        for (int i = 2; i < splits->n; i += 2) {
            int nname = splits->v[i];
            if (nname == candname) {
                ps_log_error(c, c->lineNumber, "You can't define object %S in terms of itself!", ATOM_STR(c, candname));
                ok = false;
                int idx = iv_index(splits, candname, 2);
                while (idx >= 2) {
                    if (idx >= 4) iv_splice(splits, idx - 1, 2);
                    else iv_splice(splits, idx, 2);
                    idx = iv_index(splits, candname, 2);
                }
            }
            for (int j = 2; j < i && j < splits->n; j += 2) {
                if (splits->v[j] == nname)
                    ps_log_warning(c, c->lineNumber, "You're repeating the object %S here multiple times on the RHS.  This makes no sense.  Don't do that.", ATOM_STR(c, nname));
            }
        }
        for (int i = 2; i < splits->n; i += 2) {
            int defname = splits->v[i];
            if (defname != candname && !word_declared(c, defname, NULL))
                ps_log_error(c, c->lineNumber, "You're talking about %S but it's not defined anywhere.", ATOM_STR(c, defname));
        }

        int a_and = atom(&c->atoms, "and"), a_or = atom(&c->atoms, "or");
        if (splits->n == 3) {
            PLegend *l = new_legend(c, splits->v[0], c->lineNumber);
            iv_push(&c->tmp, &l->vals, splits->v[2]);
            register_original_case_name(c, splits->v[0], mixedCase, c->lineNumber);
            pv_push(&c->tmp, &c->legend_synonyms, l);
        } else if (splits->n > 3 && splits->v[3] == a_and) {
            PLegend *l = new_legend(c, splits->v[0], c->lineNumber);
            subst_aggregate(c, splits->v[2], &l->vals, &ok);
            if (splits->n > 4) subst_aggregate(c, splits->v[4], &l->vals, &ok);
            for (int i = 6; i < splits->n; i += 2) subst_aggregate(c, splits->v[i], &l->vals, &ok);
            register_original_case_name(c, l->name, mixedCase, c->lineNumber);
            pv_push(&c->tmp, &c->legend_aggregates, l);
        } else if (splits->n > 3 && splits->v[3] == a_or) {
            bool malformed = false;
            for (int i = 5; i < splits->n; i += 2)
                if (strcmp(js_lower(&c->tmp, ATOM_STR(c, splits->v[i])), "or") != 0) { malformed = true; break; }
            if (!malformed) {
                PLegend *l = new_legend(c, splits->v[0], c->lineNumber);
                for (int i = 2; i < splits->n; i += 2) subst_property(c, splits->v[i], &l->vals, splits->v[0]);
                if (l->vals.n > 0) {
                    register_original_case_name(c, l->name, mixedCase, c->lineNumber);
                    pv_push(&c->tmp, &c->legend_properties, l);
                }
            }
        } else {
            if (ok) {
                ps_log_error(c, c->lineNumber, "This legend-entry is incorrectly-formatted - it should be one of A = B, A = B or C ( or D ...), A = B and C (and D ...)");
                ok = false;
            }
        }
    }
}

// processSoundsLine
static void process_sounds_line(PSComp *c)
{
    if (c->wipSndN == 0) return;
    if (c->wipSnd[c->wipSndN - 1].type == SND_ERROR) return;
    PSound *s = ar_alloc(&c->tmp, sizeof *s);
    s->n = c->wipSndN;
    s->t = ar_alloc(&c->tmp, sizeof(PSndTok) * s->n);
    memcpy(s->t, c->wipSnd, sizeof(PSndTok) * s->n);
    s->lineNumber = c->lineNumber;
    pv_push(&c->tmp, &c->sounds, s);
}
static void snd_push(PSComp *c, char *text, int type)
{
    if (c->wipSndN == c->wipSndCap) {
        int nc = c->wipSndCap ? c->wipSndCap * 2 : 8;
        PSndTok *n = ar_alloc(&c->tmp, sizeof(PSndTok) * nc);
        if (c->wipSndN) memcpy(n, c->wipSnd, sizeof(PSndTok) * c->wipSndN);
        c->wipSnd = n; c->wipSndCap = nc;
    }
    c->wipSnd[c->wipSndN].text = text;
    c->wipSnd[c->wipSndN].type = type;
    c->wipSndN++;
}

static void end_of_line_processing(PSComp *c, const char *mixedCase)
{
    if (c->section == SEC_LEGEND) process_legend_line(c, mixedCase);
    else if (c->section == SEC_SOUNDS) process_sounds_line(c);
}

static PLevel *last_level(PSComp *c) { return c->levels.v[c->levels.n - 1]; }

static void blank_line_handle(PSComp *c)
{
    if (c->section == SEC_LEVELS) {
        if (last_level(c)->type != LV_EMPTY) {
            PLevel *l = ar_alloc(&c->tmp, sizeof *l);
            pv_push(&c->tmp, &c->levels, l);
        }
    }
    if (c->section == SEC_OBJECTS) c->objects_section = 0;
}

static PVec *new_spritematrix(PSComp *c) { return ar_alloc(&c->tmp, sizeof(PVec)); }

// errorFallbackMatchToken: returns end position or -1
static int error_fallback(St *st)
{
    int e = m_notspaceparen(st);
    if (e < 0) e = m_notcommentstart(st);
    if (e >= 0) st->pos = e;
    return e;
}

// tryParseName (objects section)
static void try_parse_name(PSComp *c, St *st, bool sol, const char *mixedCase)
{
    int e = sol ? m_name(st) : m_notspaceparen(st);
    if (e < 0) {
        int e2 = m_notcommentstart(st);
        if (e2 >= 0) st->pos = e2;
        if (st->pos > 0)
            ps_log_warning(c, c->lineNumber, "Unknown junk in object section (possibly: sprites have to be 5 pixels wide and 5 pixels high exactly. Or maybe: the main names for objects have to be words containing only the letters a-z0.9 - if you want to call them something like \",\", do it in the legend section).");
        return;
    }
    char *cn = st_trimmed(c, st, st->pos, e);
    st->pos = e;
    int candname = atom(&c->atoms, cn);
    if (ps_obj(c, candname)) {
        ps_log_error(c, c->lineNumber, "Object \"%S\" defined multiple times.", cn);
        return;
    }
    for (int i = 0; i < c->legend_synonyms.n; i++) {
        PLegend *l = c->legend_synonyms.v[i];
        if (l->name == candname) ps_log_error(c, c->lineNumber, "Name \"%S\" already in use.", cn);
    }
    if ((sol || js_utf16_len(cn) > 1) && in_list(keyword_array, cn))
        ps_log_warning(c, c->lineNumber, "You named an object \"%S\", but this is a keyword. Don't do that!", cn);
    if (sol) {
        c->objects_candname = candname;
        register_original_case_name(c, candname, mixedCase, c->lineNumber);
        PObj *o = ar_alloc(&c->tmp, sizeof *o);
        o->name = candname; o->lineNumber = c->lineNumber;
        o->spritematrix = new_spritematrix(c);
        o->layer = -1; o->id = -1;
        ad_set(&c->objects, candname, o);
        pv_push(&c->tmp, &c->objectOrder, o);
    } else {
        register_original_case_name(c, candname, mixedCase, c->lineNumber);
        PLegend *l = new_legend(c, candname, c->lineNumber);
        iv_push(&c->tmp, &l->vals, c->objects_candname);
        pv_push(&c->tmp, &c->legend_synonyms, l);
    }
    c->objects_section = 1;
}

static void add_names_from_legend(PSComp *c, ADict *set, PVec *v, bool singleCharOnly)
{
    for (int i = 0; i < v->n; i++) {
        PLegend *l = v->v[i];
        if (!singleCharOnly || js_utf16_len(ATOM_STR(c, l->name)) == 1) ad_set(set, l->name, (void *)1);
    }
}

// substitutor for collision layers
static void subst_layer(PSComp *c, int n, IVec *out, const char *candname)
{
    int name; PLegend *e = NULL;
    switch (lookup_legend_name(c, n, &name, &e)) {
    case LK_OBJECT: iv_push(&c->tmp, out, name); return;
    case LK_AGGREGATE:
        ps_log_error(c, c->lineNumber, "\"%s\" is an aggregate (defined using \"and\"), and cannot be added to a single layer because its constituent objects must be able to coexist.", ATOM_STR(c, name));
        return;
    case LK_PROPERTY:
        for (int j = 0; j < e->vals.n; j++) if (e->vals.v[j] != name) subst_layer(c, e->vals.v[j], out, candname);
        return;
    default:
        ps_log_error(c, c->lineNumber, "Cannot add \"%S\" to a collision layer; it has not been declared.", candname);
    }
}

// =============================================================================================
// token()  (codeMirrorFn().token)

static void token(PSComp *c, St *st)
{
    bool sol = st->pos == 0;
    if (sol) {
        c->lineNumber++;
        c->wip.n = 0;
        c->wipSndN = 0;
        c->mixedCase = st->mixed;
        st->s = st->lower;
        c->tokenIndex = 0;
        c->line_should_end = false;
    }
    const char *mixedCase = c->mixedCase;
    if (c->sol_after_comment) { sol = true; c->sol_after_comment = false; }

    st_eatspace(st);

    // comments
    int ch = st_peek(st);
    if (ch == '(' && c->tokenIndex != -4) {
        st_next(st);
        c->commentLevel++;
    } else if (ch == ')') {
        st_next(st);
        if (c->commentLevel > 0) {
            c->commentLevel--;
            if (c->commentLevel == 0) { c->sol_after_comment = true; return; }
        } else {
            ps_log_warning(c, c->lineNumber, "You're trying to close a comment here, but I can't find any opening bracket to match it? [This is highly suspicious; you probably want to fix it.]");
            return;
        }
    }
    if (c->commentLevel > 0) {
        if (sol) c->sol_after_comment = true;
        while (true) {
            while (!st_eol(st) && st->s[st->pos] != '(' && st->s[st->pos] != ')') st->pos++;
            if (st_eol(st)) break;
            char cc = st->s[st->pos];
            if (cc == '(') c->commentLevel++;
            else if (cc == ')') c->commentLevel--;
            st_next(st);
            if (c->commentLevel == 0) break;
        }
        if (st_eol(st)) end_of_line_processing(c, mixedCase);
        return;
    }

    st_eatspace(st);

    if (sol && st_eol(st)) {
        end_of_line_processing(c, mixedCase);
        blank_line_handle(c);
        return;
    }

    if (c->line_should_end && !st_eol(st)) {
        ps_log_error(c, c->lineNumber, "Only comments should go after %s on a line.", c->line_should_end_because);
        st->pos = st->len;
        return;
    }

    bool shouldmatchequals = true;
    if (sol && c->section == SEC_LEVELS) {
        for (int p = st->pos; p < st->len;) {
            int cp, k = st_cp(st, p, &cp);
            if (cp != '=' && !js_is_space(cp)) { shouldmatchequals = false; break; }
            p += k;
        }
    }
    if (sol && shouldmatchequals && st->pos < st->len && st->s[st->pos] == '=') {
        while (st->pos < st->len && st->s[st->pos] == '=') st->pos++;
        c->line_should_end = true;
        c->line_should_end_because = "a bunch of equals signs ('===')";
        return;
    }

    if (c->section != SEC_LEVELS) {
        int which = -1;
        int e = m_sectionname(st, &which);
        if (e >= 0) st->pos = e;
        if (sol && e >= 0) {
            c->section = which;
            const char *secname = ps_sectionNames[which];
            if (iv_has(&c->visitedSections, which))
                ps_log_error(c, c->lineNumber, "cannot duplicate sections (you tried to duplicate \"%S\").", secname);
            c->line_should_end = true;
            c->line_should_end_because = tfmt(&c->tmp, "a section name (\"%S\")", secname);
            iv_push(&c->tmp, &c->visitedSections, which);
            if (which == 0) {
                c->objects_section = 0;
                if (c->visitedSections.n > 1)
                    ps_log_error(c, c->lineNumber, "section \"%S\" must be the first section", secname);
            } else if (!iv_has(&c->visitedSections, which - 1)) {
                const char *prev = ps_sectionNames[which - 1];
                ps_log_error(c, c->lineNumber, "section \"%S\" is out of order, must follow  \"%S\" (or it could be that the section \"%S\"is just missing totally.  You have to include all section headings, even if the section itself is empty).",
                             secname, prev, prev);
            }
            if (which == SEC_SOUNDS) {
                for (int i = 0; i < c->objects.order.n; i++) ad_set(&c->names, c->objects.order.v[i], (void *)1);
                add_names_from_legend(c, &c->names, &c->legend_synonyms, false);
                add_names_from_legend(c, &c->names, &c->legend_aggregates, false);
                add_names_from_legend(c, &c->names, &c->legend_properties, false);
            } else if (which == SEC_LEVELS) {
                memset(c->levelKeyOk, 0, sizeof c->levelKeyOk);
                for (int i = 0; i < c->objects.order.n; i++) {
                    int n = c->objects.order.v[i];
                    if (js_utf16_len(ATOM_STR(c, n)) == 1) ad_set(&c->abbrevNames, n, (void *)1);
                }
                add_names_from_legend(c, &c->abbrevNames, &c->legend_synonyms, true);
                add_names_from_legend(c, &c->abbrevNames, &c->legend_aggregates, true);
            }
            return;
        }
    }

    if (st_eol(st)) {
        end_of_line_processing(c, mixedCase);
        return;
    }

    switch (c->section) {
    case SEC_OBJECTS: {
        if (sol && c->objects_section == 2) c->objects_section = 3;
        if (sol && c->objects_section == 1) c->objects_section = 2;
        switch (c->objects_section) {
        case 0: case 1:
            c->objects_spritematrix = new_spritematrix(c);
            try_parse_name(c, st, sol, mixedCase);
            return;
        case 2: {
            c->tokenIndex = 0;
            int e = m_color(st);
            if (e < 0) {
                int e2 = m_name(st);
                if (e2 < 0) e2 = m_notcommentstart(st);
                const char *str = e2 >= 0 ? ar_strndup(&c->tmp, st->s + st->pos, e2 - st->pos) : "null";
                if (e2 >= 0) st->pos = e2;
                ps_log_error(c, c->lineNumber, "Was looking for color for object %S, got \"%s\" instead.", ATOM_STR(c, c->objects_candname), str);
                return;
            }
            char *col = st_trimmed(c, st, st->pos, e);
            st->pos = e;
            PObj *o = ps_obj(c, c->objects_candname);
            if (o) pv_push(&c->tmp, &o->colors, col);
            return;
        }
        case 3: for (;;) {
            int cc = st_peek(st);
            if (!(cc == '.' || (cc >= '0' && cc <= '9'))) {
                if (c->objects_spritematrix->n == 0) { try_parse_name(c, st, sol, mixedCase); return; }
                ps_log_error(c, c->lineNumber, "Unknown junk in spritematrix for object %S.", ATOM_STR(c, c->objects_candname));
                int e = m_notcommentstart(st);
                if (e >= 0) st->pos = e;
                return;
            }
            st->pos++;
            PVec *sm = c->objects_spritematrix;
            if (sol || sm->n == 0) pv_push(&c->tmp, sm, ar_strdup(&c->tmp, ""));
            PObj *o = ps_obj(c, c->objects_candname);
            char *row = sm->v[sm->n - 1];
            int rl = (int)strlen(row);
            char *nr = ar_alloc_raw(&c->tmp, rl + 2);
            memcpy(nr, row, rl); nr[rl] = (char)cc; nr[rl + 1] = 0;
            sm->v[sm->n - 1] = nr;
            if (rl + 1 > 5) {
                ps_log_warning(c, c->lineNumber, "Sprites must be 5 wide and 5 high.");
                int e = m_notcommentstart(st);
                if (e >= 0) st->pos = e;
                return;
            }
            if (o) o->spritematrix = sm;
            if (sm->n == 5 && rl + 1 == 5) c->objects_section = 0;
            if (cc != '.' && o) {
                int n = cc - '0';
                if (n >= o->colors.n) {
                    ps_log_error(c, c->lineNumber, "Trying to access color number %d from the color palette of sprite %S, but there are only %d defined in it.",
                                 n, ATOM_STR(c, c->objects_candname), o->colors.n);
                    return;
                }
            }
            // One pixel per token, as in JS; the next pixel of the row is handled right here when
            // a fresh token() call would come straight back to this point (no space, no comment).
            if (st_eol(st)) return;
            unsigned char nb = (unsigned char)st->s[st->pos];
            if (!(nb == '.' || (nb >= '0' && nb <= '9')) || c->objects_section != 3 || c->line_should_end || c->sol_after_comment || c->commentLevel)
                return;
            sol = false;
        }
        }
        break;
    }
    case SEC_LEGEND: {
        bool pushed = false; int pushAtom = -1;
        if (c->tokenIndex == 0) {
            int e = m_legend_lhs_name(st);
            char *nn = st_trimmed(c, st, st->pos, e);
            st->pos = e;
            c->tokenIndex++;
            pushed = true; pushAtom = atom(&c->atoms, nn);
        } else if (c->tokenIndex == 1) {
            if (st->s[st->pos] != '=') {
                ps_log_error(c, c->lineNumber, "In the legend, define new items using the equals symbol - declarations must look like \"A = B\", \"A = B or C [ or D ...]\", \"A = B and C [ and D ...]\".");
                int e = m_notcommentstart(st);
                if (e >= 0) st->pos = e;
                pushAtom = atom(&c->atoms, "ERROR");
            } else {
                st->pos++;
                pushAtom = atom(&c->atoms, "=");
            }
            st->pos = span_space(st, st->pos);
            c->tokenIndex++;
            pushed = true;
        } else if (c->tokenIndex >= 3 && (c->tokenIndex % 2) == 1) {
            int e = m_name(st);
            if (e < 0) {
                ps_log_error(c, c->lineNumber, "Something bad's happening in the LEGEND");
                int e2 = m_notcommentstart(st);
                if (e2 >= 0) st->pos = e2;
            } else {
                char *cn = st_trimmed(c, st, st->pos, e);
                st->pos = e;
                if (strcmp(cn, "and") == 0 || strcmp(cn, "or") == 0) {
                    if (c->tokenIndex >= 5 && c->wip.n > 3 && atom(&c->atoms, cn) != c->wip.v[3])
                        ps_log_error(c, c->lineNumber, "Hey! You can't go mixing ANDs and ORs in a single legend entry.");
                } else {
                    ps_log_error(c, c->lineNumber, "Expected and 'AND' or an 'OR' here, but got %S instead. In the legend, define new items using the equals symbol - declarations must look like 'A = B' or 'A = B and C' or 'A = B or C'.", cn);
                }
                pushed = true; pushAtom = atom(&c->atoms, cn);
            }
            c->tokenIndex++;
        } else {
            int e = m_name(st);
            if (e < 0) {
                ps_log_error(c, c->lineNumber, "Something bad's happening in the LEGEND");
                int e2 = m_notcommentstart(st);
                if (e2 >= 0) st->pos = e2;
            } else {
                char *cn = st_trimmed(c, st, st->pos, e);
                st->pos = e;
                pushed = true; pushAtom = atom(&c->atoms, cn);
                c->tokenIndex++;
            }
        }
        if (pushed) iv_push(&c->tmp, &c->wip, pushAtom);
        if (st_eol(st)) process_legend_line(c, mixedCase);
        return;
    }
    case SEC_SOUNDS: {
        if (c->wipSndN > 0 && c->wipSnd[c->wipSndN - 1].type == SND_ERROR) {
            int e = m_soundevents(st);
            if (e < 0) e = m_soundverbs(st);
            if (e < 0) e = m_sounddirs(st);
            if (e < 0) { int e2 = m_soundseed(st); if (e2 >= 0) st->pos = e2; }
            if (e < 0) e = m_name(st);
            if (e >= 0) st->pos = e;
            else error_fallback(st);
        } else if (c->wipSndN == 0) {
            int e = m_soundevents(st);
            if (e < 0) {
                e = m_name(st);
                if (e < 0) {
                    error_fallback(st);
                    snd_push(c, NULL, SND_ERROR);
                    ps_log_warning(c, c->lineNumber, "Was expecting a sound event (like SFX3, or ENDLEVEL) or an object name, but didn't find either.");
                } else {
                    char *nm = st_trimmed(c, st, st->pos, e);
                    st->pos = e;
                    if (!word_declared(c, atom(&c->atoms, nm), NULL)) {
                        snd_push(c, NULL, SND_ERROR);
                        ps_log_error(c, c->lineNumber, "unexpected sound token \"%s\".", nm);
                    } else {
                        snd_push(c, nm, SND_NAME);
                        c->tokenIndex++;
                    }
                }
            } else {
                snd_push(c, st_trimmed(c, st, st->pos, e), SND_SOUNDEVENT);
                st->pos = e;
                c->tokenIndex++;
            }
        } else if (c->wipSndN == 1) {
            bool is_soundevent = c->wipSnd[0].type == SND_SOUNDEVENT;
            if (is_soundevent) {
                int e = m_soundseed(st);
                if (e >= 0) {
                    snd_push(c, st_trimmed(c, st, st->pos, e), SND_SOUND);
                    st->pos = e;
                    c->tokenIndex++;
                } else {
                    error_fallback(st);
                    ps_log_error(c, c->lineNumber, "Was expecting a sound seed here (a number like 123123, like you generate by pressing the buttons above the console panel), but found something else.");
                    snd_push(c, NULL, SND_ERROR);
                }
            } else {
                int e = m_soundverbs(st);
                if (e >= 0) {
                    snd_push(c, st_trimmed(c, st, st->pos, e), SND_SOUNDVERB);
                    st->pos = e;
                    c->tokenIndex++;
                } else {
                    error_fallback(st);
                    ps_log_error(c, c->lineNumber, "Was expecting a soundverb here (MOVE, DESTROY, CANTMOVE, or the like), but found something else.");
                    snd_push(c, NULL, SND_ERROR);
                }
            }
        } else {
            bool is_soundevent = c->wipSnd[0].type == SND_SOUNDEVENT;
            PSndTok *last = &c->wipSnd[c->wipSndN - 1];
            if (is_soundevent || last->type == SND_SOUND) {
                int p0 = st->pos;
                int e = error_fallback(st);
                char *m = e >= 0 ? st_trimmed(c, st, p0, e) : "";
                ps_log_error(c, c->lineNumber, "I wasn't expecting anything after the sound declaration %S on this line, so I don't know what to do with \"%S\" here.", last->text, m);
                snd_push(c, NULL, SND_ERROR);
            } else {
                const char *verb = c->wipSnd[1].text;
                bool directional = strcmp(verb, "move") == 0 || strcmp(verb, "cantmove") == 0;
                int e = directional ? m_sounddirs(st) : -1;
                if (e >= 0) {
                    snd_push(c, st_trimmed(c, st, st->pos, e), SND_DIRECTION);
                    st->pos = e;
                    c->tokenIndex++;
                } else {
                    e = m_soundseed(st);
                    if (e >= 0) {
                        snd_push(c, st_trimmed(c, st, st->pos, e), SND_SOUND);
                        st->pos = e;
                        c->tokenIndex++;
                    } else {
                        int p0 = st->pos;
                        int e2 = error_fallback(st);
                        char *m = e2 >= 0 ? st_trimmed(c, st, p0, e2) : "";
                        if (directional)
                            ps_log_error(c, c->lineNumber, "Ah I was expecting direction or a sound seed here after %S, but I don't know what to make of \"%S\".", last->text, m);
                        else
                            ps_log_error(c, c->lineNumber, "Ah I was expecting a sound seed here after %S, but I don't know what to make of \"%S\".", last->text, m);
                        snd_push(c, NULL, SND_ERROR);
                    }
                }
            }
        }
        if (st_eol(st)) process_sounds_line(c);
        return;
    }
    case SEC_COLLISIONLAYERS: {
        if (sol) {
            IVec *layer = ar_alloc(&c->tmp, sizeof(IVec));
            pv_push(&c->tmp, &c->collisionLayers, layer);
            c->wip.n = 0;
            c->tokenIndex = 0;
        }
        int e = m_name(st);
        if (e < 0) {
            int prepos = st->pos;
            st->pos = m_csv(st);
            if (st->pos == prepos) {
                int cp = st_peek(st);
                char buf[5] = { 0 };
                u8_encode((uint32_t)cp, buf);
                ps_log_error(c, c->lineNumber, "error detected - unexpected character %s", buf);
                st_next(st);
            }
            return;
        }
        char *cn = st_trimmed(c, st, st->pos, e);
        st->pos = e;
        int candname = atom(&c->atoms, cn);
        if (strcmp(cn, "background") == 0) {
            if (c->collisionLayers.n > 0 && ((IVec *)c->collisionLayers.v[c->collisionLayers.n - 1])->n > 0)
                ps_log_error(c, c->lineNumber, "Background must be in a layer by itself.");
            c->tokenIndex = 1;
        } else if (c->tokenIndex != 0) {
            ps_log_error(c, c->lineNumber, "Background must be in a layer by itself.");
        }
        IVec ar = { 0, 0, NULL };
        subst_layer(c, candname, &ar, cn);
        if (c->collisionLayers.n == 0) {
            ps_log_error(c, c->lineNumber, "no layers found.");
            return;
        }
        IVec foundOthers = { 0, 0, NULL };
        int nl = c->collisionLayers.n;
        for (int i = 0; i < ar.n; i++)
            for (int j = 0; j < nl; j++)
                if (iv_has(c->collisionLayers.v[j], ar.v[i]) && j != nl - 1) iv_push(&c->tmp, &foundOthers, j);
        if (foundOthers.n > 0) {
            SB b; sb_init(&b, &c->tmp);
            sb_printf(&b, "Object \"%S\" included in multiple collision layers ( layers ", cn);
            for (int i = 0; i < foundOthers.n; i++) sb_printf(&b, "#%d, ", foundOthers.v[i] + 1);
            sb_printf(&b, "#%d", nl);
            ps_log_warning(c, c->lineNumber, "%s ). You should fix this! (The compiler will just pick one layer to assign it to).", sb_done(&b));
        }
        if (iv_has(&c->wip, candname))
            ps_log_warning(c, c->lineNumber, "Object \"%S\" included explicitly multiple times in the same layer. Don't do that innit.", cn);
        iv_push(&c->tmp, &c->wip, candname);
        IVec *last = c->collisionLayers.v[nl - 1];
        for (int i = 0; i < ar.n; i++) iv_push(&c->tmp, last, ar.v[i]);
        return;
    }
    case SEC_RULES: {
        if (sol) {
            PRuleLine *r = ar_alloc(&c->tmp, sizeof *r);
            // reg_notcommentstart.exec(stream.string)[0]: first run of non-'(' in the whole line
            int b = 0;
            while (b < st->len && st->s[b] == '(') b++;
            int e = b;
            while (e < st->len && st->s[e] != '(') e++;
            r->rule = ar_strndup(&c->tmp, st->s + b, e - b);
            r->lineNumber = c->lineNumber;
            r->mixed = (char *)mixedCase;
            pv_push(&c->tmp, &c->rules, r);
            c->tokenIndex = 0;
            c->inside_cell = false; c->arrow_passed = false; c->lhs_rhs_rowcount_difference = 0;
            c->rhs_pattern_started = false; c->rule_prelude = true;
        }
        if (c->tokenIndex == -4) { st->pos = st->len; return; }
        int e = m_rule_arrow(st);
        if (e >= 0) { st->pos = e; c->arrow_passed = true; return; }
        int cc = st_peek(st);
        if (cc == '[' || cc == '|' || cc == ']' || cc == '+') {
            if (cc == '[') {
                c->inside_cell = true; c->rule_prelude = false;
                if (c->arrow_passed) c->rhs_pattern_started = true;
            } else if (cc == ']') {
                c->inside_cell = false;
                if (c->arrow_passed) c->lhs_rhs_rowcount_difference--; else c->lhs_rhs_rowcount_difference++;
            }
            if (cc != '+') c->tokenIndex = 1;
            st_next(st);
            st->pos = span_space(st, st->pos);
            return;
        }
        e = m_rule_part(st);
        char *m = st_trimmed(c, st, st->pos, e);
        st->pos = e;
        if (c->tokenIndex == 0 && (strcmp(m, "startloop") == 0 || strcmp(m, "endloop") == 0)) return;
        if (c->tokenIndex == 0 && in_list(ruledirectionindicators, m)) { st->pos = span_space(st, st->pos); return; }
        if (c->tokenIndex == 1 && in_list(directions_tok, m)) { st->pos = span_space(st, st->pos); return; }
        int am = atom_find(&c->atoms, m);
        if (am >= 0 && ad_has(&c->names, am)) {
            if (sol) {
                ps_log_error(c, c->lineNumber, "Objects cannot appear outside of square brackets in rules, only directions can.");
                return;
            }
            st->pos = span_space(st, st->pos);
            return;
        }
        if (strcmp(m, "...") == 0 || strcmp(m, "rigid") == 0 || strcmp(m, "random") == 0) return;
        if (in_list(commandwords, m)) {
            if (strcmp(m, "message") == 0) c->tokenIndex = -4;
            return;
        }
        ps_log_error(c, c->lineNumber, "Name \"%s\", referred to in a rule, does not exist.", m);
        if (e == st->pos && m[0] == 0) st_next(st);   // guarantee progress
        return;
    }
    case SEC_WINCONDITIONS: {
        if (sol) {
            PWin *w = ar_alloc(&c->tmp, sizeof *w);
            int b = 0;
            while (b < st->len && st->s[b] == '(') b++;
            int e = b;
            while (e < st->len && st->s[e] != '(') e++;
            // split on [\p{Z}\s], drop empties
            PVec words = { 0, 0, NULL };
            int p = b;
            while (p < e) {
                int cp, k = st_cp(st, p, &cp);
                if (js_is_space(cp)) { p += k; continue; }
                int q = p;
                while (q < e) { int cp2, k2 = st_cp(st, q, &cp2); if (js_is_space(cp2)) break; q += k2; }
                pv_push(&c->tmp, &words, ar_strndup(&c->tmp, st->s + p, q - p));
                p = q;
            }
            w->n = words.n; w->w = (char **)words.v; w->lineNumber = c->lineNumber;
            pv_push(&c->tmp, &c->winconditions, w);
            c->tokenIndex = -1;
        }
        c->tokenIndex++;
        int e = m_single_word(st);
        if (e < 0) {
            ps_log_error(c, c->lineNumber, "incorrect format of win condition.");
            int e2 = m_notcommentstart(st);
            if (e2 >= 0) st->pos = e2;
            return;
        }
        char *w = st_trimmed(c, st, st->pos, e);
        st->pos = e;
        if (c->tokenIndex == 0) {
            if (!(strcmp(w, "all") == 0 || strcmp(w, "any") == 0 || strcmp(w, "no") == 0 || strcmp(w, "some") == 0))
                ps_log_error(c, c->lineNumber, "Expecting the start of a win condition (\"ALL\",\"SOME\",\"NO\") but got \"%S'.", w);
        } else if (c->tokenIndex == 2) {
            if (strcmp(w, "on") != 0) ps_log_error(c, c->lineNumber, "Expecting the word \"ON\" but got \"%S\".", w);
        } else if (c->tokenIndex == 1 || c->tokenIndex == 3) {
            int a = atom_find(&c->atoms, w);
            if (a < 0 || !ad_has(&c->names, a))
                ps_log_error(c, c->lineNumber, "Error in win condition: \"%S\" is not a valid object name.", w);
        } else {
            ps_log_error(c, c->lineNumber, "Error in win condition: I don't know what to do with %S.", w);
        }
        return;
    }
    case SEC_LEVELS: {
        if (sol) {
            int e = m_message_verb(st, true);
            bool message_match = e >= 0;
            if (e >= 0) st->pos = e;
            if (!message_match) {
                int e2 = m_message_verb(st, false);
                if (e2 >= 0) {
                    st->pos = e2;
                    ps_log_warning(c, c->lineNumber, "You probably meant to put a space after 'message' innit.  That's ok, I'll still interpret it as a message, but you probably want to put a space there.");
                    message_match = true;
                }
            }
            if (message_match) {
                c->tokenIndex = -4;
                PLevel *l = ar_alloc(&c->tmp, sizeof *l);
                l->type = LV_MESSAGE;
                l->message = js_trim(&c->tmp, mixedCase + st->pos, (int)strlen(mixedCase + st->pos));
                l->lineNumber = c->lineNumber;
                if (last_level(c)->type == LV_EMPTY) {
                    pv_push(&c->tmp, &c->levels, NULL);
                    c->levels.v[c->levels.n - 1] = c->levels.v[c->levels.n - 2];
                    c->levels.v[c->levels.n - 2] = l;
                } else {
                    pv_push(&c->tmp, &c->levels, l);
                }
                return;
            }
            int e3 = m_notcommentstart(st);
            if (e3 < 0) {
                ps_log_error(c, c->lineNumber, "Detected a comment where I was expecting a level. Oh gosh; if this is to do with you using '(' as a character in the legend, please don't do that ^^");
                c->commentLevel++;
                st->pos = st->len;
                return;
            }
            char *line = st_trimmed(c, st, st->pos, e3);
            c->tokenIndex = 2;
            PLevel *ll = last_level(c);
            if (ll->type == LV_MESSAGE) {
                PLevel *l = ar_alloc(&c->tmp, sizeof *l);
                l->type = LV_LEVEL; l->lineNumber = c->lineNumber;
                pv_push(&c->tmp, &l->rows, line);
                pv_push(&c->tmp, &c->levels, l);
            } else {
                if (ll->type == LV_EMPTY) { ll->type = LV_LEVEL; ll->lineNumber = c->lineNumber; }
                pv_push(&c->tmp, &ll->rows, line);
                if (js_utf16_len(line) != js_utf16_len(ll->rows.v[0]))
                    ps_log_warning(c, c->lineNumber, "Maps must be rectangular, yo (In a level, the length of each row must be the same).");
            }
        } else {
            if (c->tokenIndex == -4) { st->pos = st->len; return; }
        }
        if (c->tokenIndex == 2 && !st_eol(st)) {
            // One map character per token, as in JS. Runs of plain ASCII characters whose key is
            // known to be fine are consumed here in one go: re-entering token() for each would
            // only skip no whitespace and see no comment brackets before getting back here.
            for (;;) {
                int cp, k = st_cp(st, st->pos, &cp);
                if (cp > 0 && cp < 128 && c->levelKeyOk[cp]) {
                    st->pos += k;
                } else {
                    char *chs = ar_strndup(&c->tmp, st->s + st->pos, k);
                    st->pos += k;
                    int a = atom_find(&c->atoms, chs);
                    if (a < 0 || !ad_has(&c->abbrevNames, a)) {
                        ps_log_error(c, c->lineNumber, "Key \"%S\" not found. Do you need to add it to the legend, or define a new object?", chs);
                        return;
                    }
                    if (cp > 0 && cp < 128) c->levelKeyOk[cp] = true;
                }
                if (st_eol(st)) return;
                unsigned char b = (unsigned char)st->s[st->pos];
                if (b <= ' ' || b >= 0x7f || b == '(' || b == ')' || c->line_should_end || c->sol_after_comment || c->commentLevel)
                    return;
            }
        }
        break;
    }
    default: {   // preamble
        if (sol) c->tokenIndex = 0;
        if (c->tokenIndex == 0) {
            int e = m_single_word(st);
            if (e >= 0) {
                char *tok = st_trimmed(c, st, st->pos, e);
                st->pos = e;
                if (sol) {
                    if (in_list(metadata_with_value, tok)) {
                        if (strcmp(tok, "author") == 0 || strcmp(tok, "homepage") == 0 || strcmp(tok, "title") == 0)
                            st->s = st->mixed;
                        if (strcmp(tok, "youtube") == 0)
                            ps_log_warning(c, c->lineNumber, "Unfortunately, YouTube support hasn't been working properly for a long time - it was always a hack and it hasn't gotten less hacky over time, so I can no longer pretend to support it.");
                        int e2 = m_notcommentstart(st);
                        if (e2 >= 0) {
                            PMeta *m = ar_alloc(&c->tmp, sizeof *m);
                            m->key = tok; m->val = st_trimmed(c, st, st->pos, e2);
                            pv_push(&c->tmp, &c->metadata, m);
                            int ka = atom(&c->atoms, tok);
                            if (ad_has(&c->metadata_lines, ka))
                                ps_log_warning(c, c->lineNumber, "You've already defined a %S in the prelude on line %d.", tok,
                                               (int)(intptr_t)ad_get(&c->metadata_lines, ka));
                            ad_set(&c->metadata_lines, ka, (void *)(intptr_t)c->lineNumber);
                        } else {
                            ps_log_error(c, c->lineNumber, "MetaData \"%s\" needs a value.", tok);
                        }
                        c->tokenIndex = 1;
                        return;
                    } else if (in_list(metadata_no_value, tok)) {
                        PMeta *m = ar_alloc(&c->tmp, sizeof *m);
                        m->key = tok; m->val = "true";
                        pv_push(&c->tmp, &c->metadata, m);
                        c->tokenIndex = -1;
                        int e2 = m_notcommentstart(st);
                        if (e2 >= 0)
                            ps_log_warning(c, c->lineNumber, "MetaData %S doesn't take any parameters, but you went and gave it \"%s\".", tok, st_trimmed(c, st, st->pos, e2));
                        return;
                    } else {
                        ps_log_error(c, c->lineNumber, "Unrecognised stuff in the prelude.");
                        return;
                    }
                }
                return;
            } else {
                ps_log_error(c, c->lineNumber, "Unrecognised stuff \"%s\" in the prelude.", st->s);
            }
        } else {
            int e = m_notcommentstart(st);
            if (e >= 0) st->pos = e;
            c->tokenIndex++;
            if (c->tokenIndex > 2) {
                ps_log_warning(c, c->lineNumber, "Error: you can't embed comments in metadata values. Anything after the comment will be ignored.");
                return;
            }
            return;
        }
        break;
    }
    }
    st_next(st);
}

// loadFile's tokenizing loop. returns false when compilation was aborted (too many errors).
#define MAX_ERRORS 5
#if defined(PS_RULE_PROFILE) && defined(TARGET_PLAYDATE)
void ps_prof_begin(void);
void ps_prof_end(int id, bool matched);
#define PARSE_PROF_BEGIN() ps_prof_begin()
#define PARSE_PROF_END(id) ps_prof_end(8200 + (id), true)
#else
#define PARSE_PROF_BEGIN() do { } while (0)
#define PARSE_PROF_END(id) do { } while (0)
#endif
bool ps_parse_source(PSComp *c, const char *src, size_t len)
{
    c->section = SEC_PRELUDE;
    c->objects_spritematrix = new_spritematrix(c);
    pv_push(&c->tmp, &c->levels, ar_alloc(&c->tmp, sizeof(PLevel)));
    size_t p = 0;
    while (true) {
        size_t e = p;
        while (e < len && src[e] != '\n') e++;
        St st;
        PARSE_PROF_BEGIN();
        st.mixed = ar_strndup(&c->tmp, src + p, e - p);
        st.lower = js_lower(&c->tmp, st.mixed);
        st.s = st.lower;
        st.len = (int)(e - p);
        st.pos = 0;
        PARSE_PROF_END(10);
        PARSE_PROF_BEGIN();
        int sec0 = c->section; (void)sec0;
        do {
            int before = st.pos;
            token(c, &st);
            if (c->errorCount > MAX_ERRORS) {
                if (ps_log_hook) ps_log_hook("too many errors, aborting compilation");
                return false;
            }
            if (st.pos == before && st.pos == 0 && !st_eol(&st)) st_next(&st);   // never re-enter sol on the same line
        } while (!st_eol(&st));
        PARSE_PROF_END(sec0 + 1);
        if (e >= len) break;
        p = e + 1;
    }
    return true;
}
