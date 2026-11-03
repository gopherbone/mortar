// SPDX-License-Identifier: MIT
// rng.js port: RC4-based seedable RNG. Must match the JS bit-for-bit.
#include "ps.h"
#include <string.h>

#ifdef __clang__
#pragma STDC FP_CONTRACT OFF
#endif

static uint32_t rng_entropy = 0x9e3779b9u;
static uint32_t rng_counter = 0;

void ps_rng_set_entropy(uint32_t e) { rng_entropy ^= e; }

// Decode one UTF-8 sequence; invalid bytes become U+FFFD (like TextDecoder).
static uint32_t utf8_next(const unsigned char **pp)
{
    const unsigned char *p = *pp;
    uint32_t c = *p++;
    int n = 0;
    uint32_t min = 0;
    if (c < 0x80) { *pp = p; return c; }
    else if ((c & 0xE0) == 0xC0) { n = 1; c &= 0x1F; min = 0x80; }
    else if ((c & 0xF0) == 0xE0) { n = 2; c &= 0x0F; min = 0x800; }
    else if ((c & 0xF8) == 0xF0) { n = 3; c &= 0x07; min = 0x10000; }
    else { *pp = p; return 0xFFFD; }
    for (int k = 0; k < n; k++) {
        if ((*p & 0xC0) != 0x80) { *pp = p; return 0xFFFD; }
        c = (c << 6) | (*p++ & 0x3F);
    }
    *pp = p;
    if (c < min || c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) return 0xFFFD;
    return c;
}

// String.prototype.getBytes on one UTF-16 code unit: big-endian, no leading zero bytes.
static int unit_bytes(uint32_t u, uint8_t *out)
{
    if (u > 0xFF) { out[0] = (uint8_t)(u >> 8); out[1] = (uint8_t)u; return 2; }
    out[0] = (uint8_t)u;
    return 1;
}

// String.prototype.getBytes (UTF-8 input converted to UTF-16 code units first).
// Returns the byte count; writes at most cap bytes (only the first 256 matter to mix()).
static size_t seed_bytes(const char *seed, uint8_t *out, size_t cap)
{
    const unsigned char *p = (const unsigned char *)seed;
    size_t n = 0;
    while (*p && n < cap) {
        uint32_t c = utf8_next(&p);
        uint8_t b[2];
        int k;
        if (c >= 0x10000) {
            c -= 0x10000;
            k = unit_bytes(0xD800 + (c >> 10), b);
            for (int q = 0; q < k && n < cap; q++) out[n++] = b[q];
            k = unit_bytes(0xDC00 + (c & 0x3FF), b);
        } else {
            k = unit_bytes(c, b);
        }
        for (int q = 0; q < k && n < cap; q++) out[n++] = b[q];
    }
    return n;
}

// RC4 constructor + RC4.prototype.mix
static void rc4_init(PSRng *r, const uint8_t *in, size_t len)
{
    for (int i = 0; i < 256; i++) r->s[i] = (uint8_t)i;
    r->i = 0;
    r->j = 0;
    if (len == 0) return;
    int j = 0;
    for (int i = 0; i < 256; i++) {
        j = (j + r->s[i] + in[(size_t)i % len]) & 255;
        uint8_t t = r->s[i]; r->s[i] = r->s[j]; r->s[j] = t;
    }
}

// RNG constructor. NULL seed: JS uses (Math.random()+Date.now()).toString(); we build a
// varying string from a counter and ps_rng_set_entropy() instead.
void ps_rng_init(PSRng *r, const char *seed)
{
    char buf[32];
    if (seed == NULL) {
        uint32_t x = rng_entropy + 0x6D2B79F5u * ++rng_counter;
        x ^= x >> 15; x *= 0x2C1B3C6Du; x ^= x >> 12; x *= 0x297A2D39u; x ^= x >> 15;
        int n = 0;
        char tmp[12];
        do { tmp[n++] = (char)('0' + x % 10); x /= 10; } while (x);
        for (int k = 0; k < n; k++) buf[k] = tmp[n - 1 - k];
        buf[n++] = '.';
        buf[n++] = '5';
        buf[n] = 0;
        seed = buf;
    }
    // JS: an empty string seed leaves _state null (would throw on use); we use unmixed RC4.
    uint8_t bytes[256];
    size_t len = seed_bytes(seed, bytes, sizeof bytes);
    rc4_init(r, bytes, len);
}

