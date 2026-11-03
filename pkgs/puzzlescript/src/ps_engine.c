// SPDX-License-Identifier: MIT
// ps_engine.c: port of engine.js (game logic) and the non-DOM parts of inputoutput.js
// (checkKey/update), plus convertLevelToString/soundHistory from debug.js.
// JS globals live in PSSession. Function comments name the JS function each one ports.
#include "ps.h"
#include <string.h>
#include <math.h>

#ifndef PS_UNDO_MAX_BYTES        // cap on undo history memory; 0 = unlimited (as in JS)
#ifdef TARGET_PLAYDATE
#define PS_UNDO_MAX_BYTES (3u << 20)
#else
#define PS_UNDO_MAX_BYTES 0u
#endif
#endif
#ifndef PS_SOUND_HISTORY_MAX     // soundHistory is only for tests; keep it bounded on device
#ifdef TARGET_PLAYDATE
#define PS_SOUND_HISTORY_MAX 256
#else
#define PS_SOUND_HISTORY_MAX 0x7fffffff
#endif
#endif
#define PS_SOUND_QUEUE_MAX 32
#define TL_MAX 80                // max code points per terminal line we keep
#define TI_ROWS 16               // titleImage rows we keep (JS makes 13 or 14)

// ---------------------------------------------------------------------------------------------
// small helpers

#if defined(__GNUC__) || defined(__clang__)
#define ALWAYS_INLINE inline __attribute__((always_inline))
#else
#define ALWAYS_INLINE inline
#endif

static void *xalloc(size_t n) { return ps_malloc(n); }
static void *xrealloc(void *p, size_t n) { return ps_realloc(p, n ? n : 1); }

#define VEC_PUSH(arr, len, cap, val) do { \
    if ((len) >= (cap)) { int nc_ = (cap) ? (cap) * 2 : 16; \
        (arr) = xrealloc((arr), sizeof(*(arr)) * (size_t)nc_); (cap) = nc_; } \
    (arr)[(len)++] = (val); } while (0)

static inline bool bits_set_in(const uint32_t *m, const uint32_t *arr, int n)   // bitsSetInArray
{
    for (int i = 0; i < n; i++) if ((m[i] & arr[i]) != m[i]) return false;
    return true;
}
static inline bool any_common(const uint32_t *a, const uint32_t *b, int n)       // anyBitsInCommon
{
    for (int i = 0; i < n; i++) if (a[i] & b[i]) return true;
    return false;
}
static inline bool is_zero(const uint32_t *a, int n)
{
    for (int i = 0; i < n; i++) if (a[i]) return false;
    return true;
}
// Heap stores are slow on the device (write-through external SDRAM, ~100ns each) while reads
// that hit the cache are cheap, so hot paths store a heap word only when its value changes.
static inline void put_words(uint32_t *dst, const uint32_t *src, int n)
{
    for (int i = 0; i < n; i++) if (dst[i] != src[i]) dst[i] = src[i];
}
static ALWAYS_INLINE void or_word(uint32_t *dst, uint32_t v) { if (v & ~*dst) *dst |= v; }
static inline void zero_words(uint32_t *dst, int n) { for (int i = 0; i < n; i++) if (dst[i]) dst[i] = 0; }

static inline bool get_bit(const uint32_t *a, int idx) { return (a[idx >> 5] >> (idx & 31)) & 1u; }

// BitVec.ishiftor (out-of-range words are ignored, like typed-array writes in JS)
static inline void ishiftor(uint32_t *v, int n, uint32_t mask, int shift)
{
    int inner = shift & 31, outer = shift >> 5;
    if (outer < n) v[outer] |= mask << inner;
    if (inner > 27 && outer + 1 < n) v[outer + 1] |= (uint32_t)((int32_t)mask >> (32 - inner));
}
// BitVec.getshiftor
static inline uint32_t getshiftor(const uint32_t *v, int n, uint32_t mask, int shift)
{
    int inner = shift & 31, outer = shift >> 5;
    uint32_t r = outer < n ? v[outer] >> inner : 0;
    if (inner > 27 && outer + 1 < n) r |= v[outer + 1] << (32 - inner);
    return r & mask;
}
// BitVec.ishiftclear
static inline void ishiftclear(uint32_t *v, int n, uint32_t mask, int shift)
{
    int inner = shift & 31, outer = shift >> 5;
    if (outer < n) v[outer] &= ~(mask << inner);
    if (inner > 27 && outer + 1 < n) v[outer + 1] &= ~(mask >> (32 - inner));
}

// ---------------------------------------------------------------------------------------------
// arena for the session's precompiled rule data

typedef struct ArenaBlock { struct ArenaBlock *next; size_t used, cap; } ArenaBlock;
static void *arena_alloc(ArenaBlock **a, size_t n)
{
    n = (n + 7) & ~(size_t)7;
    if (!*a || (*a)->used + n > (*a)->cap) {
        size_t cap = n > 16384 ? n : 16384;
        ArenaBlock *b = ps_malloc_raw(sizeof(ArenaBlock) + cap);   // allocations are zeroed individually
        b->next = *a; b->used = 0; b->cap = cap;
        *a = b;
    }
    void *p = (char *)(*a + 1) + (*a)->used;
    (*a)->used += n;
    memset(p, 0, n);
    return p;
}
static void arena_free(ArenaBlock *a)
{
    while (a) { ArenaBlock *n = a->next; ps_free(a); a = n; }
}

// ---------------------------------------------------------------------------------------------
// precompiled rules: sparse word tests standing in for the JS generated match/replace code

typedef struct { uint32_t w, a, b; } WTest;

enum { REP_NONE, REP_SPARSE, REP_GENERAL };

typedef struct {
    bool isEllipsis;
    int nObj, nMov, nAny;
    WTest *obj, *mov;          // (word, present, missing)
    int *anyStart;             // nAny+1 offsets into anyW
    WTest *anyW;               // (word, mask, -)
    int repKind;
    const PSReplacement *rep;
    int nRO, nRM;
    WTest *ro, *rm;            // sparse: (word, clear, set); movement clear includes layer mask
    uint32_t *movClear;        // general: movementsClear | movementsLayerMask
    uint32_t *gen;             // general: objectsSet[so] objectsClear[so] movementsSet[sm] movClear[sm], together
    int nChoices; int *choices;   // general: bits of randomEntityMask
    int *choiceLayer;          // layer of each choice
    int nRandDir; int *randDirLayers;   // layers whose randomdir bit is set, ascending
    bool randDir;
} CPat;

// The rule data of a big game is far bigger than the device's 16KB data cache and every cache line
// a rule call touches costs ~1us there, so what the common (no match) path reads is kept small and
// inline: masks as sparse word lists stored in the struct, PSRule fields copied in.

// nonzero words of a "bits must be set" mask: (word, bits); object tests first, then movement tests
typedef struct { uint32_t w, m; } MW;
#define SMASK_INLINE 2
typedef struct { uint16_t nObj, nMov; const MW *t; MW inl[SMASK_INLINE]; } SMask;

typedef struct {
    SMask smask;               // cellRowMasks / cellRowMasks_Movements as sparse word lists
    uint32_t qw, qa, qb;       // quick reject on the first cell: one (word, present, missing) test
    int len, ellipsisCount, e1, e2;
    CPat *cells;
} CRow;

// Necessary condition for a rule to match, checked against mapCellContents(_Movements) before
// the rule's own data is touched: one word of the rule's object mask and one of the union of its
// rows' movement masks (the whole masks when the strides are 1).
typedef struct { uint32_t ow, om, mw, mm; } Gate;

typedef struct {
    Gate gate;
    uint8_t direction;
    bool hasReplacements, singleCell, isRandom, rigid;
    bool alwaysFail;           // a row the JS would turn into FALSE_FUNCTION
    int16_t nrows;
    int commandCount;
    SMask ruleMask;            // r->ruleMask, sparse
    CRow *rows;
    int rigidGroupIndex;       // groupNumber_to_RigidGroupIndex+1, when rigid
    const PSRule *r;           // cold data: commands, line number
} CRule;

typedef struct { int count; CRule *rules; Gate *gates; } CGroup;

// ---------------------------------------------------------------------------------------------
// backups (backupLevel / consolidateDiff)

typedef struct {
    bool diff;
    uint32_t *dat;
    int len;                   // words (full) or used diff entries
    int width, height;
    int flickLen, flick[4];
} PSBackup;

typedef struct { int *d; int len, cap; } IntVec;
typedef struct { int64_t *d; int len, cap; } I64Vec;

// The speculative turn run for "again" (processInput(-1, dontDoWin, dontModify)) computes exactly
// what the next real again turn's rule phase will: without rigid rules that phase only depends on
// the objects (movements are all zero), the RNG state and the message text. Its outcome is kept
// here and replayed by the real turn when all of those are unchanged (checked exactly).
#define AC_MSG 64
typedef struct {
    bool valid, armed;
    uint32_t *start; int startCap;   // objects the speculative turn started from
    int n, width, height;
    int *diffIdx; uint32_t *diffVal; int nDiff, capDiff;   // its post-rules objects, as changed words
    int cmdq[PS_CMD_COUNT]; int cmdqLen;
    char msgBefore[AC_MSG], msgAfter[AC_MSG];
    uint32_t *sfxC, *sfxD;
    I64Vec can, cant;
    unsigned rngDraws;
    const char *logs[8]; int nLogs;
} AgainCache;

// ---------------------------------------------------------------------------------------------
// session

typedef struct { uint32_t c[TL_MAX]; int n; } TLine;

struct PSSession {
    const PSGame *g;
    PSStorage storage; bool hasStorage;
    int so, sm, layerCount;
    bool rigidArrays;

    ArenaBlock *arena;
    CGroup *groups, *lateGroups;
    int maxRows;
    IntVec *rowMatches;        // per row match buffers (maxRows)
    int *tupleCounts;          // scratch for random rule groups

    PSLevel level;
    bool levelLoaded;
    int objCap;                // words allocated for level.objects
    bool movesZero;            // level.movements known to be all zero (see calculateRowColMasks)
    unsigned rngDraws;         // RNG draws (and reseeds) so far
    bool budgetHit;            // ps_over_budget_hook cut a rule loop short
    bool logCapture; const char *capLogs[8]; int nCapLogs; bool capOverflow;   // rule-phase log lines
    AgainCache ac;

    // engine.js / globalVariables.js globals
    bool unitTesting;
    int curlevel;
    PSBackup *curlevelTarget;
    bool hasUsedCheckpoint, levelEditorOpened, runrulesonlevelstart_phase, ignoreNotJustPressedAction;
    bool textMode, titleScreen, titleSelected;
    int titleMode, titleSelection;
    bool verbose_logging, throttle_movement, quittingTitleScreen, quittingMessageScreen;
    double deltatime, timer, repeatinterval, autotick, autotickinterval, againinterval;
    bool winning, againing, norepeat_action, restarting, messageselected;
    int flickLen, flick[4];    // oldflickscreendat
    int keybuffer[PS_KEY_COUNT]; int keybufferLen;
    double keyRepeatTimer; int keyRepeatIndex; double input_throttle_timer; int lastinput;

    char *messagetext;         // "" when none (never NULL)
    PSBackup **backups; int nbackups, capbackups; size_t backupBytes;
    PSBackup *restartTarget;
    PSRng rng;
    PSRng *rngp;               // the live RNG state: &rng, or a stack copy while a turn runs (see processInput)

    uint32_t *sfxCreateMask, *sfxDestroyMask;
    I64Vec canMove, cantMove;
    int64_t *soundHistory; int nSoundHistory, capSoundHistory;
    int64_t soundQueue[PS_SOUND_QUEUE_MAX]; int nSoundQueue;

    TLine ti[TI_ROWS]; int tiRows;
    char *tiUtf8[TI_ROWS]; const char *tiPtrs[TI_ROWS];
    int splitMessageLen;

    bool dirty;
    int view[4];

    // scratch
    uint32_t *o1, *o2, *o2_5, *o3, *o4, *o5, *o6, *o7, *o8, *o10, *o11;
    uint32_t *m1, *m2, *m3;
    int *playerPositions; int playerPosCap;
    // per-turn scratch, reused so a turn does not allocate. processInput nests (again turns,
    // run_rules_on_level_start in DoRestart), so the pre-turn backup is kept per nesting depth.
#define TURN_DEPTH_MAX 4
    PSBackup turnBak[TURN_DEPTH_MAX]; int turnBakCap[TURN_DEPTH_MAX]; int inputDepth;
    bool *bannedGroup;
    const char *warnings[8], *warningKeys[8]; int nWarnings;   // run-time logWarning (string literals)
    uint32_t *startObjects, *startMovements; size_t startCap;
};

static inline int rng_index(PSSession *s, uint32_t n);
static inline double rng_uniform(PSSession *s);
static bool processInput(PSSession *s, int dir, bool dontDoWin, bool dontModify);
static bool processInput_rng(PSSession *s, int dir, bool dontDoWin, bool dontModify);
static bool processInput_(PSSession *s, PSLevel *l, const PSGame *g, PSBackup *bak, int dir, bool dontDoWin, bool dontModify);
static void goToTitleScreen(PSSession *s);
static void nextLevel(PSSession *s);
static void DoUndo(PSSession *s, bool force, bool ignoreDuplicates);
static void DoRestart(PSSession *s, bool force);
static void checkWin(PSSession *s, bool dontDoWin);
static void generateTitleScreen(PSSession *s);
static void drawMessageScreen(PSSession *s);
static void redraw(PSSession *s);

static void set_messagetext(PSSession *s, const char *t)
{
    if (!t) t = "";
    if (s->messagetext && strcmp(s->messagetext, t) == 0) return;
    char *n = ps_strdup(t);
    ps_free(s->messagetext);
    s->messagetext = n;
}

// ---------------------------------------------------------------------------------------------
// sounds (sfxr.js playSound + debug.js pushSoundToHistory)

static void playSound(PSSession *s, int64_t seed, bool ignore)
{
    if (!ignore && s->nSoundHistory < PS_SOUND_HISTORY_MAX)
        VEC_PUSH(s->soundHistory, s->nSoundHistory, s->capSoundHistory, seed);
    if (s->unitTesting) return;
    if (s->nSoundQueue < PS_SOUND_QUEUE_MAX) s->soundQueue[s->nSoundQueue++] = seed;
}

// tryPlaySimpleSound
static void tryPlaySimpleSound(PSSession *s, int evt)
{
    int64_t seed = s->g->sfx_Events[evt];
    if (seed != -1) playSound(s, seed, true);
}

// clearInputHistory (debug.js)
static void clearInputHistory(PSSession *s) { s->nSoundHistory = 0; }

// ---------------------------------------------------------------------------------------------
// storage (storagewrapper.js), keys are document.URL and document.URL+'_checkpoint'

#define KEY_LEVEL ""
#define KEY_CHECKPOINT "_checkpoint"

static char *storage_get(PSSession *s, const char *k) { return s->hasStorage && s->storage.get ? s->storage.get(s->storage.ctx, k) : NULL; }
static void storage_set(PSSession *s, const char *k, const char *v) { if (s->hasStorage && s->storage.set) s->storage.set(s->storage.ctx, k, v); }
static void storage_remove(PSSession *s, const char *k) { if (s->hasStorage && s->storage.remove) s->storage.remove(s->storage.ctx, k); }

static int fmt_int(char *buf, long v)
{
    char tmp[24]; int n = 0, o = 0;
    unsigned long u = v < 0 ? (unsigned long)(-v) : (unsigned long)v;
    do { tmp[n++] = (char)('0' + u % 10); u /= 10; } while (u);
    if (v < 0) buf[o++] = '-';
    while (n) buf[o++] = tmp[--n];
    buf[o] = 0;
    return o;
}
// parseInt-ish: returns false for NaN
static bool parse_int(const char **pp, long *out)
{
    const char *p = *pp;
    while (*p == ' ' || *p == '\t' || *p == '\n' || *p == '\r') p++;
    bool neg = false;
    if (*p == '-' || *p == '+') { neg = *p == '-'; p++; }
    if (*p < '0' || *p > '9') return false;
    long v = 0;
    while (*p >= '0' && *p <= '9') v = v * 10 + (*p++ - '0');
    *out = neg ? -v : v;
    *pp = p;
    return true;
}

static void storage_set_int(PSSession *s, const char *k, long v) { char b[24]; fmt_int(b, v); storage_set(s, k, b); }

// JSON.stringify(level4Serialization())
static char *serialize_backup(const PSBackup *b)
{
    size_t cap = 96 + (size_t)b->len * 12;
    char *out = xalloc(cap), *p = out;
    memcpy(p, "{\"dat\":[", 8); p += 8;
    for (int i = 0; i < b->len; i++) {
        if (i) *p++ = ',';
        p += fmt_int(p, (int32_t)b->dat[i]);
    }
    memcpy(p, "],\"width\":", 10); p += 10; p += fmt_int(p, b->width);
    memcpy(p, ",\"height\":", 10); p += 10; p += fmt_int(p, b->height);
    memcpy(p, ",\"oldflickscreendat\":[", 22); p += 22;
    for (int i = 0; i < b->flickLen; i++) { if (i) *p++ = ','; p += fmt_int(p, b->flick[i]); }
    *p++ = ']'; *p++ = '}'; *p = 0;
    return out;
}

static const char *json_find(const char *s, const char *key)
{
    size_t kl = strlen(key);
    for (const char *p = s; (p = strchr(p, '"')); p++) {
        if (strncmp(p + 1, key, kl) == 0 && p[1 + kl] == '"') {
            p += kl + 2;
            while (*p == ' ' || *p == ':') p++;
            return p;
        }
    }
    return NULL;
}
static PSBackup *parse_backup(const char *str)
{
    const char *p = json_find(str, "dat");
    if (!p || *p != '[') return NULL;
    PSBackup *b = xalloc(sizeof *b);
    int cap = 0;
    p++;
    for (;;) {
        while (*p == ' ' || *p == ',') p++;
        long v;
        if (!parse_int(&p, &v)) break;
        if (b->len >= cap) { cap = cap ? cap * 2 : 256; b->dat = xrealloc(b->dat, sizeof(uint32_t) * (size_t)cap); }
        b->dat[b->len++] = (uint32_t)(int32_t)v;
    }
    long v;
    const char *q = json_find(str, "width");
    if (q && parse_int(&q, &v)) b->width = (int)v;
    q = json_find(str, "height");
    if (q && parse_int(&q, &v)) b->height = (int)v;
    q = json_find(str, "oldflickscreendat");
    if (q && *q == '[') {
        q++;
        while (b->flickLen < 4) {
            while (*q == ' ' || *q == ',') q++;
            if (!parse_int(&q, &v)) break;
            b->flick[b->flickLen++] = (int)v;
        }
        if (b->flickLen != 4) b->flickLen = 0;
    }
    if (b->width <= 0 || b->height <= 0 || !b->dat) { ps_free(b->dat); ps_free(b); return NULL; }
    return b;
}

// ---------------------------------------------------------------------------------------------
// backups

static void backup_free(PSBackup *b) { if (b) { ps_free(b->dat); ps_free(b); } }
static size_t backup_bytes(const PSBackup *b) { return sizeof(PSBackup) + (size_t)b->len * 4; }
static PSBackup *backup_clone(const PSBackup *b)
{
    PSBackup *c = ps_malloc_raw(sizeof *c);
    *c = *b;
    c->dat = ps_malloc_raw((size_t)(b->len ? b->len : 1) * 4);
    memcpy(c->dat, b->dat, (size_t)b->len * 4);
    return c;
}