// RC4.prototype.next / RNG.prototype.nextByte
int ps_rng_next_byte(PSRng *r)
{
    r->i = (r->i + 1) & 255;
    r->j = (r->j + r->s[r->i]) & 255;
    uint8_t t = r->s[r->i]; r->s[r->i] = r->s[r->j]; r->s[r->j] = t;
    return r->s[(r->s[r->i] + r->s[r->j]) & 255];
}

// The 7 nextByte() calls of uniform() as one integer (big-endian), with the RC4 indices in registers.
static uint64_t rng_bits56(PSRng *r)
{
    uint8_t *S = r->s;
    unsigned i = (unsigned)r->i, j = (unsigned)r->j;
    uint32_t hi = 0, lo = 0;   // 24 + 32 bits: no 64-bit shifts in the loop
    for (int k = 0; k < 7; k++) {
        i = (i + 1) & 255;
        unsigned si = S[i];
        j = (j + si) & 255;
        unsigned sj = S[j];
        S[i] = (uint8_t)sj; S[j] = (uint8_t)si;
        hi = (hi << 8) | (lo >> 24);
        lo = (lo << 8) | S[(si + sj) & 255];
    }
    r->i = (int)i; r->j = (int)j;
    return ((uint64_t)hi << 32) | lo;
}

// RNG.prototype.uniform
double ps_rng_uniform(PSRng *r)
{
    // JS: output = output*256 + byte, 7 times, in doubles. Every step but the last is exact (< 2^48);
    // the last rounds the exact 56-bit value once, which is what converting the exact integer does.
    // Integer accumulation avoids 14 double ops (soft-float on the Playdate's single-precision FPU).
    uint64_t output = rng_bits56(r);
    // Math.pow(2, 56) - 1 rounds to exactly 2^56 as a double.
    return (double)output * (1.0 / 72057594037927936.0);
}

// Math.floor(RNG.prototype.uniform() * n) without double arithmetic (soft-float on the device).
// Bit-exact: the double result is round53(round53(X) * n) / 2^56 for the 56-bit integer X uniform()
// builds, so emulate both roundings (to nearest, ties to even) in integers. Needs n < 256 so the
// product fits 64 bits; other n take the double path.
static uint64_t round53(uint64_t v)
{
    int bits = 64 - __builtin_clzll(v | 1);
    if (bits <= 53) return v;
    int sh = bits - 53;
    uint64_t rem = v & ((1ull << sh) - 1), half = 1ull << (sh - 1);
    v >>= sh;
    if (rem > half || (rem == half && (v & 1))) v++;
    return v << sh;
}
int ps_rng_index(PSRng *r, uint32_t n)
{
    uint64_t output = rng_bits56(r);
    if (n == 0 || n >= 256) {
        double u = (double)output * (1.0 / 72057594037927936.0) * (double)n;
        double f = (double)(long long)u;
        return (int)(f > u ? f - 1.0 : f);
    }
    // round53(output) is within 4 of output and the product's rounding within 2^10, so unless
    // output*n lies within that much of a multiple of 2^56 the result is floor(output*n / 2^56).
    uint64_t p = output * n;
    uint64_t rem = p & ((1ull << 56) - 1);
    if (rem > 4096 && rem < (1ull << 56) - 4096) return (int)(p >> 56);
    return (int)(round53(round53(output) * n) >> 56);
}

// RNG.prototype.random(n, m): integer in [n, m)
int ps_rng_random(PSRng *r, int n, int m)
{
    double u = ps_rng_uniform(r) * (double)(m - n);
    double f = (double)(long long)u;
    if (f > u) f -= 1.0;   // Math.floor
    return n + (int)f;
}