// backupLevel into a reusable buffer (cap is the buffer's capacity in words)
static void backupLevelInto(PSSession *s, PSBackup *b, int *cap)
{
    int n = s->level.n_tiles * s->so;
    if (n > *cap || !b->dat) {
        ps_free(b->dat);
        *cap = n > 0 ? n : 1;
        b->dat = ps_malloc_raw((size_t)*cap * 4);
        memcpy(b->dat, s->level.objects, (size_t)n * 4);
    } else if (n <= 2048) {
        put_words(b->dat, s->level.objects, n);   // usually mostly unchanged since the last turn
    } else {
        memcpy(b->dat, s->level.objects, (size_t)n * 4);   // two arrays this big don't fit the device cache
    }
    b->diff = false;
    b->len = n;
    b->width = s->level.width; b->height = s->level.height;
    b->flickLen = s->flickLen;
    memcpy(b->flick, s->flick, sizeof b->flick);
}

// backupLevel / level4Serialization
static PSBackup *backupLevel(PSSession *s)
{
    PSBackup *b = xalloc(sizeof *b);
    int n = s->level.n_tiles * s->so;
    b->dat = ps_malloc_raw((size_t)(n ? n : 1) * 4);
    if (n) memcpy(b->dat, s->level.objects, (size_t)n * 4);
    b->len = n;
    b->width = s->level.width; b->height = s->level.height;
    b->flickLen = s->flickLen;
    memcpy(b->flick, s->flick, sizeof b->flick);
    return b;
}

// applyDiff
static void applyDiff(const PSBackup *diff, uint32_t *objs)
{
    int index = 0;
    while (index < diff->len) {
        int start = (int)diff->dat[index];
        int copy = (int)diff->dat[index + 1];
        if (copy == 0) break;
        put_words(objs + start, diff->dat + index + 2, copy);
        index += 2 + copy;
    }
}

// unconsolidateDiff (takes ownership of before; returns a full backup)
static PSBackup *unconsolidateDiff(PSBackup *before, const PSBackup *after)
{
    if (!before->diff) return before;
    PSBackup *r = xalloc(sizeof *r);
    r->dat = ps_malloc_raw((size_t)(after->len ? after->len : 1) * 4);
    memcpy(r->dat, after->dat, (size_t)after->len * 4);
    r->len = after->len;
    applyDiff(before, r->dat);
    r->width = before->width; r->height = before->height;
    r->flickLen = before->flickLen; memcpy(r->flick, before->flick, sizeof r->flick);
    backup_free(before);
    return r;
}

// consolidateDiff (takes ownership of before)
static PSBackup *consolidateDiff(PSBackup *before, const PSBackup *after)
{
    if (before->width != after->width || before->height != after->height || before->len != after->len) return before;
    if (before->diff || after->diff) return before;
    if (before->len < 1024) return before;
    int cap = 128, pos = 0, chainStart = -1;
    bool chain = false;
    uint32_t *res = ps_malloc_raw((size_t)cap * 4);
    const uint32_t *bd = before->dat, *ad = after->dat;
    for (int i = 0; i < before->len; i++) {
        if (!chain) {
            if (bd[i] != ad[i]) {
                chain = true;
                chainStart = pos;
                if (cap < pos + 4) { cap *= 2; res = xrealloc(res, (size_t)cap * 4); }
                res[pos] = (uint32_t)i; res[pos + 1] = 1; res[pos + 2] = bd[i];
                pos += 3;
            }
        } else {
            if (bd[i] != ad[i]) {
                if (cap < pos + 4) { cap *= 2; res = xrealloc(res, (size_t)cap * 4); }
                res[chainStart + 1]++;
                res[pos++] = bd[i];
            } else {
                chain = false;
            }
        }
    }
    PSBackup *r = xalloc(sizeof *r);
    r->diff = true;
    r->dat = xrealloc(res, (size_t)(pos ? pos : 1) * 4);
    r->len = pos;
    r->width = before->width; r->height = before->height;
    r->flickLen = before->flickLen; memcpy(r->flick, before->flick, sizeof r->flick);
    backup_free(before);
    return r;
}

static void backups_pop(PSSession *s)
{
    if (s->nbackups == 0) return;
    PSBackup *b = s->backups[--s->nbackups];
    s->backupBytes -= backup_bytes(b);
    backup_free(b);
}
static void backups_clear(PSSession *s)
{
    while (s->nbackups) backups_pop(s);
}

static void undo_after_push(PSSession *s, int n, bool newIsFull, size_t virtualBytes);
static void addUndoStateOwned(PSSession *s, PSBackup *b);
static void restoreLevel(PSSession *s, const PSBackup *lev);

// addUndoState (copies st)
static void addUndoState(PSSession *s, const PSBackup *st)
{
    addUndoStateOwned(s, backup_clone(st));
}
// addUndoState for a backup nobody else holds: pushed as is, no copy
static void addUndoStateOwned(PSSession *s, PSBackup *b)
{
    VEC_PUSH(s->backups, s->nbackups, s->capbackups, b);
    s->backupBytes += backup_bytes(b);
    undo_after_push(s, s->nbackups, !b->diff, 0);
}

// What addUndoState does once the new entry is on the stack: consolidate the third-newest entry
// and apply the memory cap. n counts the new entry; when it is not really pushed (virtualBytes is
// its size) the stack holds n-1 entries.
static void undo_after_push(PSSession *s, int n, bool newIsFull, size_t virtualBytes)
{
    if (n > 2 && newIsFull) {
        PSBackup *old = s->backups[n - 3];
        size_t ob = backup_bytes(old);
        PSBackup *c = consolidateDiff(old, s->backups[n - 2]);
        s->backupBytes -= ob;
        s->backups[n - 3] = c;
        s->backupBytes += backup_bytes(c);
    }
    // device memory cap: drop the oldest entries (nothing depends on them)
    if ((size_t)PS_UNDO_MAX_BYTES != 0) {
        int drop = 0;
        size_t bytes = s->backupBytes + virtualBytes;
        while (bytes > PS_UNDO_MAX_BYTES && n - drop > 2) bytes -= backup_bytes(s->backups[drop++]);
        if (drop) {
            for (int i = 0; i < drop; i++) backup_free(s->backups[i]);
            memmove(s->backups, s->backups + drop, sizeof(PSBackup *) * (size_t)(s->nbackups - drop));
            s->nbackups -= drop;
            s->backupBytes = bytes - virtualBytes;
        }
    }
}

// addUndoState(s, bak); DoUndo(s, true, false) -- push the pre-turn state and restore it right
// away -- without copying bak onto the undo stack only to pop it again. The push's side effects
// (consolidating the entry below, the memory cap) still happen exactly as they would.
static void undo_push_restore(PSSession *s, const PSBackup *bak)
{
    if (!s->levelLoaded) { addUndoState(s, bak); return; }   // DoUndo would return early
    undo_after_push(s, s->nbackups + 1, !bak->diff, backup_bytes(bak));
    restoreLevel(s, bak);
}

// ---------------------------------------------------------------------------------------------
// level arrays

static void level_free_arrays(PSLevel *l)
{
    ps_free(l->movements); ps_free(l->rigidGroupIndexMask); ps_free(l->rigidMovementAppliedMask);
    ps_free(l->rowCellContents); ps_free(l->colCellContents); ps_free(l->mapCellContents);
    ps_free(l->rowCellContents_Movements); ps_free(l->colCellContents_Movements); ps_free(l->mapCellContents_Movements);
    l->movements = l->rigidGroupIndexMask = l->rigidMovementAppliedMask = NULL;
    l->rowCellContents = l->colCellContents = l->mapCellContents = NULL;
    l->rowCellContents_Movements = l->colCellContents_Movements = l->mapCellContents_Movements = NULL;
}

// RebuildLevelArrays
static void RebuildLevelArrays(PSSession *s)
{
    PSLevel *l = &s->level;
    int so = s->so, sm = s->sm;
    level_free_arrays(l);
    size_t nt = (size_t)(l->n_tiles ? l->n_tiles : 1);
    l->movements = xalloc(nt * (size_t)sm * 4);
    s->movesZero = true;
    l->mapCellContents = xalloc((size_t)so * 4);
    l->mapCellContents_Movements = xalloc((size_t)sm * 4);
    l->rowCellContents = xalloc((size_t)(l->height ? l->height : 1) * (size_t)so * 4);
    l->colCellContents = xalloc((size_t)(l->width ? l->width : 1) * (size_t)so * 4);
    l->rowCellContents_Movements = xalloc((size_t)(l->height ? l->height : 1) * (size_t)sm * 4);
    l->colCellContents_Movements = xalloc((size_t)(l->width ? l->width : 1) * (size_t)sm * 4);
    if (s->rigidArrays) {
        l->rigidMovementAppliedMask = xalloc(nt * (size_t)sm * 4);
        l->rigidGroupIndexMask = xalloc(nt * (size_t)sm * 4);
    }
    if (l->n_tiles > s->playerPosCap) {
        s->playerPosCap = l->n_tiles;
        ps_free(s->playerPositions);
        s->playerPositions = xalloc(sizeof(int) * (size_t)s->playerPosCap);
    }
}

// restoreLevel
static void restoreLevel(PSSession *s, const PSBackup *lev)
{
    PSLevel *l = &s->level;
    s->flickLen = lev->flickLen;
    memcpy(s->flick, lev->flick, sizeof s->flick);
    if (lev->diff) {
        applyDiff(lev, l->objects);
    } else if (l->objects && lev->len == l->n_tiles * s->so && lev->len <= s->objCap) {
        put_words(l->objects, lev->dat, lev->len);   // in place: only changed words are stored
    } else {
        ps_free(l->objects);
        s->objCap = lev->len ? lev->len : 1;
        l->objects = ps_malloc_raw((size_t)s->objCap * 4);
        memcpy(l->objects, lev->dat, (size_t)lev->len * 4);
    }
    if (l->width != lev->width || l->height != lev->height) {
        l->width = lev->width;
        l->height = lev->height;
        l->n_tiles = lev->width * lev->height;
        RebuildLevelArrays(s);
    } else {
        // (JS only zeroes n_tiles words here; movements are always zero between turns anyway)
        zero_words(l->movements, l->n_tiles * s->sm);
        s->movesZero = true;
        if (s->rigidArrays) {
            zero_words(l->rigidMovementAppliedMask, l->n_tiles * s->sm);
            zero_words(l->rigidGroupIndexMask, l->n_tiles * s->sm);
        }
        zero_words(l->rowCellContents, l->height * s->so);
        zero_words(l->colCellContents, l->width * s->so);
    }
    s->againing = false;
    l->commandQueueLen = 0;
}

// One mask kind (objects or movements) of generate_calculateRowColMasks: row/col/map ORs of every
// cell. Accumulated in stack memory (fast TCM on the device) and stored once.
#define RC_ROWBUF 512   // words
#define RC_MAXSTRIDE 64
static void rowcol_pass(const uint32_t *cells, int stride, int w, int h, uint32_t *rowOut, uint32_t *colOut, uint32_t *mapOut)
{
    if (stride > RC_MAXSTRIDE) {
        memset(rowOut, 0, (size_t)h * (size_t)stride * 4);
        memset(colOut, 0, (size_t)w * (size_t)stride * 4);
        memset(mapOut, 0, (size_t)stride * 4);
        for (int i = 0; i < w; i++)
            for (int j = 0; j < h; j++) {
                const uint32_t *c = cells + ((size_t)i * h + j) * stride;
                for (int k = 0; k < stride; k++) {
                    or_word(&rowOut[j * stride + k], c[k]); or_word(&colOut[i * stride + k], c[k]); or_word(&mapOut[k], c[k]);
                }
            }
        return;
    }
    uint32_t rowStack[RC_ROWBUF], colAcc[RC_MAXSTRIDE], mapAcc[RC_MAXSTRIDE];
    bool stackRows = h * stride <= RC_ROWBUF;
    uint32_t *rowAcc = stackRows ? rowStack : rowOut;
    memset(rowAcc, 0, (size_t)h * (size_t)stride * 4);
    memset(mapAcc, 0, (size_t)stride * 4);
    if (stride == 1) {
        uint32_t map = 0;
        for (int i = 0; i < w; i++) {
            const uint32_t *c = cells + (size_t)i * h;
            uint32_t col = 0;
            if (stackRows) for (int j = 0; j < h; j++) { uint32_t v = c[j]; col |= v; rowAcc[j] |= v; }
            else for (int j = 0; j < h; j++) { uint32_t v = c[j]; col |= v; or_word(&rowAcc[j], v); }
            if (colOut[i] != col) colOut[i] = col;
            map |= col;
        }
        mapAcc[0] = map;
    } else {
        for (int i = 0; i < w; i++) {
            memset(colAcc, 0, (size_t)stride * 4);
            const uint32_t *c = cells + (size_t)i * h * stride;
            for (int j = 0; j < h; j++, c += stride) {
                uint32_t *r = rowAcc + (size_t)j * stride;
                for (int k = 0; k < stride; k++) {
                    uint32_t v = c[k];
                    colAcc[k] |= v;
                    if (stackRows) r[k] |= v; else or_word(&r[k], v);
                }
            }
            put_words(colOut + (size_t)i * stride, colAcc, stride);
            for (int k = 0; k < stride; k++) mapAcc[k] |= colAcc[k];
        }
    }
    if (stackRows) put_words(rowOut, rowAcc, h * stride);
    put_words(mapOut, mapAcc, stride);
}

// generate_calculateRowColMasks. When the movements were all zero before startMovement (the
// usual case: resolveMovements clears them every turn), only the nMoved cells startMovement wrote
// can have movement, so the movement masks come from those cells alone.
static void calculateRowColMasks(PSSession *s, const int *moved, int nMoved)
{
    PSLevel *l = &s->level;
    int sm = s->sm, h = l->height;
    rowcol_pass(l->objects, s->so, l->width, h, l->rowCellContents, l->colCellContents, l->mapCellContents);
    if (!moved) {
        rowcol_pass(l->movements, sm, l->width, h, l->rowCellContents_Movements, l->colCellContents_Movements,
                    l->mapCellContents_Movements);
        return;
    }
    zero_words(l->rowCellContents_Movements, h * sm);
    zero_words(l->colCellContents_Movements, l->width * sm);
    zero_words(l->mapCellContents_Movements, sm);
    for (int k = 0; k < nMoved; k++) {
        int i = moved[k];
        const uint32_t *m = l->movements + (size_t)i * sm;
        for (int w = 0; w < sm; w++) {
            or_word(&l->rowCellContents_Movements[(i % h) * sm + w], m[w]);
            or_word(&l->colCellContents_Movements[(i / h) * sm + w], m[w]);
            or_word(&l->mapCellContents_Movements[w], m[w]);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// rule precompilation (Rule / CellPattern constructors + generate* functions)

static void cpat_build(PSSession *s, CPat *p, const PSCellPattern *src, const PSRule *rule)
{
    int so = s->so, sm = s->sm;
    p->isEllipsis = src->isEllipsis;
    if (p->isEllipsis) return;
    // (arrays are sized exactly: big games have thousands of cells, and so/sm can be 5+ words)
    int n = 0;
    for (int i = 0; i < so; i++)
        if ((src->objectsPresent && src->objectsPresent[i]) || (src->objectsMissing && src->objectsMissing[i])) n++;
    p->obj = n ? arena_alloc(&s->arena, sizeof(WTest) * (size_t)n) : NULL;
    for (int i = 0; i < so; i++) {
        uint32_t a = src->objectsPresent ? src->objectsPresent[i] : 0, b = src->objectsMissing ? src->objectsMissing[i] : 0;
        if (a || b) { p->obj[p->nObj].w = (uint32_t)i; p->obj[p->nObj].a = a; p->obj[p->nObj].b = b; p->nObj++; }
    }
    n = 0;
    for (int i = 0; i < sm; i++)
        if ((src->movementsPresent && src->movementsPresent[i]) || (src->movementsMissing && src->movementsMissing[i])) n++;
    p->mov = n ? arena_alloc(&s->arena, sizeof(WTest) * (size_t)n) : NULL;
    for (int i = 0; i < sm; i++) {
        uint32_t a = src->movementsPresent ? src->movementsPresent[i] : 0, b = src->movementsMissing ? src->movementsMissing[i] : 0;
        if (a || b) { p->mov[p->nMov].w = (uint32_t)i; p->mov[p->nMov].a = a; p->mov[p->nMov].b = b; p->nMov++; }
    }
    p->nAny = src->anyCount;
    if (p->nAny) {
        n = 0;
        for (int j = 0; j < p->nAny; j++) for (int i = 0; i < so; i++) if (src->anyObjectsPresent[j][i]) n++;
        p->anyStart = arena_alloc(&s->arena, sizeof(int) * (size_t)(p->nAny + 1));
        p->anyW = arena_alloc(&s->arena, sizeof(WTest) * (size_t)(n ? n : 1));
        int k = 0;
        for (int j = 0; j < p->nAny; j++) {
            p->anyStart[j] = k;
            for (int i = 0; i < so; i++) {
                uint32_t m = src->anyObjectsPresent[j][i];
                if (m) { p->anyW[k].w = (uint32_t)i; p->anyW[k].a = m; k++; }
            }
        }
        p->anyStart[p->nAny] = k;
    }

    const PSReplacement *r = src->replacement;
    p->rep = r;
    if (!r) { p->repKind = REP_NONE; return; }
    bool det = is_zero(r->randomEntityMask, so) && is_zero(r->randomDirMask, sm);
    if (det && !rule->rigid) {
        // generateSparseReplaceFunction
        int nro = 0, nrm = 0;
        for (int i = 0; i < so; i++) if (r->objectsClear[i] || r->objectsSet[i]) nro++;
        for (int i = 0; i < sm; i++) if (r->movementsClear[i] || r->movementsLayerMask[i] || r->movementsSet[i]) nrm++;
        p->ro = nro ? arena_alloc(&s->arena, sizeof(WTest) * (size_t)nro) : NULL;
        p->rm = nrm ? arena_alloc(&s->arena, sizeof(WTest) * (size_t)nrm) : NULL;
        for (int i = 0; i < so; i++)
            if (r->objectsClear[i] || r->objectsSet[i]) {
                p->ro[p->nRO].w = (uint32_t)i; p->ro[p->nRO].a = r->objectsClear[i]; p->ro[p->nRO].b = r->objectsSet[i]; p->nRO++;
            }
        for (int i = 0; i < sm; i++)
            if (r->movementsClear[i] || r->movementsLayerMask[i] || r->movementsSet[i]) {
                p->rm[p->nRM].w = (uint32_t)i; p->rm[p->nRM].a = r->movementsClear[i] | r->movementsLayerMask[i];
                p->rm[p->nRM].b = r->movementsSet[i]; p->nRM++;
            }
        p->repKind = (p->nRO || p->nRM) ? REP_SPARSE : REP_NONE;
        return;
    }
    p->repKind = REP_GENERAL;
    p->movClear = arena_alloc(&s->arena, sizeof(uint32_t) * (size_t)sm);
    for (int i = 0; i < sm; i++) p->movClear[i] = r->movementsClear[i] | r->movementsLayerMask[i];
    n = 0;
    for (int i = 0; i < 32 * so; i++) if (get_bit(r->randomEntityMask, i)) n++;
    p->choices = n ? arena_alloc(&s->arena, sizeof(int) * (size_t)n) : NULL;
    for (int i = 0; i < 32 * so; i++) if (get_bit(r->randomEntityMask, i)) p->choices[p->nChoices++] = i;
    p->randDir = !is_zero(r->randomDirMask, sm);
    p->gen = arena_alloc(&s->arena, sizeof(uint32_t) * (size_t)(2 * so + 2 * sm));
    for (int i = 0; i < so; i++) { p->gen[i] = r->objectsSet[i]; p->gen[so + i] = r->objectsClear[i]; }
    for (int i = 0; i < sm; i++) { p->gen[2 * so + i] = r->movementsSet[i]; p->gen[2 * so + sm + i] = p->movClear[i]; }
    if (p->nChoices) {
        p->choiceLayer = arena_alloc(&s->arena, sizeof(int) * (size_t)p->nChoices);
        for (int k = 0; k < p->nChoices; k++) p->choiceLayer[k] = s->g->objects[p->choices[k]].layer;
    }
    for (int li = 0; li < s->layerCount; li++) if (get_bit(r->randomDirMask, 5 * li)) p->nRandDir++;
    if (p->nRandDir) {
        p->randDirLayers = arena_alloc(&s->arena, sizeof(int) * (size_t)p->nRandDir);
        int k = 0;
        for (int li = 0; li < s->layerCount; li++) if (get_bit(r->randomDirMask, 5 * li)) p->randDirLayers[k++] = li;
    }
}

static void smask_build(PSSession *s, SMask *m, const uint32_t *objMask, const uint32_t *movMask)
{
    int no = 0, nm = 0;
    for (int i = 0; i < s->so; i++) if (objMask && objMask[i]) no++;
    for (int i = 0; i < s->sm; i++) if (movMask && movMask[i]) nm++;
    m->nObj = (uint16_t)no; m->nMov = (uint16_t)nm;
    MW *t = no + nm <= SMASK_INLINE ? m->inl : arena_alloc(&s->arena, sizeof(MW) * (size_t)(no + nm));
    int k = 0;
    for (int i = 0; i < s->so; i++) if (objMask && objMask[i]) { t[k].w = (uint32_t)i; t[k].m = objMask[i]; k++; }
    for (int i = 0; i < s->sm; i++) if (movMask && movMask[i]) { t[k].w = (uint32_t)i; t[k].m = movMask[i]; k++; }
    m->t = t;
}

static void crule_build(PSSession *s, CRule *cr, const PSRule *r)
{
    cr->r = r;
    cr->nrows = r->rowCount;
    cr->rows = arena_alloc(&s->arena, sizeof(CRow) * (size_t)(r->rowCount ? r->rowCount : 1));
    for (int i = 0; i < r->rowCount; i++) {
        const PSCellRow *row = &r->rows[i];
        CRow *c = &cr->rows[i];
        c->len = row->len;
        c->ellipsisCount = row->ellipsisCount;
        smask_build(s, &c->smask, r->cellRowMasks[i], r->cellRowMasks_Movements[i]);
        c->cells = arena_alloc(&s->arena, sizeof(CPat) * (size_t)(row->len ? row->len : 1));
        c->e1 = c->e2 = -1;
        for (int j = 0; j < row->len; j++) {
            cpat_build(s, &c->cells[j], &row->cells[j], r);
            if (row->cells[j].isEllipsis) { if (c->e1 < 0) c->e1 = j; else if (c->e2 < 0) c->e2 = j; }
        }
        if (row->len == 0 || row->cells[0].isEllipsis) cr->alwaysFail = true;
        else {
            const CPat *p0 = &c->cells[0];
            int best = -1;
            for (int k = 0; k < p0->nObj; k++) if (best < 0 || (p0->obj[k].a && !p0->obj[best].a)) best = k;
            if (best >= 0) { c->qw = p0->obj[best].w; c->qa = p0->obj[best].a; c->qb = p0->obj[best].b; }
        }
    }
    smask_build(s, &cr->ruleMask, r->ruleMask, NULL);
    cr->direction = (uint8_t)r->direction;
    cr->hasReplacements = r->hasReplacements;
    cr->isRandom = r->isRandom;
    cr->rigid = r->rigid;
    cr->commandCount = r->commandCount;
    cr->nrows = (int16_t)r->rowCount;
    // gate: a nonzero word of the object mask and of the union of the rows' movement masks
    cr->gate.ow = cr->gate.om = cr->gate.mw = cr->gate.mm = 0;
    for (int i = 0; i < s->so; i++) if (r->ruleMask && r->ruleMask[i]) { cr->gate.ow = (uint32_t)i; cr->gate.om = r->ruleMask[i]; break; }
    for (int i = 0; i < s->sm && !cr->gate.mm; i++) {
        uint32_t u = 0;
        for (int j = 0; j < r->rowCount; j++) if (r->cellRowMasks_Movements[j]) u |= r->cellRowMasks_Movements[j][i];
        if (u) { cr->gate.mw = (uint32_t)i; cr->gate.mm = u; }
    }
    if (r->rowCount > s->maxRows) s->maxRows = r->rowCount;
    cr->singleCell = r->rowCount == 1 && r->rows[0].len == 1 && r->rows[0].ellipsisCount == 0;
    cr->rigidGroupIndex = 0;
    if (r->rigid && s->g->groupNumber_to_RigidGroupIndex && r->groupNumber >= 0 && r->groupNumber < s->g->groupNumberCount)
        cr->rigidGroupIndex = s->g->groupNumber_to_RigidGroupIndex[r->groupNumber] + 1;
}

static CGroup *build_groups(PSSession *s, const PSRuleGroup *src, int n)
{
    CGroup *gs = arena_alloc(&s->arena, sizeof(CGroup) * (size_t)(n ? n : 1));
    for (int i = 0; i < n; i++) {
        gs[i].count = src[i].count;
        gs[i].rules = arena_alloc(&s->arena, sizeof(CRule) * (size_t)(src[i].count ? src[i].count : 1));
        for (int j = 0; j < src[i].count; j++) {
            crule_build(s, &gs[i].rules[j], &src[i].rules[j]);
            if (src[i].rules[j].rigid) s->rigidArrays = true;
        }
        gs[i].gates = arena_alloc(&s->arena, sizeof(Gate) * (size_t)(src[i].count ? src[i].count : 1));
        for (int j = 0; j < src[i].count; j++) gs[i].gates[j] = gs[i].rules[j].gate;
    }
    return gs;
}

// ---------------------------------------------------------------------------------------------
// matching

// CellPattern.prototype.generateMatchFunction
static ALWAYS_INLINE bool cpat_match(const CPat *p, const uint32_t *objs, const uint32_t *movs, int i, int so, int sm)
{
    const uint32_t *o = objs + (size_t)i * (size_t)so;
    for (int k = 0; k < p->nObj; k++) {
        uint32_t v = o[p->obj[k].w];
        if ((v & p->obj[k].a) != p->obj[k].a || (v & p->obj[k].b)) return false;
    }
    if (p->nMov) {
        const uint32_t *m = movs + (size_t)i * (size_t)sm;
        for (int k = 0; k < p->nMov; k++) {
            uint32_t v = m[p->mov[k].w];
            if ((v & p->mov[k].a) != p->mov[k].a || (v & p->mov[k].b)) return false;
        }
    }
    for (int j = 0; j < p->nAny; j++) {
        uint32_t acc = 0;
        for (int k = p->anyStart[j]; k < p->anyStart[j + 1]; k++) acc |= o[p->anyW[k].w] & p->anyW[k].a;
        if (!acc) return false;
    }
    return true;
}

#define CMATCH(cell, idx) cpat_match((cell), objs, movs, (idx), so, sm)

// bitsSetInArray(mask, contents) for both halves of a sparse mask
static ALWAYS_INLINE bool smask_in(const SMask *m, const uint32_t *objC, const uint32_t *movC)
{
    const MW *t = m->t;
    int no = m->nObj, n = no + m->nMov;
    for (int k = 0; k < no; k++) if ((objC[t[k].w] & t[k].m) != t[k].m) return false;
    for (int k = no; k < n; k++) if ((movC[t[k].w] & t[k].m) != t[k].m) return false;
    return true;
}
static ALWAYS_INLINE bool gate_open(const Gate *g, const PSLevel *l)
{
    return (l->mapCellContents[g->ow] & g->om) == g->om && (l->mapCellContents_Movements[g->mw] & g->mm) == g->mm;
}

// generateCellRowMatchesFunction, ellipsisCount 0
static ALWAYS_INLINE bool row_match0(const CRow *row, int i, int d, const uint32_t *objs, const uint32_t *movs, int so, int sm)
{
    for (int c = 0; c < row->len; c++) if (!CMATCH(&row->cells[c], i + c * d)) return false;
    return true;
}

static inline void ivec_push(IntVec *v, int x) { VEC_PUSH(v->d, v->len, v->cap, x); }

// cells after the first ellipsis, for gap k (ellipsisCount 1, or the middle part for 2)
static ALWAYS_INLINE bool row_tail1(const CRow *row, int i, int k, int d, const uint32_t *objs, const uint32_t *movs, int so, int sm)
{
    int end = row->ellipsisCount == 2 ? row->e2 : row->len;
    for (int c = row->e1 + 1; c < end; c++) if (!CMATCH(&row->cells[c], i + d * (k + c - 1))) return false;
    return true;
}
static ALWAYS_INLINE bool row_tail2(const CRow *row, int i, int k1, int k2, int d, const uint32_t *objs, const uint32_t *movs, int so, int sm)
{
    for (int c = row->e2 + 1; c < row->len; c++) if (!CMATCH(&row->cells[c], i + d * (k1 + k2 + c - 2))) return false;
    return true;
}
static ALWAYS_INLINE bool row_head(const CRow *row, int i, int d, const uint32_t *objs, const uint32_t *movs, int so, int sm)
{
    for (int c = 0; c < row->e1; c++) if (!CMATCH(&row->cells[c], i + c * d)) return false;
    return true;
}

// generateCellRowMatchesFunction, ellipsisCount 1: pushes [i,k]
static void row_match1(const CRow *row, int i, int kmax, int kmin, int d, const uint32_t *objs, const uint32_t *movs,
                       int so, int sm, IntVec *out)
{
    if (!row_head(row, i, d, objs, movs, so, sm)) return;
    for (int k = kmin; k < kmax; k++)
        if (row_tail1(row, i, k, d, objs, movs, so, sm)) { ivec_push(out, i); ivec_push(out, k); }
}

// generateCellRowMatchesFunction, ellipsisCount 2: pushes [i,k1,k2]
static void row_match2(const CRow *row, int i, int kmax, int kmin, int k1max, int k1min, int k2max, int k2min, int d,
                       const uint32_t *objs, const uint32_t *movs, int so, int sm, IntVec *out)
{
    if (!row_head(row, i, d, objs, movs, so, sm)) return;
    (void)kmin;
    for (int k1 = k1min; k1 < k1max; k1++) {
        if (!row_tail1(row, i, k1, d, objs, movs, so, sm)) continue;
        for (int k2 = k2min; k1 + k2 < kmax && k2 < k2max; k2++)
            if (row_tail2(row, i, k1, k2, d, objs, movs, so, sm)) { ivec_push(out, i); ivec_push(out, k1); ivec_push(out, k2); }
    }
}

static inline int delta_index(const PSLevel *l, int dir)   // Level.prototype.delta_index
{
    switch (dir) {
    case 1: return -1;
    case 2: return 1;
    case 4: return -l->height;
    case 8: return l->height;
    default: return 0;
    }
}

// generateMatchCellRow / generateMatchCellRowWildCard. Instantiated for stride 1/1 (most games:
// <= 32 objects and <= 6 layers) so the per-cell word loops collapse to single tests.
static bool cpat_replace(PSSession *s, const CPat *p, const CRule *cr, int currentIndex);

// apply != NULL (single-cell rules): replace each match as it is found instead of collecting the
// matches first. Same result: a replacement only changes its own cell (which no later match
// reads) and only adds bits to the row/col masks of rows/columns already scanned or tested.
// out->len then counts the matches and *changed collects the replacement results.
static ALWAYS_INLINE void matchCellRow_(PSSession *s, int direction, const CRow *row, int d, IntVec *out, int so, int sm,
                                        const CRule *apply, bool *changed)
{
    PSLevel *l = &s->level;
    out->len = 0;
    if (!smask_in(&row->smask, l->mapCellContents, l->mapCellContents_Movements)) return;
    int wc = row->ellipsisCount;
    int xmin = 0, xmax = l->width, ymin = 0, ymax = l->height;
    int len = row->len - wc;
    switch (direction) {
    case 1: ymin += len - 1; break;
    case 2: ymax -= len - 1; break;
    case 4: xmin += len - 1; break;
    case 8: xmax -= len - 1; break;
    default: break;
    }
    const uint32_t *objs = l->objects, *movs = l->movements;
    int h = l->height;
    bool horizontal = direction > 2;
    const uint32_t qw = row->qw, qa = row->qa, qb = row->qb;
    if (horizontal) {
        for (int y = ymin; y < ymax; y++) {
            if (!smask_in(&row->smask, l->rowCellContents + (size_t)y * so, l->rowCellContents_Movements + (size_t)y * sm)) continue;
            for (int x = xmin; x < xmax; x++) {
                int i = x * h + y;
                { uint32_t v = objs[(size_t)i * so + qw]; if ((v & qa) != qa || (v & qb)) continue; }
                if (wc == 0) {
                    if (row_match0(row, i, d, objs, movs, so, sm)) {
                        if (apply) { out->len++; if (cpat_replace(s, &row->cells[0], apply, i)) *changed = true; }
                        else ivec_push(out, i);
                    }
                } else {
                    int kmax = direction == 4 ? x - len + 2 : l->width - (x + len) + 1;
                    if (wc == 1) row_match1(row, i, kmax, 0, d, objs, movs, so, sm, out);
                    else row_match2(row, i, kmax, 0, kmax, 0, kmax, 0, d, objs, movs, so, sm, out);
                }
            }
        }
    } else {
        for (int x = xmin; x < xmax; x++) {
            if (!smask_in(&row->smask, l->colCellContents + (size_t)x * so, l->colCellContents_Movements + (size_t)x * sm)) continue;
            for (int y = ymin; y < ymax; y++) {
                int i = x * h + y;
                { uint32_t v = objs[(size_t)i * so + qw]; if ((v & qa) != qa || (v & qb)) continue; }
                if (wc == 0) {
                    if (row_match0(row, i, d, objs, movs, so, sm)) {
                        if (apply) { out->len++; if (cpat_replace(s, &row->cells[0], apply, i)) *changed = true; }
                        else ivec_push(out, i);
                    }
                } else {
                    int kmax = direction == 2 ? l->height - (y + len) + 1 : y - len + 2;
                    if (wc == 1) row_match1(row, i, kmax, 0, d, objs, movs, so, sm, out);
                    else row_match2(row, i, kmax, 0, kmax, 0, kmax, 0, d, objs, movs, so, sm, out);
                }
            }
        }
    }
}

static void matchCellRow(PSSession *s, int direction, const CRow *row, int d, IntVec *out)
{
    if (s->so == 1 && s->sm == 1) matchCellRow_(s, direction, row, d, out, 1, 1, NULL, NULL);
    else matchCellRow_(s, direction, row, d, out, s->so, s->sm, NULL, NULL);
}
// findMatches + the replacement loop of tryApply for a single-cell rule, in one scan.
// Returns false when nothing matched (*changed: some replacement changed the level).
static bool matchApplySingle(PSSession *s, const CRule *cr, bool *changed)
{
    PSLevel *l = &s->level;
    if (cr->alwaysFail || !gate_open(&cr->gate, l)) return false;
    if (!smask_in(&cr->ruleMask, l->mapCellContents, NULL)) return false;
    IntVec *m = &s->rowMatches[0];
    if (s->so == 1 && s->sm == 1) matchCellRow_(s, cr->direction, &cr->rows[0], 0, m, 1, 1, cr, changed);
    else matchCellRow_(s, cr->direction, &cr->rows[0], 0, m, s->so, s->sm, cr, changed);
    bool any = m->len > 0;
    m->len = 0;
    return any;
}

// Rule.prototype.findMatches: fills s->rowMatches[0..nrows); returns false when there are none.
static bool findMatches(PSSession *s, const CRule *cr)
{
    PSLevel *l = &s->level;
    if (cr->alwaysFail || !gate_open(&cr->gate, l)) return false;
    if (!smask_in(&cr->ruleMask, l->mapCellContents, NULL)) return false;
    int d = delta_index(l, cr->direction);
    for (int i = 0; i < cr->nrows; i++) {
        matchCellRow(s, cr->direction, &cr->rows[i], d, &s->rowMatches[i]);
        if (s->rowMatches[i].len == 0) return false;
    }
    return true;
}

static inline int row_stride(const CRow *r) { return 1 + r->ellipsisCount; }

// ---------------------------------------------------------------------------------------------
// replacement (CellPattern.prototype.replace / generateReplaceFunction / generateSparseReplaceFunction)

static bool cpat_replace(PSSession *s, const CPat *p, const CRule *cr, int currentIndex)
{
    PSLevel *l = &s->level;
    int so = s->so, sm = s->sm, h = l->height;
    if (p->repKind == REP_NONE) return false;
    uint32_t *objs = l->objects + (size_t)currentIndex * so;
    uint32_t *movs = l->movements + (size_t)currentIndex * sm;
    int row = currentIndex % h, col = currentIndex / h;
    if (p->repKind == REP_SPARSE) {
        bool changed = false;
        for (int k = 0; k < p->nRO && !changed; k++) {
            uint32_t o = objs[p->ro[k].w];
            if (((o & ~p->ro[k].a) | p->ro[k].b) != o) changed = true;
        }
        for (int k = 0; k < p->nRM && !changed; k++) {
            uint32_t m = movs[p->rm[k].w];
            if (((m & ~p->rm[k].a) | p->rm[k].b) != m) changed = true;
        }
        if (!changed) return false;
        for (int k = 0; k < p->nRO; k++) {
            int w = (int)p->ro[k].w;
            uint32_t o = objs[w], n = (o & ~p->ro[k].a) | p->ro[k].b;
            if (n != o) {
                objs[w] = n;
                or_word(&s->sfxCreateMask[w], n & ~o);
                or_word(&s->sfxDestroyMask[w], o & ~n);
            }
            or_word(&l->rowCellContents[row * so + w], n);
            or_word(&l->colCellContents[col * so + w], n);
            or_word(&l->mapCellContents[w], n);
        }
        for (int k = 0; k < p->nRM; k++) {
            int w = (int)p->rm[k].w;
            uint32_t m = movs[w], n = (m & ~p->rm[k].a) | p->rm[k].b;
            if (n != m) { movs[w] = n; s->movesZero = false; }
            or_word(&l->rowCellContents_Movements[row * sm + w], n);
            or_word(&l->colCellContents_Movements[col * sm + w], n);
            or_word(&l->mapCellContents_Movements[w], n);
        }
        return true;
    }

    if (so == 1 && sm <= 8 && !cr->rigid) {
        // generateReplaceFunction for one object word (in registers) and a few movement words
        const uint32_t *gen = p->gen;
        uint32_t oset = gen[0], oclr = gen[1], mset[8], mclr[8];
        for (int i = 0; i < sm; i++) { mset[i] = gen[2 + i]; mclr[i] = gen[2 + sm + i]; }
        if (p->nChoices > 0) {
            int k = rng_index(s, (uint32_t)p->nChoices);
            int rand = p->choices[k], layer = p->choiceLayer[k];
            oset |= 1u << (rand & 31);
            oclr |= s->g->layerMasks[layer][0];
            ishiftor(mclr, sm, 0x1f, 5 * layer);
        }
        for (int k = 0; k < p->nRandDir; k++) {
            int bit = rng_index(s, 4) + 5 * p->randDirLayers[k];
            if ((bit >> 5) < sm) mset[bit >> 5] |= 1u << (bit & 31);
        }
        uint32_t o = objs[0], n = (o & ~oclr) | oset, diff = n ^ o;
        for (int i = 0; i < sm; i++) { mset[i] = (movs[i] & ~mclr[i]) | mset[i]; diff |= mset[i] ^ movs[i]; }
        if (!diff) return false;
        if (n != o) {
            or_word(&s->sfxCreateMask[0], n & ~o);
            or_word(&s->sfxDestroyMask[0], o & ~n);
            objs[0] = n;
        }
        or_word(&l->colCellContents[col], n);
        or_word(&l->rowCellContents[row], n);
        or_word(&l->mapCellContents[0], n);
        uint32_t *rowM = l->rowCellContents_Movements + (size_t)row * sm, *colM = l->colCellContents_Movements + (size_t)col * sm;
        for (int i = 0; i < sm; i++) {
            uint32_t nm = mset[i];
            if (movs[i] != nm) { movs[i] = nm; s->movesZero = false; }
            or_word(&colM[i], nm);
            or_word(&rowM[i], nm);
            or_word(&l->mapCellContents_Movements[i], nm);
        }
        return true;
    }

    const PSReplacement *r = p->rep;
    uint32_t stk[4 * 2 * 16];   // set/clear and the new cell, for objects and movements (stack: fast on device)
    bool onStack = so <= 16 && sm <= 16;
    uint32_t *objectsSet = onStack ? stk : s->o1, *objectsClear = onStack ? stk + 16 : s->o2;
    uint32_t *movementsSet = onStack ? stk + 32 : s->m1, *movementsClear = onStack ? stk + 48 : s->m2;
    // (small fixed-size copies/compares as loops: so/sm are 1..a few words, memcpy/memcmp calls cost more)
    const uint32_t *gen = p->gen;
    for (int i = 0; i < so; i++) { objectsSet[i] = gen[i]; objectsClear[i] = gen[so + i]; }
    for (int i = 0; i < sm; i++) { movementsSet[i] = gen[2 * so + i]; movementsClear[i] = gen[2 * so + sm + i]; }
    if (p->nChoices > 0) {
        int k = rng_index(s, (uint32_t)p->nChoices);
        int rand = p->choices[k];
        int layer = p->choiceLayer[k];
        objectsSet[rand >> 5] |= 1u << (rand & 31);
        const uint32_t *lm = s->g->layerMasks[layer];
        for (int i = 0; i < so; i++) objectsClear[i] |= lm[i];
        ishiftor(movementsClear, sm, 0x1f, 5 * layer);
    }
    for (int k = 0; k < p->nRandDir; k++) {
        int layerIndex = p->randDirLayers[k];
        int randomDir = rng_index(s, 4);
        int bit = randomDir + 5 * layerIndex;
        movementsSet[bit >> 5] |= 1u << (bit & 31);
    }
    uint32_t *curCell = onStack ? stk + 64 : s->o2_5, *curMov = onStack ? stk + 80 : s->m3;
    uint32_t diff = 0;
    for (int i = 0; i < so; i++) { curCell[i] = (objs[i] & ~objectsClear[i]) | objectsSet[i]; diff |= curCell[i] ^ objs[i]; }
    for (int i = 0; i < sm; i++) { curMov[i] = (movs[i] & ~movementsClear[i]) | movementsSet[i]; diff |= curMov[i] ^ movs[i]; }

    bool rigidchange = false;
    if (cr->rigid && l->rigidGroupIndexMask) {
        uint32_t rigidMask[32];
        uint32_t *rm = sm <= 32 ? rigidMask : xalloc((size_t)sm * 4);
        memset(rm, 0, (size_t)sm * 4);
        for (int layer = 0; layer < l->layerCount; layer++) ishiftor(rm, sm, (uint32_t)cr->rigidGroupIndex, layer * 5);
        uint32_t *cg = l->rigidGroupIndexMask + (size_t)currentIndex * sm;
        uint32_t *ca = l->rigidMovementAppliedMask + (size_t)currentIndex * sm;
        bool nz = false;
        for (int i = 0; i < sm; i++) {
            rm[i] &= r->movementsLayerMask[i];
            rm[i] &= ~ca[i];
            if (rm[i]) nz = true;
        }
        if (nz) {
            for (int i = 0; i < sm; i++) { cg[i] |= rm[i]; ca[i] |= r->movementsLayerMask[i]; }
            rigidchange = true;
        }
        if (rm != rigidMask) ps_free(rm);
    }
    if (!diff && !rigidchange) return false;
    uint32_t *rowC = l->rowCellContents + (size_t)row * so, *colC = l->colCellContents + (size_t)col * so;
    for (int i = 0; i < so; i++) {
        uint32_t o = objs[i], n = curCell[i];
        if (n != o) {
            or_word(&s->sfxCreateMask[i], n & ~o);
            or_word(&s->sfxDestroyMask[i], o & ~n);
            objs[i] = n;
        }
        or_word(&colC[i], n);
        or_word(&rowC[i], n);
        or_word(&l->mapCellContents[i], n);
    }
    uint32_t *rowM = l->rowCellContents_Movements + (size_t)row * sm, *colM = l->colCellContents_Movements + (size_t)col * sm;
    for (int i = 0; i < sm; i++) {
        uint32_t n = curMov[i];
        if (movs[i] != n) { movs[i] = n; s->movesZero = false; }
        or_word(&colM[i], n);
        or_word(&rowM[i], n);
        or_word(&l->mapCellContents_Movements[i], n);
    }
    return true;
}

// tuple element accessors: tuple[r] for row r is at rowMatches[r].d[idx[r]*stride].
// Re-check one tuple against the current level (the JS re-runs the row match function with the
// gap ranges pinned to the tuple's own gaps, which is exactly one candidate per row).
static bool tuple_recheck(PSSession *s, const CRule *cr, const int *tidx, int d)
{
    PSLevel *l = &s->level;
    int so = s->so, sm = s->sm;
    const uint32_t *objs = l->objects, *movs = l->movements;
    for (int r = 0; r < cr->nrows; r++) {
        const CRow *row = &cr->rows[r];
        const int *t = s->rowMatches[r].d + (size_t)tidx[r] * row_stride(row);
        bool ok;
        if (row->ellipsisCount == 0) ok = row_match0(row, t[0], d, objs, movs, so, sm);
        else if (row->ellipsisCount == 1)
            ok = row_head(row, t[0], d, objs, movs, so, sm) && row_tail1(row, t[0], t[1], d, objs, movs, so, sm);
        else
            ok = row_head(row, t[0], d, objs, movs, so, sm) && row_tail1(row, t[0], t[1], d, objs, movs, so, sm) &&
                 row_tail2(row, t[0], t[1], t[2], d, objs, movs, so, sm);
        if (!ok) return false;
    }
    return true;
}

// Rule.prototype.generateApplyAt
static bool applyAt(PSSession *s, const CRule *cr, const int *tidx, bool check, int delta)
{
    if (check && !tuple_recheck(s, cr, tidx, delta)) return false;
    bool result = false;
    for (int r = 0; r < cr->nrows; r++) {
        const CRow *row = &cr->rows[r];
        const int *t = s->rowMatches[r].d + (size_t)tidx[r] * row_stride(row);
        int ellipse_index = 0;
        int currentIndex = t[0];
        for (int c = 0; c < row->len; c++) {
            const CPat *p = &row->cells[c];
            if (p->isEllipsis) {
                int k = t[1 + ellipse_index];
                ellipse_index++;
                currentIndex += delta * k;
            } else {
                result = cpat_replace(s, p, cr, currentIndex) || result;
                currentIndex += delta;
            }
        }
    }
    return result;
}

static bool cmdq_has(const PSLevel *l, int t)
{
    for (int i = 0; i < l->commandQueueLen; i++) if (l->commandQueue[i] == t) return true;
    return false;
}

// Rule.prototype.queueCommands
static void queueCommands(PSSession *s, const PSRule *r)
{
    PSLevel *l = &s->level;
    if (r->commandCount == 0) return;
    bool preexisting_cancel = cmdq_has(l, PS_CMD_CANCEL);
    bool preexisting_restart = cmdq_has(l, PS_CMD_RESTART);
    bool currule_cancel = false, currule_restart = false;
    for (int i = 0; i < r->commandCount; i++) {
        if (r->commands[i].type == PS_CMD_CANCEL) currule_cancel = true;
        else if (r->commands[i].type == PS_CMD_RESTART) currule_restart = true;
    }
    if (preexisting_cancel) return;
    if (preexisting_restart && !currule_cancel) return;
    if (currule_cancel || currule_restart) {
        l->commandQueueLen = 0;
        set_messagetext(s, "");
    }
    for (int i = 0; i < r->commandCount; i++) {
        int t = r->commands[i].type;
        if (cmdq_has(l, t)) continue;
        if (l->commandQueueLen < PS_CMD_COUNT) l->commandQueue[l->commandQueueLen++] = t;
        if (t == PS_CMD_MESSAGE) set_messagetext(s, r->commands[i].message);
    }
}

// mixed-radix tuple index -> per-row indices (generateTuples order: row 0 varies fastest)
static void tuple_decompose(PSSession *s, const CRule *cr, size_t t, int *tidx)
{
    for (int r = 0; r < cr->nrows; r++) {
        size_t n = (size_t)(s->rowMatches[r].len / row_stride(&cr->rows[r]));
        tidx[r] = (int)(t % n);
        t /= n;
    }
}
static size_t tuple_count(PSSession *s, const CRule *cr)
{
    size_t total = 1;
    for (int r = 0; r < cr->nrows; r++) total *= (size_t)(s->rowMatches[r].len / row_stride(&cr->rows[r]));
    return total;
}

// Rule.prototype.tryApply
#ifdef PS_RULE_PROFILE   // host profiling (tests/stress_prof): time spent per rule / turn phase
extern void ps_prof_begin(void);
extern void ps_prof_end(int lineNumber, bool matched);
#define PROF_PHASE(id, stmt) do { ps_prof_begin(); stmt; ps_prof_end(8100 + (id), true); } while (0)
static bool tryApply_(PSSession *s, const CRule *cr);
static bool tryApply(PSSession *s, const CRule *cr)
{
    ps_prof_begin();
    bool r = tryApply_(s, cr);
    ps_prof_end(cr->r->lineNumber, r);
    return r;
}
#define tryApply tryApply_
#endif
static bool tryApply(PSSession *s, const CRule *cr)
{
    PSLevel *l = &s->level;
    int delta = delta_index(l, cr->direction);
    bool result = false;
    if (cr->singleCell && cr->hasReplacements) {
        if (!matchApplySingle(s, cr, &result)) return false;
        if (cr->commandCount) queueCommands(s, cr->r);
        return result;
    }
    if (!findMatches(s, cr)) return false;
    if (cr->hasReplacements) {
        if (cr->singleCell) {
            const CPat *cell = &cr->rows[0].cells[0];
            IntVec *m = &s->rowMatches[0];
            for (int k = 0; k < m->len; k++) result = cpat_replace(s, cell, cr, m->d[k]) || result;
        } else {
            // tuples in generateTuples order (row 0 varies fastest) as a mixed-radix counter. The
            // JS re-checks every tuple after the first; until a replacement has changed something
            // since the matches were collected that check cannot fail, so it is skipped.
            int buf[2 * 32];
            int nr = cr->nrows;
            int *ti = nr <= 32 ? buf : xalloc(sizeof(int) * 2 * (size_t)nr), *radix = ti + nr;
            size_t total = 1;
            for (int r = 0; r < nr; r++) {
                ti[r] = 0;
                radix[r] = s->rowMatches[r].len / row_stride(&cr->rows[r]);
                total *= (size_t)radix[r];
            }
            bool dirty = false;
            for (size_t t = 0; t < total; t++) {
                if (t > 0) for (int r = 0; r < nr && ++ti[r] == radix[r]; r++) ti[r] = 0;
                bool success = applyAt(s, cr, ti, dirty, delta);
                if (success) dirty = true;
                result = success || result;
            }
            if (ti != buf) ps_free(ti);
        }
    }
    if (cr->commandCount) queueCommands(s, cr->r);
    return result;
}

#ifdef PS_RULE_PROFILE
#undef tryApply
#else
#define PROF_PHASE(id, stmt) stmt
#endif

static inline int rng_index(PSSession *s, uint32_t n) { s->rngDraws++; return ps_rng_index(s->rngp, n); }
static inline double rng_uniform(PSSession *s) { s->rngDraws++; return ps_rng_uniform(s->rngp); }
static bool over_budget(PSSession *s)
{
    if (ps_over_budget_hook && ps_over_budget_hook()) { s->budgetHit = true; return true; }
    return false;
}
static void rule_log(PSSession *s, const char *msg)
{
    if (s->logCapture) { if (s->nCapLogs < 8) s->capLogs[s->nCapLogs++] = msg; else s->capOverflow = true; }
    if (ps_log_hook) ps_log_hook(msg);
}

// applyRandomRuleGroup
static bool applyRandomRuleGroup(PSSession *s, const CGroup *g)
{
    size_t total = 0;
    for (int i = 0; i < g->count; i++) {
        size_t n = findMatches(s, &g->rules[i]) ? tuple_count(s, &g->rules[i]) : 0;
        s->tupleCounts[i] = (int)n;
        total += n;
    }
    if (total == 0) return false;
    size_t pick = total < 256 ? (size_t)rng_index(s, (uint32_t)total) : (size_t)floor(rng_uniform(s) * (double)total);
    int ri = 0;
    while (ri < g->count && pick >= (size_t)s->tupleCounts[ri]) { pick -= (size_t)s->tupleCounts[ri]; ri++; }
    const CRule *cr = &g->rules[ri];
    findMatches(s, cr);    // deterministic: same matches as above
    int tidx[64];
    int *ti = cr->nrows <= 64 ? tidx : xalloc(sizeof(int) * (size_t)cr->nrows);
    tuple_decompose(s, cr, pick, ti);
    bool modified = applyAt(s, cr, ti, false, delta_index(&s->level, cr->direction));
    if (ti != tidx) ps_free(ti);
    if (cr->commandCount) queueCommands(s, cr->r);
    return modified;
}

// applyRuleGroup
static bool applyRuleGroup(PSSession *s, const CGroup *g)
{
    if (g->count == 0) return false;
    if (g->rules[0].isRandom) return applyRandomRuleGroup(s, g);
    const int MAX_LOOP_COUNT = 200;
    bool hasChanges = false, madeChangeThisLoop = true;
    int loopcount = 0;
    while (madeChangeThisLoop && loopcount++ < MAX_LOOP_COUNT) {
        if (over_budget(s)) { loopcount = MAX_LOOP_COUNT; break; }
        madeChangeThisLoop = false;
        int consecutiveFailures = 0;
        for (int i = 0; i < g->count; i++) {
            if (gate_open(&g->gates[i], &s->level) && tryApply(s, &g->rules[i])) {
                madeChangeThisLoop = true;
                consecutiveFailures = 0;
            } else {
                consecutiveFailures++;
                if (consecutiveFailures == g->count) break;
            }
        }
        if (madeChangeThisLoop) hasChanges = true;
    }
    if (loopcount >= MAX_LOOP_COUNT) rule_log(s, "Got caught looping lots in a rule group :O");
    return hasChanges;
}

// applyRules
static void applyRules(PSSession *s, const CGroup *rules, int count, const int *loopPoint, const bool *bannedGroup)
{
    bool loopPropagated = false;
    int loopCount = 0, idx = 0;
    while (idx < count) {
        if (!bannedGroup || !bannedGroup[idx]) loopPropagated = applyRuleGroup(s, &rules[idx]) || loopPropagated;
        if (loopPropagated && loopPoint && loopPoint[idx] >= 0) {
            idx = loopPoint[idx];
            loopPropagated = false;
            loopCount++;
            if (loopCount > 200 || over_budget(s)) {
                rule_log(s, "got caught in an endless startloop...endloop vortex, escaping!");
                break;
            }
            continue;
        }
        idx++;
        if (idx == count && loopPropagated && loopPoint && loopPoint[idx] >= 0) {
            idx = loopPoint[idx];
            loopPropagated = false;
            loopCount++;
            if (loopCount > 200 || over_budget(s)) {
                rule_log(s, "got caught in an endless startloop...endloop vortex, escaping!");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// movement

// getPlayerPositions
static int getPlayerPositions(PSSession *s, int *out)
{
    PSLevel *l = &s->level;
    const uint32_t *pm = s->g->playerMask;
    int so = s->so, n = 0;
    if (!pm || !l->objects) return 0;
    for (int i = 0; i < l->n_tiles; i++) {
        const uint32_t *c = l->objects + (size_t)i * so;
        if (s->g->playerMaskAggregate ? bits_set_in(pm, c, so) : any_common(pm, c, so)) out[n++] = i;
    }
    return n;
}

static bool vec_includes(const I64Vec *v, int64_t x)
{
    for (int i = 0; i < v->len; i++) if (v->d[i] == x) return true;
    return false;
}

// generate_moveEntitiesAtIndex
static void moveEntitiesAtIndex(PSSession *s, int positionIndex, const uint32_t *entityMask, int dirMask)
{
    PSLevel *l = &s->level;
    int so = s->so, sm = s->sm;
    const uint32_t *cell = l->objects + (size_t)positionIndex * so;
    uint32_t *mov = l->movements + (size_t)positionIndex * sm;
    uint32_t buf[32];
    uint32_t *m = sm <= 32 ? buf : mov;
    s->movesZero = false;
    if (m != mov) memcpy(m, mov, (size_t)sm * 4);
    for (int w = 0; w < s->so; w++) {
        uint32_t bits = cell[w] & entityMask[w];
        while (bits) {
            int i = w * 32 + __builtin_ctz(bits);
            bits &= bits - 1;
            if (i < s->g->objectCount) ishiftor(m, sm, (uint32_t)dirMask, 5 * s->g->objects[i].layer);
        }
    }
    if (m != mov) put_words(mov, m, sm);
    int colIndex = positionIndex / l->height, rowIndex = positionIndex % l->height;
    for (int i = 0; i < sm; i++) {
        or_word(&l->colCellContents_Movements[colIndex * sm + i], mov[i]);
        or_word(&l->rowCellContents_Movements[rowIndex * sm + i], mov[i]);
        or_word(&l->mapCellContents_Movements[i], mov[i]);
    }
}

// startMovement
static int startMovement(PSSession *s, int dir)
{
    int n = getPlayerPositions(s, s->playerPositions);
    for (int i = 0; i < n; i++) moveEntitiesAtIndex(s, s->playerPositions[i], s->g->playerMask, dir);
    return n;
}

// The JS getMovements() hands out 3 rotating buffers; repositionEntitiesAtCell holds one while
// repositionEntitiesOnLayer may fetch more for sfx checks, so every 3rd fetch reloads the held
// mask from level.movements. We reproduce that exactly.
typedef struct { uint32_t *held; int fetches; const uint32_t *src; int sm; } MovRing;

// repositionEntitiesOnLayer
static bool repositionEntitiesOnLayer(PSSession *s, int positionIndex, int layer, int dirMask, MovRing *ring)
{
    PSLevel *l = &s->level;
    int so = s->so, h = l->height;
    int dx = 0, dy = 0;
    switch (dirMask) {
    case 1: dy = -1; break;
    case 2: dy = 1; break;
    case 4: dx = -1; break;
    case 8: dx = 1; break;
    default: break;   // 15 '?', 16 action, 3 'no': [0,0]
    }
    int tx = positionIndex / h, ty = positionIndex % h;
    int maxx = l->width - 1, maxy = h - 1;
    if ((tx == 0 && dx < 0) || (tx == maxx && dx > 0) || (ty == 0 && dy < 0) || (ty == maxy && dy > 0)) return false;
    int targetIndex = positionIndex + dy + dx * h;
    const uint32_t *layerMask = s->g->layerMasks[layer];
    uint32_t tbuf[16], sbuf[16];
    uint32_t *targetMask = so <= 16 ? tbuf : s->o7, *sourceMask = so <= 16 ? sbuf : s->o8;
    memcpy(targetMask, l->objects + (size_t)targetIndex * so, (size_t)so * 4);
    memcpy(sourceMask, l->objects + (size_t)positionIndex * so, (size_t)so * 4);
    if (any_common(layerMask, targetMask, so) && dirMask != 16) return false;

    int nmm = s->g->sfx_MovementMaskCounts ? s->g->sfx_MovementMaskCounts[layer] : 0;
    for (int i = 0; i < nmm; i++) {
        const PSSfxMovementMask *o = &s->g->sfx_MovementMasks[layer][i];
        if (any_common(o->objectMask, sourceMask, so)) {
            const uint32_t *movementMask = l->movements + (size_t)positionIndex * s->sm;
            if (ring && ++ring->fetches % 3 == 0) memcpy(ring->held, ring->src, (size_t)ring->sm * 4);
            if (any_common(movementMask, o->directionMask, s->sm) && !vec_includes(&s->canMove, o->seed))
                VEC_PUSH(s->canMove.d, s->canMove.len, s->canMove.cap, o->seed);
        }
    }
    int colIndex = targetIndex / h, rowIndex = targetIndex % h;
    for (int i = 0; i < so; i++) {
        uint32_t moving = sourceMask[i] & layerMask[i];
        sourceMask[i] &= ~layerMask[i];
        targetMask[i] |= moving;
        or_word(&l->colCellContents[colIndex * so + i], moving);
        or_word(&l->rowCellContents[rowIndex * so + i], moving);
    }
    put_words(l->objects + (size_t)positionIndex * so, sourceMask, so);
    put_words(l->objects + (size_t)targetIndex * so, targetMask, so);
    return true;
}

// generate_repositionEntitiesAtCell
static bool repositionEntitiesAtCell(PSSession *s, int positionIndex)
{
    PSLevel *l = &s->level;
    int sm = s->sm;
    uint32_t *src = l->movements + (size_t)positionIndex * sm;
    if (is_zero(src, sm)) return false;
    uint32_t buf[32];
    uint32_t *movementMask = sm <= 32 ? buf : xalloc((size_t)sm * 4);
    memcpy(movementMask, src, (size_t)sm * 4);
    MovRing ring = { movementMask, 0, src, sm };
    bool moved = false;
    for (int layer = 0; layer < s->layerCount; layer++) {
        uint32_t layerMovement = getshiftor(movementMask, sm, 0x1f, 5 * layer);
        if (layerMovement != 0) {
            if (repositionEntitiesOnLayer(s, positionIndex, layer, (int)layerMovement, &ring)) {
                ishiftclear(movementMask, sm, layerMovement, 5 * layer);
                moved = true;
            }
        }
    }
    int colIndex = positionIndex / l->height, rowIndex = positionIndex % l->height;
    for (int i = 0; i < sm; i++) {
        if (src[i] != movementMask[i]) src[i] = movementMask[i];
        or_word(&l->colCellContents_Movements[colIndex * sm + i], movementMask[i]);
        or_word(&l->rowCellContents_Movements[rowIndex * sm + i], movementMask[i]);
        or_word(&l->mapCellContents_Movements[i], movementMask[i]);
    }
    if (movementMask != buf) ps_free(movementMask);
    return moved;
}

// generate_resolveMovements. bannedGroup has ruleGroupCount+1 entries; the last stands in for
// JS's bannedGroup[undefined].
static bool resolveMovements(PSSession *s, bool *bannedGroup)
{
    PSLevel *l = &s->level;
    const PSGame *g = s->g;
    int so = s->so, sm = s->sm;
    // mapCellContents_Movements is a superset of every movement bit set this turn: when it is empty no
    // cell has movement, so both passes below would only clear what is already clear.
    if (is_zero(l->mapCellContents_Movements, sm)) {
        if (g->rigid && l->rigidGroupIndexMask) {
            zero_words(l->rigidGroupIndexMask, l->n_tiles * sm);
            zero_words(l->rigidMovementAppliedMask, l->n_tiles * sm);
        }
        s->movesZero = true;
        return false;
    }
    // Cells without movement are no-ops below, and colCellContents_Movements[x] stays a superset of
    // column x's movements here (movement is only ever cleared during resolution), so columns
    // whose mask is empty are skipped; the visiting order of the other cells is unchanged.
    bool moved = true;
    int h = l->height;
    while (moved) {
        moved = false;
        for (int x = 0; x < l->width; x++) {
            if (is_zero(l->colCellContents_Movements + (size_t)x * sm, sm)) continue;
            for (int i = x * h; i < x * h + h; i++) moved = repositionEntitiesAtCell(s, i) || moved;
        }
    }
    bool doUndo = false;
    uint32_t mbuf[32];
    uint32_t *movementMask = sm <= 32 ? mbuf : s->m1;
    bool rigidArr = g->rigid && l->rigidGroupIndexMask;
    for (int i = 0; i < l->n_tiles; i++) {
        if (!rigidArr && i % h == 0 && is_zero(l->colCellContents_Movements + (size_t)(i / h) * sm, sm)) { i += h - 1; continue; }
        const uint32_t *cellMask = l->objects + (size_t)i * so;
        uint32_t *mv = l->movements + (size_t)i * sm;
        if (is_zero(mv, sm)) {   // nothing to report or clear (stores are what costs on device)
            if (rigidArr) {
                zero_words(l->rigidGroupIndexMask + (size_t)i * sm, sm);
                zero_words(l->rigidMovementAppliedMask + (size_t)i * sm, sm);
            }
            continue;
        }
        memcpy(movementMask, mv, (size_t)sm * 4);
        {
            if (g->rigid && l->rigidMovementAppliedMask) {
                const uint32_t *rma = l->rigidMovementAppliedMask + (size_t)i * sm;
                if (!is_zero(rma, sm)) {
                    for (int k = 0; k < sm; k++) movementMask[k] &= rma[k];
                    if (!is_zero(movementMask, sm)) {
                        for (int j = 0; j < s->layerCount; j++) {
                            uint32_t layerSection = getshiftor(movementMask, sm, 0x1f, 5 * j);
                            if (layerSection != 0) {
                                int rgi = (int)getshiftor(l->rigidGroupIndexMask + (size_t)i * sm, sm, 0x1f, 5 * j) - 1;
                                int groupIndex = (rgi >= 0 && rgi < g->rigidGroupCount && g->rigidGroupIndex_to_GroupIndex)
                                    ? g->rigidGroupIndex_to_GroupIndex[rgi] : -1;
                                if (groupIndex < 0 || groupIndex >= g->ruleGroupCount) groupIndex = g->ruleGroupCount;
                                if (!bannedGroup[groupIndex]) {
                                    bannedGroup[groupIndex] = true;
                                    doUndo = true;
                                }
                                break;
                            }
                        }
                    }
                }
            }
            for (int j = 0; j < g->sfx_MovementFailureMaskCount; j++) {
                const PSSfxMovementMask *o = &g->sfx_MovementFailureMasks[j];
                if (any_common(cellMask, o->objectMask, so) && any_common(o->directionMask, movementMask, sm) &&
                    !vec_includes(&s->cantMove, o->seed))
                    VEC_PUSH(s->cantMove.d, s->cantMove.len, s->cantMove.cap, o->seed);
            }
        }
        zero_words(mv, sm);
        if (rigidArr) {
            zero_words(l->rigidGroupIndexMask + (size_t)i * sm, sm);
            zero_words(l->rigidMovementAppliedMask + (size_t)i * sm, sm);
        }
    }
    s->movesZero = true;   // every cell with movement was cleared above
    return doUndo;
}

// ---------------------------------------------------------------------------------------------
// turn processing

// playSounds
static void playSounds(PSSession *s)
{
    const PSGame *g = s->g;
    for (int i = 0; i < s->cantMove.len; i++) playSound(s, s->cantMove.d[i], false);
    for (int i = 0; i < s->canMove.len; i++) playSound(s, s->canMove.d[i], false);
    for (int i = 0; i < g->sfx_CreationMaskCount; i++)
        if (any_common(s->sfxCreateMask, g->sfx_CreationMasks[i].objectMask, s->so)) playSound(s, g->sfx_CreationMasks[i].seed, false);
    for (int i = 0; i < g->sfx_DestructionMaskCount; i++)
        if (any_common(s->sfxDestroyMask, g->sfx_DestructionMasks[i].objectMask, s->so)) playSound(s, g->sfx_DestructionMasks[i].seed, false);
}

// showTempMessage
static void showTempMessage(PSSession *s)
{
    s->keybufferLen = 0;
    s->textMode = true;
    s->titleScreen = false;
    s->quittingMessageScreen = false;
    s->messageselected = false;
    s->ignoreNotJustPressedAction = true;
    tryPlaySimpleSound(s, PS_SFX_SHOWMESSAGE);
    drawMessageScreen(s);
    redraw(s);
}

// processOutputCommands (iterates a snapshot; the queue can be cleared underneath)
static void processOutputCommands(PSSession *s)
{
    int q[PS_CMD_COUNT], n = s->level.commandQueueLen;
    memcpy(q, s->level.commandQueue, sizeof(int) * (size_t)n);
    for (int i = 0; i < n; i++) {
        if (q[i] >= PS_CMD_SFX0 && q[i] <= PS_CMD_SFX10) tryPlaySimpleSound(s, PS_SFX_EVENT_SFX0 + (q[i] - PS_CMD_SFX0));
        if (!s->unitTesting && q[i] == PS_CMD_MESSAGE) showTempMessage(s);
    }
}

// level4Serialization -> storage
static void store_checkpoint(PSSession *s)
{
    char *str = serialize_backup(s->restartTarget);
    storage_set(s, KEY_CHECKPOINT, str);
    ps_free(str);
}

// processCommandQueue
static bool processCommandQueue(PSSession *s, PSBackup *bak, bool dontModify, bool dontDoWin, int inputDir)
{
    PSLevel *l = &s->level;
    if (cmdq_has(l, PS_CMD_CANCEL)) {
        if (!dontModify) processOutputCommands(s);
        bool commandsLeft = l->commandQueueLen > 1;
        undo_push_restore(s, bak);
        tryPlaySimpleSound(s, PS_SFX_CANCEL);
        return commandsLeft;
    }
    if (cmdq_has(l, PS_CMD_RESTART)) {
        if (!dontModify) processOutputCommands(s);
        addUndoState(s, bak);
        if (!dontModify) DoRestart(s, true);
    }
    bool modified = false;
    int nw = l->n_tiles * s->so;
    bool differs;
    PROF_PHASE(11, differs = nw != bak->len || memcmp(l->objects, bak->dat, (size_t)(nw < bak->len ? nw : bak->len) * 4) != 0);
    if (differs) {
        if (dontModify) {
            undo_push_restore(s, bak);
            return true;
        } else {
            if (inputDir != -1) {
                PROF_PHASE(10, addUndoState(s, bak));
            } else if (s->nbackups > 0) {
                PSBackup *last = s->backups[s->nbackups - 1];
                size_t ob = backup_bytes(last);
                PSBackup *u = unconsolidateDiff(last, bak);
                s->backups[s->nbackups - 1] = u;
                s->backupBytes += backup_bytes(u) - ob;
            }
            modified = true;
        }
    }
    if (dontModify && (cmdq_has(l, PS_CMD_WIN) || cmdq_has(l, PS_CMD_RESTART))) return true;
    if (!dontModify) {
        playSounds(s);
        processOutputCommands(s);
    }
    if (!s->textMode) PROF_PHASE(12, checkWin(s, dontDoWin));
    if (!s->winning) {
        if (!dontModify && cmdq_has(l, PS_CMD_CHECKPOINT)) {
            backup_free(s->restartTarget);
            s->restartTarget = backupLevel(s);
            s->hasUsedCheckpoint = true;
            store_checkpoint(s);
            storage_set_int(s, KEY_LEVEL, s->curlevel);
        }
        if (cmdq_has(l, PS_CMD_AGAIN) && modified) {
            char *oldMessageText = ps_strdup(s->messagetext);
            bool oldVerbose = s->verbose_logging;
            s->verbose_logging = false;
            bool again;
            PROF_PHASE(9, again = processInput(s, -1, true, true));
            if (again) {
                s->againing = true;
                s->timer = 0;
            }
            s->verbose_logging = oldVerbose;
            set_messagetext(s, oldMessageText);
            ps_free(oldMessageText);
        }
    }
    l->commandQueueLen = 0;
    return modified;
}

// Record the speculative turn's rule-phase outcome (see AgainCache). The objects it started from
// are bak's; processInput hands that buffer over to the cache afterwards instead of copying it.
static void again_capture(PSSession *s, PSLevel *l, const PSBackup *bak)
{
    AgainCache *ac = &s->ac;
    int n = l->n_tiles * s->so;
    if (bak->len != n) return;
    ac->nDiff = 0;
    const uint32_t *a = bak->dat, *o = l->objects;
    for (int i = 0; i < n; i++) {
        if (a[i] == o[i]) continue;
        if (ac->nDiff == ac->capDiff) {
            ac->capDiff = ac->capDiff ? ac->capDiff * 2 : 64;
            ac->diffIdx = xrealloc(ac->diffIdx, sizeof(int) * (size_t)ac->capDiff);
            ac->diffVal = xrealloc(ac->diffVal, sizeof(uint32_t) * (size_t)ac->capDiff);
        }
        ac->diffIdx[ac->nDiff] = i; ac->diffVal[ac->nDiff] = o[i]; ac->nDiff++;
    }
    ac->n = n; ac->width = l->width; ac->height = l->height;
    memcpy(ac->cmdq, l->commandQueue, sizeof(int) * (size_t)l->commandQueueLen);
    ac->cmdqLen = l->commandQueueLen;
    memcpy(ac->msgAfter, s->messagetext, strlen(s->messagetext) + 1);
    if (!ac->sfxC) { ac->sfxC = xalloc((size_t)s->so * 4); ac->sfxD = xalloc((size_t)s->so * 4); }
    memcpy(ac->sfxC, s->sfxCreateMask, (size_t)s->so * 4);
    memcpy(ac->sfxD, s->sfxDestroyMask, (size_t)s->so * 4);
    ac->can.len = 0; ac->cant.len = 0;
    for (int i = 0; i < s->canMove.len; i++) VEC_PUSH(ac->can.d, ac->can.len, ac->can.cap, s->canMove.d[i]);
    for (int i = 0; i < s->cantMove.len; i++) VEC_PUSH(ac->cant.d, ac->cant.len, ac->cant.cap, s->cantMove.d[i]);
    ac->rngDraws = s->rngDraws;
    memcpy(ac->logs, s->capLogs, sizeof(const char *) * (size_t)s->nCapLogs);
    ac->nLogs = s->nCapLogs;
    ac->armed = true;
}

#ifdef PS_AGAIN_CACHE_CHECK
#include <stdio.h>
#include <stdlib.h>
static uint64_t againHits;
static bool again_replay_ok(PSSession *s, PSLevel *l, const PSGame *g)
{
    AgainCache *ac = &s->ac;
    int n = l->n_tiles * s->so;
    return !(g->rigid || !s->movesZero || s->rngDraws != ac->rngDraws || n != ac->n || l->width != ac->width ||
        l->height != ac->height || strcmp(s->messagetext, ac->msgBefore) != 0 || memcmp(l->objects, ac->start, (size_t)n * 4) != 0);
}
static void again_check(PSSession *s, PSLevel *l)
{
    AgainCache *ac = &s->ac;
    for (int k = 0; k < ac->nDiff; k++) ac->start[ac->diffIdx[k]] = ac->diffVal[k];
    bool ok = memcmp(l->objects, ac->start, (size_t)ac->n * 4) == 0 && l->commandQueueLen == ac->cmdqLen &&
              memcmp(l->commandQueue, ac->cmdq, sizeof(int) * (size_t)ac->cmdqLen) == 0 && strcmp(s->messagetext, ac->msgAfter) == 0 &&
              memcmp(s->sfxCreateMask, ac->sfxC, (size_t)s->so * 4) == 0 && memcmp(s->sfxDestroyMask, ac->sfxD, (size_t)s->so * 4) == 0 &&
              s->canMove.len == ac->can.len && s->cantMove.len == ac->cant.len && s->movesZero &&
              (!ac->can.len || memcmp(s->canMove.d, ac->can.d, sizeof(int64_t) * (size_t)ac->can.len) == 0) &&
              (!ac->cant.len || memcmp(s->cantMove.d, ac->cant.d, sizeof(int64_t) * (size_t)ac->cant.len) == 0);
    if (!ok) { fprintf(stderr, "again cache mismatch\n"); abort(); }
    againHits++;
    if (getenv("PS_AGAIN_STATS") && (againHits & (againHits - 1)) == 0) fprintf(stderr, "again cache: %llu verified hits\n", (unsigned long long)againHits);
}
#endif

// A real again turn starting from the speculative turn's starting point: apply its outcome instead
// of running the rules again. false (nothing changed) when anything that phase reads differs.
#ifdef PS_AGAIN_CACHE_CHECK
__attribute__((unused))
#endif
static bool again_replay(PSSession *s, PSLevel *l, const PSGame *g)
{
    AgainCache *ac = &s->ac;
    ac->valid = false;   // one use
    int n = l->n_tiles * s->so;
    if (g->rigid || !s->movesZero || s->rngDraws != ac->rngDraws || n != ac->n || l->width != ac->width ||
        l->height != ac->height || strcmp(s->messagetext, ac->msgBefore) != 0 || memcmp(l->objects, ac->start, (size_t)n * 4) != 0)
        return false;
    for (int k = 0; k < ac->nDiff; k++) l->objects[ac->diffIdx[k]] = ac->diffVal[k];
    memcpy(l->commandQueue, ac->cmdq, sizeof(int) * (size_t)ac->cmdqLen);
    l->commandQueueLen = ac->cmdqLen;
    set_messagetext(s, ac->msgAfter);
    memcpy(s->sfxCreateMask, ac->sfxC, (size_t)s->so * 4);
    memcpy(s->sfxDestroyMask, ac->sfxD, (size_t)s->so * 4);
    s->canMove.len = 0; s->cantMove.len = 0;
    for (int i = 0; i < ac->can.len; i++) VEC_PUSH(s->canMove.d, s->canMove.len, s->canMove.cap, ac->can.d[i]);
    for (int i = 0; i < ac->cant.len; i++) VEC_PUSH(s->cantMove.d, s->cantMove.len, s->cantMove.cap, ac->cant.d[i]);
    for (int i = 0; i < ac->nLogs; i++) if (ps_log_hook) ps_log_hook(ac->logs[i]);
    return true;
}

// processInput
static bool processInput(PSSession *s, int dir, bool dontDoWin, bool dontModify)
{
    if (!s->levelLoaded) { s->againing = false; return false; }
    if (s->rngp != &s->rng) return processInput_rng(s, dir, dontDoWin, dontModify);
    // The RC4 state is rewritten on every random draw; keep it in stack memory (fast TCM on the
    // device; the heap is write-through SDRAM) for the turn and store back the words that changed.
    PSRng local = s->rng;
    s->rngp = &local;
    bool r = processInput_rng(s, dir, dontDoWin, dontModify);
    s->rngp = &s->rng;
    put_words((uint32_t *)&s->rng, (const uint32_t *)&local, (int)(sizeof(PSRng) / 4));
    return r;
}

static bool processInput_rng(PSSession *s, int dir, bool dontDoWin, bool dontModify)
{
    PSLevel *l = &s->level;
    const PSGame *g = s->g;
    s->againing = false;
    int depth = s->inputDepth;
    PSBackup *bak;
    if (depth < TURN_DEPTH_MAX) {
        bak = &s->turnBak[depth];
        PROF_PHASE(5, backupLevelInto(s, bak, &s->turnBakCap[depth]));
    } else {
        bak = backupLevel(s);
    }
    s->inputDepth++;
    bool modified;
    PROF_PHASE(6, modified = processInput_(s, l, g, bak, dir, dontDoWin, dontModify));
    s->inputDepth--;
    if (s->ac.armed) {   // set by again_capture in this (speculative) turn: bak holds its start objects
        s->ac.armed = false;
        if (depth < TURN_DEPTH_MAX) {
            uint32_t *t = s->ac.start; int tc = s->ac.startCap;
            s->ac.start = bak->dat; s->ac.startCap = s->turnBakCap[depth];
            bak->dat = t; s->turnBakCap[depth] = t ? tc : 0;
            s->ac.valid = true;
        }
    }
    if (depth >= TURN_DEPTH_MAX) backup_free(bak);
    return modified;
}

static bool processInput_(PSSession *s, PSLevel *l, const PSGame *g, PSBackup *bak, int dir, bool dontDoWin, bool dontModify)
{
    int inputindex = dir;
    int nPlayer = 0;
    bool movesZeroBefore = s->movesZero;
    if (dir >= 0) {
        switch (dir) {
        case 0: dir = 1; break;
        case 1: dir = 4; break;
        case 2: dir = 2; break;
        case 3: dir = 8; break;
        case 4: dir = 16; break;
        default: break;
        }
        movesZeroBefore = s->movesZero;
        PROF_PHASE(3, nPlayer = startMovement(s, dir));
    }
    bool speculative = inputindex == -1 && dontModify && dontDoWin;
#ifndef PS_AGAIN_CACHE_CHECK
    if (inputindex == -1 && !dontModify && s->ac.valid && again_replay(s, l, g)) goto commands;
#else   // test build: run the rules anyway and check the replay would have produced the same state
    bool checkReplay = inputindex == -1 && !dontModify && s->ac.valid && again_replay_ok(s, l, g);
    s->ac.valid = false;
#endif
    if (speculative) {
        s->ac.armed = false;
        s->budgetHit = false; s->logCapture = true; s->nCapLogs = 0; s->capOverflow = false;
        size_t ml = strlen(s->messagetext);
        if (ml < AC_MSG) memcpy(s->ac.msgBefore, s->messagetext, ml + 1); else s->ac.msgBefore[0] = 1;   // (never matches)
    }
    unsigned rngDraws0 = s->rngDraws;
    bool *bannedGroup = s->bannedGroup;
    memset(bannedGroup, 0, sizeof(bool) * (size_t)(g->ruleGroupCount + 1));
    l->commandQueueLen = 0;
    bool rigidloop = false;
    size_t objBytes = (size_t)l->n_tiles * (size_t)s->so * 4, movBytes = (size_t)l->n_tiles * (size_t)s->sm * 4;
    uint32_t *startObjects = NULL, *startMovements = NULL;
    if (g->rigid) {
        size_t need = objBytes > movBytes ? objBytes : movBytes;
        if (need > s->startCap || !s->startObjects) {
            ps_free(s->startObjects); ps_free(s->startMovements);
            s->startCap = need ? need : 4;
            s->startObjects = xalloc(s->startCap);
            s->startMovements = xalloc(s->startCap);
        }
        startObjects = s->startObjects;
        startMovements = s->startMovements;
        memcpy(startObjects, l->objects, objBytes);
        memcpy(startMovements, l->movements, movBytes);
    }
    memset(s->sfxCreateMask, 0, (size_t)s->so * 4);
    memset(s->sfxDestroyMask, 0, (size_t)s->so * 4);
    s->canMove.len = 0;
    s->cantMove.len = 0;
    PROF_PHASE(4, calculateRowColMasks(s, movesZeroBefore ? s->playerPositions : NULL, nPlayer));

    int i = 0;
    do {
        rigidloop = false;
        i++;
        PROF_PHASE(7, applyRules(s, s->groups, g->ruleGroupCount, g->loopPoint, bannedGroup));
        bool shouldUndo;
        PROF_PHASE(1, shouldUndo = resolveMovements(s, bannedGroup));
        if (shouldUndo) {
            rigidloop = true;
            if (startObjects) {
                memcpy(l->objects, startObjects, objBytes);
                memcpy(l->movements, startMovements, movBytes);
                s->movesZero = false;
            }
            // rigid masks were all zeroed by resolveMovements (the JS restores shared references)
            l->commandQueueLen = 0;
            memset(s->sfxCreateMask, 0, (size_t)s->so * 4);
            memset(s->sfxDestroyMask, 0, (size_t)s->so * 4);
            s->canMove.len = 0;
        } else {
            PROF_PHASE(8, applyRules(s, s->lateGroups, g->lateRuleGroupCount, g->lateLoopPoint, NULL));
        }
    } while (i < 50 && rigidloop && !over_budget(s));
    if (i >= 50) rule_log(s, "Looped through 50 times, gave up.  too many loops!");
    if (speculative) {
        s->logCapture = false;
        if (!g->rigid && s->movesZero && s->rngDraws == rngDraws0 && !s->budgetHit && !s->capOverflow &&
            s->ac.msgBefore[0] != 1 && strlen(s->messagetext) < AC_MSG)
            again_capture(s, l, bak);
    }

    if (nPlayer > 0 && g->metadata.require_player_movement) {
        bool somemoved = false;
        for (int k = 0; k < nPlayer; k++) {
            const uint32_t *c = l->objects + (size_t)s->playerPositions[k] * s->so;
            bool clear = true;
            for (int w = 0; w < s->so; w++) if (g->playerMask[w] & c[w]) { clear = false; break; }
            if (clear) { somemoved = true; break; }
        }
        if (!somemoved) {
            undo_push_restore(s, bak);
            set_messagetext(s, "");
            s->textMode = false;
            return false;
        }
    }
#ifdef PS_AGAIN_CACHE_CHECK
    if (checkReplay) again_check(s, l);
#else
commands:;
#endif
    bool modified;
    PROF_PHASE(2, modified = processCommandQueue(s, bak, dontModify, dontDoWin, inputindex));
    if (s->winning) s->againing = false;
    return modified;
}

// checkWin
static void checkWin(PSSession *s, bool dontDoWin)
{
    PSLevel *l = &s->level;
    const PSGame *g = s->g;
    int so = s->so;
    if (s->levelEditorOpened) dontDoWin = true;
    if (cmdq_has(l, PS_CMD_WIN)) {
        if (!dontDoWin) {
            extern void ps__DoWin(PSSession *s);
            ps__DoWin(s);
        }
        return;
    }
    if (g->winconditionCount == 0) return;
    // All conditions in one pass over the level (the result is their AND; a separate pass per
    // condition re-reads the whole level each time, which is what costs on device).
    enum { WC_MAX = 64 };
    int nwc = g->winconditionCount;
    bool passed = true;
    if (nwc <= WC_MAX) {
        bool found[WC_MAX];
        int needSome = 0;
        for (int wc = 0; wc < nwc; wc++) { found[wc] = false; if (g->winconditions[wc].quantifier == 0) needSome++; }
        for (int i = 0; i < l->n_tiles && passed; i++) {
            const uint32_t *c = l->objects + (size_t)i * so;
            for (int wc = 0; wc < nwc; wc++) {
                const PSWinCondition *w = &g->winconditions[wc];
#define F1(c) (w->aggr1 ? bits_set_in(w->filter1, (c), so) : any_common(w->filter1, (c), so))
#define F2(c) (w->aggr2 ? bits_set_in(w->filter2, (c), so) : any_common(w->filter2, (c), so))
                switch (w->quantifier) {
                case -1: if (F1(c) && F2(c)) passed = false; break;
                case 0: if (!found[wc] && F1(c) && F2(c)) { found[wc] = true; needSome--; } break;
                case 1: if (F1(c) && !F2(c)) passed = false; break;
                default: break;
                }
                if (!passed) break;
            }
        }
        if (needSome > 0) passed = false;
    } else {
        for (int wc = 0; wc < nwc; wc++) {
            const PSWinCondition *w = &g->winconditions[wc];
            bool rulePassed = true;
            switch (w->quantifier) {
            case -1:
                for (int i = 0; i < l->n_tiles; i++) {
                    const uint32_t *c = l->objects + (size_t)i * so;
                    if (F1(c) && F2(c)) { rulePassed = false; break; }
                }
                break;
            case 0: {
                bool passedTest = false;
                for (int i = 0; i < l->n_tiles; i++) {
                    const uint32_t *c = l->objects + (size_t)i * so;
                    if (F1(c) && F2(c)) { passedTest = true; break; }
                }
                if (!passedTest) rulePassed = false;
                break;
            }
            case 1:
                for (int i = 0; i < l->n_tiles; i++) {
                    const uint32_t *c = l->objects + (size_t)i * so;
                    if (F1(c) && !F2(c)) { rulePassed = false; break; }
                }
                break;
            default: break;
            }
            if (!rulePassed) passed = false;
        }
#undef F1
#undef F2
    }
    if (passed && !dontDoWin) {
        extern void ps__DoWin(PSSession *s);
        ps__DoWin(s);
    }
}

// DoWin
void ps__DoWin(PSSession *s);
void ps__DoWin(PSSession *s)
{
    if (s->winning) return;
    s->againing = false;
    tryPlaySimpleSound(s, PS_SFX_ENDLEVEL);
    if (s->unitTesting) {
        nextLevel(s);
        return;
    }
    s->winning = true;
    s->timer = 0;
}

// DoRestart
static void DoRestart(PSSession *s, bool force)
{
    if (s->restarting) return;
    if (!force && s->g->metadata.norestart) return;
    if (!s->levelLoaded || !s->restartTarget) return;
    if (s->againing) DoUndo(s, force, true);
    s->restarting = true;
    if (!force) addUndoStateOwned(s, backupLevel(s));
    restoreLevel(s, s->restartTarget);
    tryPlaySimpleSound(s, PS_SFX_RESTART);
    if (s->g->metadata.run_rules_on_level_start) processInput(s, -1, true, false);
    s->level.commandQueueLen = 0;
    s->restarting = false;
}

// backupDiffers
static bool backupDiffers(PSSession *s)
{
    if (s->nbackups == 0) return true;
    const PSBackup *bak = s->backups[s->nbackups - 1];
    if (bak->diff) return bak->len != 0;
    int n = s->level.n_tiles * s->so;
    for (int i = 0; i < n; i++) if (i >= bak->len || s->level.objects[i] != bak->dat[i]) return true;
    return false;
}

// DoUndo
static void DoUndo(PSSession *s, bool force, bool ignoreDuplicates)
{
    if (!s->levelEditorOpened && s->g->metadata.noundo && !force) return;
    if (!s->levelLoaded) return;
    if (ignoreDuplicates) while (!backupDiffers(s)) backups_pop(s);
    if (s->nbackups > 0) {
        restoreLevel(s, s->backups[s->nbackups - 1]);
        backups_pop(s);
        if (!force) tryPlaySimpleSound(s, PS_SFX_UNDO);
    }
}

// ---------------------------------------------------------------------------------------------
// level loading

static bool showContinueOptionOnTitleScreen(PSSession *s)
{
    return (s->curlevel > 0 || s->curlevelTarget != NULL) && s->curlevel >= 0 && s->curlevel < s->g->levelCount;
}

// loadLevelFromLevelDat
static void loadLevelFromLevelDat(PSSession *s, int levelIndex, const char *randomseed, bool clearinputhistory)
{
    const PSGame *g = s->g;
    char seedbuf[40];
    if (!randomseed || !*randomseed) {
        // JS: (Math.random() + Date.now()).toString(); the RNG module picks the entropy
        ps_rng_init(s->rngp, NULL); s->rngDraws++;
        randomseed = NULL;
    } else {
        size_t n = strlen(randomseed);
        if (n >= sizeof seedbuf) n = sizeof seedbuf - 1;
        memcpy(seedbuf, randomseed, n); seedbuf[n] = 0;
        ps_rng_init(s->rngp, randomseed); s->rngDraws++;
    }
    s->ignoreNotJustPressedAction = true;
    s->titleScreen = false;
    s->titleMode = showContinueOptionOnTitleScreen(s) ? 1 : 0;
    s->titleSelection = showContinueOptionOnTitleScreen(s) ? 1 : 0;
    s->titleSelected = false;
    s->againing = false;
    if (levelIndex < 0 || levelIndex >= g->levelCount) {
        if (ps_log_hook) ps_log_hook("Trying to access a level that doesn't exist.");
        goToTitleScreen(s);
        return;
    }
    const PSLevelDef *ld = &g->levels[levelIndex];
    if (!ld->isMessage) {
        s->titleMode = 0;
        s->textMode = false;
        PSLevel *l = &s->level;
        l->lineNumber = ld->lineNumber;
        l->width = ld->width;
        l->height = ld->height;
        l->n_tiles = ld->width * ld->height;
        l->layerCount = g->layerCount;
        ps_free(l->objects);
        size_t nb = (size_t)l->n_tiles * (size_t)s->so * 4;
        l->objects = xalloc(nb ? nb : 4);
        s->objCap = l->n_tiles * s->so;
        if (nb) memcpy(l->objects, ld->objects, nb);
        l->commandQueueLen = 0;
        RebuildLevelArrays(s);
        s->levelLoaded = true;
        if (g->metadata.has_flickscreen) {
            s->flickLen = 4;
            s->flick[0] = 0; s->flick[1] = 0;
            s->flick[2] = g->metadata.flickscreen[0] < l->width ? g->metadata.flickscreen[0] : l->width;
            s->flick[3] = g->metadata.flickscreen[1] < l->height ? g->metadata.flickscreen[1] : l->height;
        } else if (g->metadata.has_zoomscreen) {
            s->flickLen = 4;
            s->flick[0] = 0; s->flick[1] = 0;
            s->flick[2] = g->metadata.zoomscreen[0] < l->width ? g->metadata.zoomscreen[0] : l->width;
            s->flick[3] = g->metadata.zoomscreen[1] < l->height ? g->metadata.zoomscreen[1] : l->height;
        }
        backups_clear(s);
        backup_free(s->restartTarget);
        s->restartTarget = backupLevel(s);
        s->keybufferLen = 0;
        if (g->metadata.run_rules_on_level_start) {
            s->runrulesonlevelstart_phase = true;
            processInput(s, -1, true, false);
            s->runrulesonlevelstart_phase = false;
        }
    } else {
        s->ignoreNotJustPressedAction = true;
        tryPlaySimpleSound(s, PS_SFX_SHOWMESSAGE);
        drawMessageScreen(s);
    }
    if (clearinputhistory) clearInputHistory(s);
    redraw(s);
}

// loadLevelFromStateTarget (target is copied)
static void loadLevelFromStateTarget(PSSession *s, int levelindex, const PSBackup *target)
{
    PSBackup *t = backup_clone(target);
    s->curlevel = levelindex;
    if (s->curlevelTarget != target) { backup_free(s->curlevelTarget); s->curlevelTarget = backup_clone(target); }
    tryPlaySimpleSound(s, PS_SFX_STARTLEVEL);
    loadLevelFromLevelDat(s, levelindex, NULL, false);
    if (s->levelLoaded) restoreLevel(s, t);
    backup_free(s->restartTarget);
    s->restartTarget = t;
}

static void set_curlevelTarget_null(PSSession *s) { backup_free(s->curlevelTarget); s->curlevelTarget = NULL; }

// loadLevelFromState
static void loadLevelFromState(PSSession *s, int levelindex, const char *randomseed)
{
    s->curlevel = levelindex;
    set_curlevelTarget_null(s);
    if (levelindex >= 0 && levelindex < s->g->levelCount && !s->g->levels[levelindex].isMessage)
        tryPlaySimpleSound(s, PS_SFX_STARTLEVEL);
    loadLevelFromLevelDat(s, levelindex, randomseed, false);
}

// doSetupTitleScreenLevelContinue (globalVariables.js)
static void doSetupTitleScreenLevelContinue(PSSession *s)
{
    char *lv = storage_get(s, KEY_LEVEL);
    if (!lv) return;
    char *cp = storage_get(s, KEY_CHECKPOINT);
    if (cp) {
        PSBackup *b = parse_backup(cp);
        if (b) { backup_free(s->curlevelTarget); s->curlevelTarget = b; }
        ps_free(cp);
    }
    const char *p = lv;
    long v;
    s->curlevel = parse_int(&p, &v) ? (int)v : 0;
    ps_free(lv);
}

// goToTitleScreen
static void goToTitleScreen(PSSession *s)
{
    s->againing = false;
    set_messagetext(s, "");
    s->titleScreen = true;
    s->textMode = true;
    doSetupTitleScreenLevelContinue(s);
    s->titleSelection = showContinueOptionOnTitleScreen(s) ? 1 : 0;
    generateTitleScreen(s);
}

// nextLevel
static void nextLevel(PSSession *s)
{
    const PSGame *g = s->g;
    bool startedInTextMode = s->textMode;
    s->againing = false;
    set_messagetext(s, "");
    if (s->curlevel > g->levelCount) s->curlevel = g->levelCount - 1;
    s->ignoreNotJustPressedAction = true;
    if (s->titleScreen) {
        if (s->titleSelection == 0) {
            s->curlevel = 0;
            set_curlevelTarget_null(s);
        }
        if (s->curlevelTarget) loadLevelFromStateTarget(s, s->curlevel, s->curlevelTarget);
        else loadLevelFromState(s, s->curlevel, NULL);
    } else {
        if (s->hasUsedCheckpoint) {
            set_curlevelTarget_null(s);
            s->hasUsedCheckpoint = false;
        }
        if (s->curlevel < g->levelCount - 1) {
            s->curlevel++;
            set_curlevelTarget_null(s);
            s->textMode = false;
            s->titleScreen = false;
            s->quittingMessageScreen = false;
            s->messageselected = false;
            loadLevelFromState(s, s->curlevel, NULL);
        } else {
            storage_remove(s, KEY_LEVEL);
            storage_remove(s, KEY_CHECKPOINT);
            s->curlevel = 0;
            set_curlevelTarget_null(s);
            goToTitleScreen(s);
            tryPlaySimpleSound(s, PS_SFX_ENDGAME);
        }
    }
    if (startedInTextMode) clearInputHistory(s);
    storage_set_int(s, KEY_LEVEL, s->curlevel);
    if (s->curlevelTarget) {
        if (s->levelLoaded) {
            backup_free(s->restartTarget);
            s->restartTarget = backupLevel(s);
            store_checkpoint(s);
        }
    } else {
        storage_remove(s, KEY_CHECKPOINT);
    }
    if (g->metadata.has_flickscreen && s->levelLoaded) {
        s->flickLen = 4;
        s->flick[0] = 0; s->flick[1] = 0;
        s->flick[2] = g->metadata.flickscreen[0] < s->level.width ? g->metadata.flickscreen[0] : s->level.width;
        s->flick[3] = g->metadata.flickscreen[1] < s->level.height ? g->metadata.flickscreen[1] : s->level.height;
    }
    redraw(s);
}

// setGameState (session is created per game, so the "rebuild" path is not needed)
static void setGameState(PSSession *s, const char *cmd, int levelArg, const char *randomseed)
{
    const PSGame *g = s->g;
    s->flickLen = 0;
    s->timer = 0;
    s->autotick = 0;
    s->winning = false;
    s->againing = false;
    s->messageselected = false;
    memset(s->sfxCreateMask, 0, (size_t)s->so * 4);
    memset(s->sfxDestroyMask, 0, (size_t)s->so * 4);
    ps_rng_init(s->rngp, randomseed && *randomseed ? randomseed : NULL); s->rngDraws++;
    backups_clear(s);
    // JS uses the metadata value whenever it is present, so "again_interval 0" means every frame (the
    // compiler fills in the 0.15s defaults when the keys are absent, and 0 for realtime_interval)
    s->autotickinterval = g->metadata.realtime_interval * 1000;
    s->repeatinterval = g->metadata.key_repeat_interval * 1000;
    s->againinterval = g->metadata.again_interval * 1000;
    s->norepeat_action = g->metadata.norepeat_action;
    s->throttle_movement = g->metadata.throttle_movement;
    s->verbose_logging = g->metadata.verbose_logging;

    if (strcmp(cmd, "restart") == 0) {
        s->winning = false;
        s->timer = 0;
        s->titleScreen = true;
        tryPlaySimpleSound(s, PS_SFX_TITLESCREEN);
        s->textMode = true;
        s->titleSelection = showContinueOptionOnTitleScreen(s) ? 1 : 0;
        s->titleSelected = false;
        s->quittingMessageScreen = false;
        s->quittingTitleScreen = false;
        s->messageselected = false;
        s->titleMode = showContinueOptionOnTitleScreen(s) ? 1 : 0;
        generateTitleScreen(s);
    } else if (strcmp(cmd, "loadLevel") == 0) {
        s->curlevel = levelArg;
        set_curlevelTarget_null(s);
        s->winning = false;
        s->timer = 0;
        s->titleScreen = false;
        s->textMode = false;
        s->titleSelection = showContinueOptionOnTitleScreen(s) ? 1 : 0;
        s->titleSelected = false;
        s->quittingMessageScreen = false;
        s->quittingTitleScreen = false;
        s->messageselected = false;
        s->titleMode = 0;
        loadLevelFromState(s, levelArg, randomseed);
    }
    clearInputHistory(s);
    redraw(s);
}

// ---------------------------------------------------------------------------------------------
// terminal text (generateTitleScreen, drawMessageScreen, wordwrap, expandCJKCharacters)

static uint32_t utf8_decode(const unsigned char **pp)
{
    const unsigned char *p = *pp;
    uint32_t c = *p++;
    int n = 0;
    if (c < 0x80) n = 0;
    else if ((c & 0xE0) == 0xC0) { n = 1; c &= 0x1F; }
    else if ((c & 0xF0) == 0xE0) { n = 2; c &= 0x0F; }
    else if ((c & 0xF8) == 0xF0) { n = 3; c &= 0x07; }
    else { *pp = p; return 0xFFFD; }
    for (int k = 0; k < n; k++) {
        if ((*p & 0xC0) != 0x80) { *pp = p; return 0xFFFD; }
        c = (c << 6) | (*p++ & 0x3F);
    }
    *pp = p;
    return c;
}
static int utf8_encode(uint32_t c, char *o)
{
    if (c < 0x80) { o[0] = (char)c; return 1; }
    if (c < 0x800) { o[0] = (char)(0xC0 | (c >> 6)); o[1] = (char)(0x80 | (c & 0x3F)); return 2; }
    if (c < 0x10000) { o[0] = (char)(0xE0 | (c >> 12)); o[1] = (char)(0x80 | ((c >> 6) & 0x3F)); o[2] = (char)(0x80 | (c & 0x3F)); return 3; }
    o[0] = (char)(0xF0 | (c >> 18)); o[1] = (char)(0x80 | ((c >> 12) & 0x3F));
    o[2] = (char)(0x80 | ((c >> 6) & 0x3F)); o[3] = (char)(0x80 | (c & 0x3F)); return 4;
}

typedef struct { uint32_t *c; int n, cap; } U32Str;
static void u32_push(U32Str *s, uint32_t c) { VEC_PUSH(s->c, s->n, s->cap, c); }
static void u32_from_utf8(U32Str *s, const char *str)
{
    const unsigned char *p = (const unsigned char *)str;
    while (*p) u32_push(s, utf8_decode(&p));
}

static bool js_is_space(uint32_t c)   // \s
{
    return c == 9 || c == 10 || c == 11 || c == 12 || c == 13 || c == 32 || c == 0xA0 || c == 0x1680 ||
           (c >= 0x2000 && c <= 0x200A) || c == 0x2028 || c == 0x2029 || c == 0x202F || c == 0x205F ||
           c == 0x3000 || c == 0xFEFF;
}
static bool js_is_lineterm(uint32_t c) { return c == 10 || c == 13 || c == 0x2028 || c == 0x2029; }

// expandCJKCharacters
static void expandCJK(const U32Str *in, U32Str *out)
{
    for (int i = 0; i < in->n; i++) {
        u32_push(out, in->c[i]);
        if (!ps_font_glyph(in->c[i])) u32_push(out, ' ');
    }
}

// wordwrap: str.match(/.{1,W}(\s|$)|.{W}|.+$/g). Writes (start,len) pairs; returns count.
static int wordwrap(const U32Str *str, int width, int **outPairs)
{
    int n = str->n, count = 0, cap = 0;
    int *pairs = NULL;
    int p = 0;
    while (p < n) {
        int run = 0;   // non-line-terminator chars available from p
        while (p + run < n && !js_is_lineterm(str->c[p + run])) run++;
        int mlen = -1;
        int maxk = run < width ? run : width;
        for (int k = maxk; k >= 1; k--) {      // alt 1: .{1,W}(\s|$)
            if (p + k == n) { mlen = k; break; }
            if (js_is_space(str->c[p + k])) { mlen = k + 1; break; }
        }
        if (mlen < 0 && run >= width) mlen = width;          // alt 2: .{W}
        if (mlen < 0 && run >= 1 && p + run == n) mlen = run; // alt 3: .+$
        if (mlen < 0) { p++; continue; }
        if (count * 2 + 2 > cap) { cap = cap ? cap * 2 : 32; pairs = xrealloc(pairs, sizeof(int) * (size_t)cap); }
        pairs[count * 2] = p; pairs[count * 2 + 1] = mlen;
        count++;
        p += mlen;
    }
    *outPairs = pairs;
    return count;
}

static void tl_set_ascii(TLine *t, const char *a)
{
    t->n = 0;
    while (*a && t->n < TL_MAX) t->c[t->n++] = (unsigned char)*a++;
}
static void tl_set_u32(TLine *t, const uint32_t *c, int n)
{
    if (n > TL_MAX) n = TL_MAX;
    memcpy(t->c, c, sizeof(uint32_t) * (size_t)n);
    t->n = n;
}

static void ti_commit(PSSession *s)
{
    for (int r = 0; r < s->tiRows && r < TI_ROWS; r++) {
        char *o = s->tiUtf8[r];
        int k = 0;
        for (int i = 0; i < s->ti[r].n; i++) k += utf8_encode(s->ti[r].c[i], o + k);
        o[k] = 0;
    }
    for (int r = s->tiRows; r < TI_ROWS; r++) s->tiUtf8[r][0] = 0;
    s->dirty = true;
}

static const char *const intro_template[13] = {
    "..................................",
    "..................................",
    "..................................",
    "......Puzzle Script Terminal......",
    "..............v 1.8...............",
    "..................................",
    "..................................",
    "..................................",
    ".........insert cartridge.........",
    "..................................",
    "..................................",
    "..................................",
    ".................................."
};
static const char *const blank_row = "..................................";
static const char *const menu_startgame[3] = {
    "............start game............", "..........#.start game.#..........", "-----------.start game.-----------"
};
static const char *const menu_newgame[3] = {
    ".............new game.............", "...........#.new game.#...........", "------------.new game.------------"
};
static const char *const menu_continue[3] = {
    ".............continue.............", "...........#.continue.#...........", "------------.continue.------------"
};
static const char *const ctl_arrows = ".d-pad to move....................";
static const char *const ctl_action = ".A to action......................";
static const char *const ctl_undorestart = ".B to undo, hold B to restart.....";
static const char *const ctl_undo = ".B to undo........................";
static const char *const ctl_restart = ".hold B to restart................";
static const char *const loading_line = "------------ loading  ------------";

static int get_title_animation_frame(PSSession *s) { return (int)floor(((s->timer / 1000) / 0.3) * 10); }

// wordwrap + align into lines; align: 0 centre, 1 right
static int make_aligned_lines(const char *text, TLine *out, int maxOut, int align)
{
    U32Str a = {0, 0, 0}, b = {0, 0, 0};
    u32_from_utf8(&a, text);
    expandCJK(&a, &b);
    int *pairs = NULL;
    int n = wordwrap(&b, PS_TERMINAL_WIDTH, &pairs);
    int outN = 0;
    for (int i = 0; i < n && outN < maxOut; i++) {
        const uint32_t *c = b.c + pairs[2 * i];
        int len = pairs[2 * i + 1];
        TLine *t = &out[outN++];
        int W = PS_TERMINAL_WIDTH;
        if (len >= W) { tl_set_u32(t, c, len); continue; }
        if (align == 0) {   // align_centre
            int free_space = W - len, left = free_space / 2, right = free_space - left;
            t->n = 0;
            for (int k = 0; k < left; k++) t->c[t->n++] = '.';
            for (int k = 0; k < len; k++) t->c[t->n++] = c[k];
            for (int k = 0; k < right; k++) t->c[t->n++] = '.';
        } else {            // align_right
            uint32_t tmp[TL_MAX]; int tn = 0;
            for (int k = 0; k < len; k++) tmp[tn++] = c[k];
            if (tn < W - 1) tmp[tn++] = '.';
            t->n = 0;
            for (int k = tn; k < W; k++) t->c[t->n++] = '.';
            for (int k = 0; k < tn; k++) t->c[t->n++] = tmp[k];
        }
    }
    ps_free(pairs); ps_free(a.c); ps_free(b.c);
    return outN;
}

// logWarning(str, state.metadata_lines[key], true) from engine.js: printed each time, recorded
// once (errorStrings dedupes outside compilation; with a line number it is not counted as an error)
static void logWarning(PSSession *s, const char *msg, const char *metaKey)
{
    if (ps_log_hook) ps_log_hook(msg);
    for (int i = 0; i < s->nWarnings; i++) if (strcmp(s->warnings[i], msg) == 0) return;
    if (s->nWarnings >= (int)(sizeof s->warnings / sizeof s->warnings[0])) return;
    s->warnings[s->nWarnings] = msg;
    s->warningKeys[s->nWarnings] = metaKey;
    s->nWarnings++;
}

// generateTitleScreen
static void generateTitleScreen(PSSession *s)
{
    const PSGame *g = s->g;
    s->titleMode = showContinueOptionOnTitleScreen(s) ? 1 : 0;
    if (g->levelCount == 0) {
        s->tiRows = 13;
        for (int i = 0; i < 13; i++) tl_set_ascii(&s->ti[i], intro_template[i]);
        ti_commit(s);
        return;
    }
    TLine title_lines[8], author_lines[8];
    int t_len = make_aligned_lines(g->metadata.title ? g->metadata.title : "PuzzleScript Game", title_lines, 8, 0);
    int a_len = 0;
    if (g->metadata.author) {
        size_t al = strlen(g->metadata.author);
        char *by = xalloc(al + 4);
        memcpy(by, "by ", 3); memcpy(by + 3, g->metadata.author, al + 1);
        a_len = make_aligned_lines(by, author_lines, 8, 1);
        ps_free(by);
    }
    const char *menu[3];
    if (s->titleMode == 0) {
        menu[0] = blank_row;
        menu[1] = s->titleSelected ? menu_startgame[2] : menu_startgame[1];
        menu[2] = blank_row;
    } else if (s->titleSelection == 0) {
        menu[0] = menu_newgame[s->titleSelected ? 2 : 1];
        menu[1] = blank_row;
        menu[2] = menu_continue[0];
    } else {
        menu[0] = menu_newgame[0];
        menu[1] = blank_row;
        menu[2] = menu_continue[s->titleSelected ? 2 : 1];
    }
    const char *controls[4]; int nc = 0;
    controls[nc++] = ctl_arrows;
    bool has_action = !g->metadata.noaction, has_undo = !g->metadata.noundo, has_restart = !g->metadata.norestart;
    int extra = 0;
    if (has_action) controls[nc++] = ctl_action; else extra++;
    if (has_undo && has_restart) controls[nc++] = ctl_undorestart;
    else if (has_restart) controls[nc++] = ctl_restart;
    else if (has_undo) controls[nc++] = ctl_undo;
    else extra++;
    if (extra > 1) { extra--; controls[nc++] = blank_row; }
    int header_size = 5 + extra;
    int bs = 0, bm = 0, be = 0;
    if (bs + t_len + bm + a_len + be < header_size) be++;
    if (bs + t_len + bm + a_len + be < header_size) bm++;
    if (bs + t_len + bm + a_len + be < header_size) bs++;
    if (bs + t_len + bm + a_len + be < header_size) bs++;
    bool title_trimmed = false, author_trimmed = false;
    while (bs + t_len + bm + a_len + be > header_size) {
        if (a_len > 1) { a_len--; author_trimmed = true; }
        else if (t_len > 1) { t_len--; title_trimmed = true; }
        else break;
    }
    if (title_trimmed) logWarning(s, "Game title is too long to fit on screen; truncating to fit.", "title");
    if (author_trimmed) logWarning(s, "Author name is too long to fit on screen; truncating to fit.", "author");
    int r = 0;
    for (int i = 0; i < bs; i++) tl_set_ascii(&s->ti[r++], blank_row);
    for (int i = 0; i < t_len; i++) s->ti[r++] = title_lines[i];
    for (int i = 0; i < bm; i++) tl_set_ascii(&s->ti[r++], blank_row);
    for (int i = 0; i < a_len; i++) s->ti[r++] = author_lines[i];
    for (int i = 0; i < be; i++) tl_set_ascii(&s->ti[r++], blank_row);
    int selection_row = 0;
    for (int i = 0; i < 3; i++) {
        if (strstr(menu[i], "---")) selection_row = r;
        tl_set_ascii(&s->ti[r++], menu[i]);
    }
    tl_set_ascii(&s->ti[r++], blank_row);
    for (int i = 0; i < nc; i++) tl_set_ascii(&s->ti[r++], controls[i]);
    tl_set_ascii(&s->ti[r++], blank_row);
    while (r < PS_TERMINAL_HEIGHT) tl_set_ascii(&s->ti[r++], blank_row);
    if (r < TI_ROWS) tl_set_ascii(&s->ti[r++], blank_row);
    s->tiRows = r;

    if (s->titleSelected) {
        int frame = get_title_animation_frame(s) + 2;
        bool loading_text = frame > 12;
        TLine *row = &s->ti[selection_row];
        if (loading_text) tl_set_ascii(row, loading_line);
        frame = frame % 23;
        if (frame > 11) frame = 11 - (frame % 12);
        int L = 11 - frame, R = 22 + frame;
        TLine old = *row, nl;
        nl.n = 0;
#define SLICE(a, b) for (int k_ = (a); k_ < (b) && k_ < old.n; k_++) nl.c[nl.n++] = old.c[k_]
        if (!loading_text) {
            for (int k = 0; k < L; k++) nl.c[nl.n++] = ' ';
            nl.c[nl.n++] = '#';
            SLICE(L + 1, R);
            nl.c[nl.n++] = '#';
            for (int k = 0; k < L; k++) nl.c[nl.n++] = ' ';
        } else {
            SLICE(0, L);
            nl.c[nl.n++] = '#';
            SLICE(L + 1, R);
            nl.c[nl.n++] = '#';
            SLICE(R + 1, old.n);
        }
#undef SLICE
        *row = nl;
    }
    for (int i = 0; i < s->tiRows; i++)
        for (int k = 0; k < s->ti[i].n; k++) if (s->ti[i].c[k] == '.') s->ti[i].c[k] = ' ';
    ti_commit(s);
}

static void js_trim(U32Str *u)
{
    int a = 0, b = u->n;
    while (a < b && js_is_space(u->c[a])) a++;
    while (b > a && js_is_space(u->c[b - 1])) b--;
    if (a > 0 && b > a) memmove(u->c, u->c + a, sizeof(uint32_t) * (size_t)(b - a));   // (u->c is NULL when empty)
    u->n = b - a;
}

// drawMessageScreen
static void drawMessageScreen(PSSession *s)
{
    s->titleMode = 0;
    s->textMode = true;
    s->tiRows = 13;
    for (int i = 0; i < 13; i++) tl_set_ascii(&s->ti[i], i == 10 ? "          A to continue           " : "                                  ");
    TLine emptyLine = s->ti[9], xToContinue = s->ti[10];
    s->ti[10] = emptyLine;
    int width = s->ti[0].n;
    U32Str msg = {0, 0, 0}, ex = {0, 0, 0};
    if (s->messagetext[0] == 0) {
        const PSLevelDef *ld = (s->curlevel >= 0 && s->curlevel < s->g->levelCount) ? &s->g->levels[s->curlevel] : NULL;
        if (ld && ld->message) u32_from_utf8(&msg, ld->message);
        js_trim(&msg);
    } else {
        u32_from_utf8(&msg, s->messagetext);
    }
    expandCJK(&msg, &ex);
    int *pairs = NULL;
    int n = wordwrap(&ex, width, &pairs);
    s->splitMessageLen = n;
    int offset = 5 - (n / 2);
    if (offset < 0) offset = 0;
    int count = n < 12 ? n : 12;
    for (int i = 0; i < count; i++) {
        const uint32_t *m = ex.c + pairs[2 * i];
        int ml = pairs[2 * i + 1];
        int row = offset + i;
        if (row >= 13) break;
        int lmargin = (width - ml) / 2;
        TLine old = s->ti[row], nl;
        nl.n = 0;
        for (int k = 0; k < lmargin && k < old.n; k++) nl.c[nl.n++] = old.c[k];
        for (int k = 0; k < ml && nl.n < TL_MAX; k++) nl.c[nl.n++] = m[k];
        for (int k = lmargin + ml; k < old.n && nl.n < TL_MAX; k++) nl.c[nl.n++] = old.c[k];
        s->ti[row] = nl;
    }
    int endPos = 10;
    if (count >= 10) endPos = count < 12 ? count + 1 : 12;
    s->ti[endPos] = s->quittingMessageScreen ? emptyLine : xToContinue;
    ps_free(pairs); ps_free(msg.c); ps_free(ex.c);
    ti_commit(s);
}

// ---------------------------------------------------------------------------------------------
// camera (the oldflickscreendat part of graphics.js redraw)

static void redraw(PSSession *s)
{
    s->dirty = true;
    if (s->textMode || !s->levelLoaded || s->unitTesting) return;
    const PSGame *g = s->g;
    PSLevel *l = &s->level;
    int sw = l->width, sh = l->height;
    bool flick = g->metadata.has_flickscreen, zoom = !flick && g->metadata.has_zoomscreen;
    if (flick) { sw = g->metadata.flickscreen[0]; sh = g->metadata.flickscreen[1]; }
    else if (zoom) { sw = g->metadata.zoomscreen[0]; sh = g->metadata.zoomscreen[1]; }
    int mini = 0, maxi = sw, minj = 0, maxj = sh;
    if (flick || zoom) {
        int n = getPlayerPositions(s, s->playerPositions);
        if (n > 0) {
            int pp = s->playerPositions[0];
            int px = pp / l->height, py = pp % l->height;
            if (flick) {
                mini = (px / sw) * sw;
                minj = (py / sh) * sh;
            } else {
                int a = px - sw / 2, b = l->width - sw;
                mini = a < b ? a : b; if (mini < 0) mini = 0;
                a = py - sh / 2; b = l->height - sh;
                minj = a < b ? a : b; if (minj < 0) minj = 0;
            }
            maxi = mini + sw < l->width ? mini + sw : l->width;
            maxj = minj + sh < l->height ? minj + sh : l->height;
            s->flickLen = 4;
            s->flick[0] = mini; s->flick[1] = minj; s->flick[2] = maxi; s->flick[3] = maxj;
        } else if (s->flickLen == 4) {
            mini = s->flick[0]; minj = s->flick[1]; maxi = s->flick[2]; maxj = s->flick[3];
        }
    }
    s->view[0] = mini; s->view[1] = minj; s->view[2] = maxi; s->view[3] = maxj;
}

// ---------------------------------------------------------------------------------------------
// input (inputoutput.js checkKey / pressKey / onKeyUp / update)

static void checkKey(PSSession *s, int key, bool justPressed)
{
    const PSGame *g = s->g;
    if (s->winning) return;
    int inputdir = -1;
    switch (key) {
    case PS_KEY_LEFT: inputdir = 1; break;
    case PS_KEY_UP: inputdir = 0; break;
    case PS_KEY_RIGHT: inputdir = 3; break;
    case PS_KEY_DOWN: inputdir = 2; break;
    case PS_KEY_ACTION:
        if (justPressed && s->ignoreNotJustPressedAction) s->ignoreNotJustPressedAction = false;
        if (!justPressed && s->ignoreNotJustPressedAction) return;
        if (!s->norepeat_action || justPressed) inputdir = 4;
        else return;
        break;
    case PS_KEY_UNDO:
        if (!s->textMode) {
            DoUndo(s, false, true);
            redraw(s);
            return;
        }
        break;
    case PS_KEY_RESTART:
        if (!s->textMode && justPressed) {
            DoRestart(s, false);
            redraw(s);
            return;
        }
        break;
    case PS_KEY_QUIT:
        if (!s->titleScreen) {
            goToTitleScreen(s);
            tryPlaySimpleSound(s, PS_SFX_TITLESCREEN);
            redraw(s);
            return;
        }
        break;
    default: break;
    }
    if (s->throttle_movement && inputdir >= 0 && inputdir <= 3) {
        if (s->lastinput == inputdir && s->input_throttle_timer < s->repeatinterval) return;
        s->lastinput = inputdir;
        s->input_throttle_timer = 0;
    }
    if (s->textMode) {
        if (g->levelCount == 0) {
        } else if (s->titleScreen) {
            if (!s->quittingTitleScreen) {
                if (s->titleMode == 0) {
                    if (inputdir == 4 && justPressed && !s->titleSelected) {
                        tryPlaySimpleSound(s, PS_SFX_STARTGAME);
                        s->titleSelected = true;
                        s->messageselected = false;
                        s->timer = 0;
                        s->quittingTitleScreen = true;
                        generateTitleScreen(s);
                        redraw(s);
                        clearInputHistory(s);
                    }
                } else {
                    if (inputdir == 4 && justPressed) {
                        if (!s->titleSelected) {
                            tryPlaySimpleSound(s, PS_SFX_STARTGAME);
                            s->titleSelected = true;
                            s->messageselected = false;
                            s->timer = 0;
                            s->quittingTitleScreen = true;
                            generateTitleScreen(s);
                            redraw(s);
                        }
                    } else if (inputdir == 0 || inputdir == 2) {
                        s->titleSelection = inputdir == 0 ? 0 : 1;
                        generateTitleScreen(s);
                        redraw(s);
                    }
                }
            }
        } else {
            if (inputdir == 4 && justPressed) {
                if (s->unitTesting) {
                    nextLevel(s);
                    return;
                } else if (!s->messageselected) {
                    s->messageselected = true;
                    s->timer = 0;
                    s->quittingMessageScreen = true;
                    tryPlaySimpleSound(s, PS_SFX_CLOSEMESSAGE);
                    s->titleScreen = false;
                    drawMessageScreen(s);
                }
            }
        }
    } else {
        if (!s->againing && inputdir >= 0) {
            if (inputdir == 4 && g->metadata.noaction) {
            } else {
                if (processInput(s, inputdir, false, false)) redraw(s);
            }
        }
    }
}

// pressKey + onKeyDown
void ps_key_down(PSSession *s, int key)
{
    if (key < 0 || key >= PS_KEY_COUNT) return;
    for (int i = 0; i < s->keybufferLen; i++) if (s->keybuffer[i] == key) return;
    int idx = s->keyRepeatIndex + 1;
    if (idx > s->keybufferLen) idx = s->keybufferLen;
    if (idx < 0) idx = 0;
    s->keyRepeatIndex = idx;
    memmove(s->keybuffer + idx + 1, s->keybuffer + idx, sizeof(int) * (size_t)(s->keybufferLen - idx));
    s->keybuffer[idx] = key;
    s->keybufferLen++;
    s->keyRepeatTimer = 0;
    checkKey(s, key, true);
}

// onKeyUp
void ps_key_up(PSSession *s, int key)
{
    int index = -1;
    for (int i = 0; i < s->keybufferLen; i++) if (s->keybuffer[i] == key) { index = i; break; }
    if (index < 0) return;
    memmove(s->keybuffer + index, s->keybuffer + index + 1, sizeof(int) * (size_t)(s->keybufferLen - index - 1));
    s->keybufferLen--;
    if (s->keyRepeatIndex >= index) s->keyRepeatIndex--;
}

// update
void ps_update(PSSession *s, double dt)
{
    s->deltatime = dt * 1000.0;
    int lastframe = get_title_animation_frame(s);
    s->timer += s->deltatime;
    s->input_throttle_timer += s->deltatime;
    if (s->quittingTitleScreen) {
        int thisFrame = get_title_animation_frame(s);
        if (s->timer / 1000 > 0.3) {
            s->quittingTitleScreen = false;
            nextLevel(s);
        } else if (thisFrame > lastframe) {
            generateTitleScreen(s);
            redraw(s);
        }
    }
    if (s->againing) {
        if (s->timer > s->againinterval && s->messagetext[0] == 0) {
            if (processInput(s, -1, false, false)) {
                redraw(s);
                s->keyRepeatTimer = 0;
                s->autotick = 0;
            }
        }
    }
    if (s->quittingMessageScreen) {
        if (s->timer / 1000 > 0.15) {
            s->quittingMessageScreen = false;
            if (s->messagetext[0] == 0) {
                nextLevel(s);
            } else {
                set_messagetext(s, "");
                s->textMode = false;
                s->titleScreen = false;
                s->titleMode = (s->curlevel > 0 || s->curlevelTarget) ? 1 : 0;
                s->titleSelected = false;
                s->ignoreNotJustPressedAction = true;
                s->titleSelection = 0;
                redraw(s);
                checkWin(s, false);
            }
        }
    }
    if (s->winning) {
        if (s->timer / 1000 > 0.5) {
            s->winning = false;
            nextLevel(s);
        }
    }
    if (s->keybufferLen > 0) {
        s->keyRepeatTimer += s->deltatime;
        double ticklength = s->throttle_movement ? s->repeatinterval : s->repeatinterval / sqrt((double)s->keybufferLen);
        if (s->keyRepeatTimer > ticklength) {
            s->keyRepeatTimer = 0;
            s->keyRepeatIndex = (s->keyRepeatIndex + 1) % s->keybufferLen;
            checkKey(s, s->keybuffer[s->keyRepeatIndex], false);
        }
    }
    if (s->autotickinterval > 0 && !s->textMode && !s->levelEditorOpened && !s->againing && !s->winning) {
        s->autotick += s->deltatime;
        if (s->autotick > s->autotickinterval) {
            s->autotick = 0;
            if (processInput(s, -1, false, false)) redraw(s);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// public API

PSSession *ps_session_new(const PSGame *g, const PSStorage *storage, const char *seed)
{
    PSSession *s = xalloc(sizeof *s);
    s->g = g;
    s->rngp = &s->rng;
    if (storage) { s->storage = *storage; s->hasStorage = true; }
    s->so = g->strideObj > 0 ? g->strideObj : 1;
    s->sm = g->strideMov > 0 ? g->strideMov : 1;
    s->layerCount = g->layerCount;
    s->rigidArrays = g->rigid;
    s->groups = build_groups(s, g->rules, g->ruleGroupCount);
    s->lateGroups = build_groups(s, g->lateRules, g->lateRuleGroupCount);
    int maxGroup = 1;
    for (int i = 0; i < g->ruleGroupCount; i++) if (g->rules[i].count > maxGroup) maxGroup = g->rules[i].count;
    for (int i = 0; i < g->lateRuleGroupCount; i++) if (g->lateRules[i].count > maxGroup) maxGroup = g->lateRules[i].count;
    s->tupleCounts = xalloc(sizeof(int) * (size_t)maxGroup);
    s->bannedGroup = xalloc(sizeof(bool) * (size_t)(g->ruleGroupCount + 1));
    if (s->maxRows < 1) s->maxRows = 1;
    s->rowMatches = xalloc(sizeof(IntVec) * (size_t)s->maxRows);

    size_t ob = (size_t)s->so * 4, mb = (size_t)s->sm * 4;
    uint32_t **os[] = { &s->o1, &s->o2, &s->o2_5, &s->o3, &s->o4, &s->o5, &s->o6, &s->o7, &s->o8, &s->o10, &s->o11,
                        &s->sfxCreateMask, &s->sfxDestroyMask };
    for (size_t i = 0; i < sizeof os / sizeof os[0]; i++) *os[i] = xalloc(ob);
    s->m1 = xalloc(mb); s->m2 = xalloc(mb); s->m3 = xalloc(mb);
    for (int r = 0; r < TI_ROWS; r++) { s->tiUtf8[r] = xalloc(TL_MAX * 4 + 1); s->tiPtrs[r] = s->tiUtf8[r]; }

    // globalVariables.js initial values
    s->messagetext = ps_strdup("");
    s->ignoreNotJustPressedAction = true;
    s->textMode = true;
    s->titleScreen = true;
    s->deltatime = 17;
    s->repeatinterval = 150;
    s->againinterval = 150;
    s->lastinput = -100;
    s->level.layerCount = g->layerCount;
    s->level.width = s->level.height = 0;
    doSetupTitleScreenLevelContinue(s);
    setGameState(s, "restart", 0, seed);
    return s;
}

void ps_session_free(PSSession *s)
{
    if (!s) return;
    backups_clear(s);
    ps_free(s->backups);
    backup_free(s->restartTarget);
    backup_free(s->curlevelTarget);
    level_free_arrays(&s->level);
    ps_free(s->level.objects);
    for (int i = 0; i < s->maxRows; i++) ps_free(s->rowMatches[i].d);
    ps_free(s->rowMatches);
    ps_free(s->tupleCounts);
    ps_free(s->bannedGroup);
    ps_free(s->startObjects); ps_free(s->startMovements);
    for (int i = 0; i < TURN_DEPTH_MAX; i++) ps_free(s->turnBak[i].dat);
    uint32_t *bufs[] = { s->o1, s->o2, s->o2_5, s->o3, s->o4, s->o5, s->o6, s->o7, s->o8, s->o10, s->o11,
                         s->sfxCreateMask, s->sfxDestroyMask, s->m1, s->m2, s->m3 };
    for (size_t i = 0; i < sizeof bufs / sizeof bufs[0]; i++) ps_free(bufs[i]);
    for (int r = 0; r < TI_ROWS; r++) ps_free(s->tiUtf8[r]);
    ps_free(s->canMove.d); ps_free(s->cantMove.d);
    ps_free(s->ac.start); ps_free(s->ac.diffIdx); ps_free(s->ac.diffVal); ps_free(s->ac.sfxC); ps_free(s->ac.sfxD);
    ps_free(s->ac.can.d); ps_free(s->ac.cant.d);
    ps_free(s->soundHistory);
    ps_free(s->playerPositions);
    ps_free(s->messagetext);
    arena_free(s->arena);
    ps_free(s);
}

void ps_set_unit_testing(PSSession *s, bool on) { s->unitTesting = on; }

void ps_load_level(PSSession *s, int levelIndex, const char *seed)
{
    setGameState(s, "loadLevel", levelIndex, seed);
}

bool ps_process_input(PSSession *s, int dir) { return processInput(s, dir, false, false); }
bool ps_againing(PSSession *s) { return s->againing; }
void ps_clear_againing(PSSession *s) { s->againing = false; }
void ps_undo(PSSession *s) { DoUndo(s, false, true); }
void ps_restart(PSSession *s) { DoRestart(s, false); }

// convertLevelToString (debug.js)
typedef struct { char *key; int idx; } SeenCell;
static int cmp_str(const void *a, const void *b) { return strcmp(*(const char *const *)a, *(const char *const *)b); }
static void sort_names(const char **v, int n)   // insertion sort (n is small)
{
    for (int i = 1; i < n; i++) {
        const char *x = v[i];
        int j = i - 1;
        while (j >= 0 && cmp_str(&v[j], &x) > 0) { v[j + 1] = v[j]; j--; }
        v[j + 1] = x;
    }
}
char *ps_level_to_string(PSSession *s)
{
    const PSLevel *l = &s->level;
    const PSGame *g = s->g;
    size_t cap = 256, len = 0;
    char *out = xalloc(cap);
#define APPEND(str, n) do { size_t n_ = (n); if (len + n_ + 1 > cap) { while (len + n_ + 1 > cap) cap *= 2; out = xrealloc(out, cap); } \
    memcpy(out + len, (str), n_); len += n_; out[len] = 0; } while (0)
    SeenCell *seen = NULL; int nseen = 0, capseen = 0;
    const char **names = xalloc(sizeof(char *) * (size_t)(g->objectCount + 1));
    char *key = NULL; size_t keycap = 0;
    int next = 0;
    for (int y = 0; y < l->height; y++) {
        for (int x = 0; x < l->width; x++) {
            const uint32_t *cell = l->objects + (size_t)(x + y * l->width) * s->so;
            int nn = 0;
            for (int bit = 0; bit < 32 * s->so; bit++)
                if (get_bit(cell, bit)) names[nn++] = bit < g->objectCount ? g->objects[bit].name : "";
            sort_names(names, nn);
            size_t kl = 0;
            for (int i = 0; i < nn; i++) kl += strlen(names[i]) + 1;
            if (kl + 1 > keycap) { keycap = kl + 64; key = xrealloc(key, keycap); }
            size_t p = 0;
            for (int i = 0; i < nn; i++) {
                if (i) key[p++] = ' ';
                size_t sl = strlen(names[i]);
                memcpy(key + p, names[i], sl); p += sl;
            }
            key[p] = 0;
            int idx = -1;
            for (int i = 0; i < nseen; i++) if (strcmp(seen[i].key, key) == 0) { idx = seen[i].idx; break; }
            if (idx < 0) {
                SeenCell sc = { ps_strdup(key), next++ };
                VEC_PUSH(seen, nseen, capseen, sc);
                idx = sc.idx;
                APPEND(key, p);
                APPEND(":", 1);
            }
            char nb[16];
            int k = fmt_int(nb, idx);
            nb[k++] = ',';
            APPEND(nb, (size_t)k);
        }
        APPEND("\n", 1);
    }
#undef APPEND
    for (int i = 0; i < nseen; i++) ps_free(seen[i].key);
    ps_free(seen); ps_free(names); ps_free(key);
    return out;
}

int ps_sound_history(PSSession *s, const int64_t **seeds)
{
    if (seeds) *seeds = s->soundHistory;
    return s->nSoundHistory;
}

bool ps_in_text_mode(const PSSession *s) { return s->textMode; }
bool ps_on_title_screen(const PSSession *s) { return s->titleScreen; }
const char *const *ps_text_lines(const PSSession *s) { return s->tiPtrs; }
const PSLevel *ps_current_level(const PSSession *s) { return s->levelLoaded ? &s->level : NULL; }
int ps_current_level_index(const PSSession *s) { return s->curlevel; }

void ps_view_rect(const PSSession *s, int *x0, int *y0, int *x1, int *y1)
{
    const PSLevel *l = &s->level;
    int a = 0, b = 0, c = l->width, d = l->height;
    if (s->g->metadata.has_flickscreen || s->g->metadata.has_zoomscreen) {
        a = s->view[0]; b = s->view[1]; c = s->view[2]; d = s->view[3];
        if (c <= a || d <= b) { a = 0; b = 0; c = l->width; d = l->height; }
        if (c > l->width) c = l->width;
        if (d > l->height) d = l->height;
    }
    *x0 = a; *y0 = b; *x1 = c; *y1 = d;
}

bool ps_take_dirty(PSSession *s) { bool d = s->dirty; s->dirty = false; return d; }

int ps_take_sounds(PSSession *s, int64_t *out, int max)
{
    int n = s->nSoundQueue < max ? s->nSoundQueue : max;
    memcpy(out, s->soundQueue, sizeof(int64_t) * (size_t)n);
    memmove(s->soundQueue, s->soundQueue + n, sizeof(int64_t) * (size_t)(s->nSoundQueue - n));
    s->nSoundQueue -= n;
    return n;
}

void ps_goto_level(PSSession *s, int levelIndex)
{
    setGameState(s, "loadLevel", levelIndex, NULL);
}

void ps_goto_title(PSSession *s)
{
    goToTitleScreen(s);
    tryPlaySimpleSound(s, PS_SFX_TITLESCREEN);
    redraw(s);
}

bool ps_is_winning(const PSSession *s) { return s->winning; }

int ps_session_warnings(const PSSession *s, const char *const **messages, const char *const **metaKeys)
{
    if (messages) *messages = s->warnings;
    if (metaKeys) *metaKeys = s->warningKeys;
    return s->nWarnings;
}
bool ps_is_againing(const PSSession *s) { return s->againing; }

// Diagnostics for tests/stress.c (not in ps.h; declare it where needed): bytes held by the undo
// history and the number of entries.
size_t ps_debug_undo_bytes(const PSSession *s, int *count);
size_t ps_debug_undo_bytes(const PSSession *s, int *count)
{
    if (count) *count = s->nbackups;
    return s->backupBytes;
}
