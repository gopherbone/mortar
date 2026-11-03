// SPDX-License-Identifier: MIT
// Internal shared state for ps_parse.c (parser.js) and ps_compile.c (compiler.js).
// Not part of the public API.
#ifndef PS_COMPILE_INTERNAL_H
#define PS_COMPILE_INTERNAL_H

#include "ps.h"
#include <setjmp.h>
#include <stdarg.h>

// ---------------------------------------------------------------------------------------------
// arena (bump allocator; everything is freed at once)
typedef struct ArenaBlock { struct ArenaBlock *next; size_t used, cap; } ArenaBlock;
typedef struct { ArenaBlock *head; size_t total; } Arena;
void *ar_alloc(Arena *a, size_t n);              // zeroed, 8-aligned
void *ar_alloc_raw(Arena *a, size_t n);          // not zeroed (for buffers filled right away)
void  ar_free_all(Arena *a);
void  ar_reset(Arena *a);                        // free everything but keep one block for reuse
char *ar_strdup(Arena *a, const char *s);
char *ar_strndup(Arena *a, const char *s, size_t n);

// growable vectors living in an arena
typedef struct { int n, cap; int *v; } IVec;
typedef struct { int n, cap; void **v; } PVec;
void iv_push(Arena *a, IVec *v, int x);
void pv_push(Arena *a, PVec *v, void *x);
bool iv_has(const IVec *v, int x);
int  iv_index(const IVec *v, int x, int from);
void iv_splice(IVec *v, int at, int count);      // remove count items at index
void pv_splice(PVec *v, int at, int count);
IVec iv_clone(Arena *a, const IVec *v);

// string builder (temp arena)
typedef struct { Arena *a; char *s; int n, cap; } SB;
void sb_init(SB *b, Arena *a);
void sb_putc(SB *b, char c);
void sb_puts(SB *b, const char *s);
void sb_putn(SB *b, const char *s, int n);
void sb_printf(SB *b, const char *fmt, ...);     // %s %d %c %S(upper-cased string) %%
void sb_vprintf(SB *b, const char *fmt, va_list ap);
char *sb_done(SB *b);                            // NUL-terminated, owned by arena
char *tfmt(Arena *a, const char *fmt, ...);

// ---------------------------------------------------------------------------------------------
// UTF-8 / JS character classes
int  u8_decode(const char *s, int len, int pos, int *cp);   // returns byte length (>=1)
int  u8_encode(uint32_t cp, char *out);
bool js_is_space(int cp);          // [\p{Z}\s]
bool js_is_Z(int cp);              // \p{Z}
bool js_is_namechar(int cp);       // [\p{L}\p{N}_]
bool js_is_wordchar(int cp);       // \w (ASCII)
int  js_lower_cp(int cp);          // keeps UTF-8 byte length
int  js_upper_cp(int cp);
char *js_lower(Arena *a, const char *s);
char *js_upper(Arena *a, const char *s);
char *js_trim(Arena *a, const char *s, int n);   // trim JS whitespace from s[0..n)
int  js_utf16_len(const char *s);
int  js_utf16_len_n(const char *s, int n);
double js_number(const char *s);   // Number(s)
double js_parsefloat(const char *s);
double js_parseint(const char *s); // parseInt(s) (base 10), NaN when invalid
bool js_is_array_index(const char *s);

// ---------------------------------------------------------------------------------------------
// atoms (interned strings; equal strings <=> equal ids)
#define ATOM_RECENT 64
typedef struct { unsigned hash; int id; int len; char s[20]; } AtomRecent;   // id+1, 0 = empty
typedef struct { Arena *a; int n, cap; char **str; int *hash_next; unsigned *hashes; int *lens; int *buckets; int nbuckets;
                 AtomRecent recent[ATOM_RECENT]; } Atoms;
int atom(Atoms *t, const char *s);
int atom_n(Atoms *t, const char *s, int n);
int atom_find(Atoms *t, const char *s);          // -1 if absent
#define ATOM_STR(c, id) ((c)->atoms.str[id])

struct PSComp;
// dictionary keyed by atom, insertion ordered (like a JS object with string keys)
typedef struct { Arena *a; int cap; void **val; bool *has; IVec order; } ADict;
void  ad_init(ADict *d, Arena *a);
bool  ad_has(const ADict *d, int key);
void *ad_get(const ADict *d, int key);
void  ad_set(ADict *d, int key, void *val);      // appends key to order if new
void  ad_del(ADict *d, int key);
// Object.keys order: array-index-like keys ascending first, then insertion order. Returns temp array.
int  *ad_keys(struct PSComp *c, const ADict *d, int *count);

// ---------------------------------------------------------------------------------------------
// parser.js state

typedef struct {
    int name;               // atom
    int lineNumber;
    PVec colors;            // char*
    PVec *spritematrix;     // rows (char*)
    int layer, id;          // -1 when unassigned
    // compiled
    uint32_t *rgb; int ncolors;
    int8_t *sprite; int spriteN;
} PObj;

// legend entry: synonyms have one value
typedef struct { int name; IVec vals; int lineNumber; } PLegend;

enum { SND_SOUNDEVENT, SND_NAME, SND_SOUNDVERB, SND_DIRECTION, SND_SOUND, SND_ERROR };
typedef struct { char *text; int type; } PSndTok;
typedef struct { int n; PSndTok *t; int lineNumber; } PSound;

typedef struct { char *rule; int lineNumber; char *mixed; } PRuleLine;
typedef struct { int n; char **w; int lineNumber; } PWin;

enum { LV_EMPTY, LV_LEVEL, LV_MESSAGE };
typedef struct { int type; int lineNumber; char *message; PVec rows; } PLevel;

typedef struct { char *key, *val; } PMeta;

typedef struct PSComp {
    Arena tmp;              // compile-time scratch
    Arena perm;             // diagnostics and atoms (outlive temporary work arenas)
    Arena *out;             // PSGame storage
    PSGame *g;
    jmp_buf abort_jb;
    Atoms atoms;

    // diagnostics
    PVec errs;              // char* plain text
    PVec errKeys;           // dedupe keys
    int errorCount;
    bool compiling;

    // ---- parser state (parser.js startState)
    int lineNumber;
    int commentLevel;
    int section;            // -1 = '' (preamble), else index in sectionNames
    IVec visitedSections;
    bool line_should_end; const char *line_should_end_because;
    bool sol_after_comment;
    bool inside_cell; int lhs_rhs_rowcount_difference; bool arrow_passed, rhs_pattern_started, rule_prelude;
    int objects_candname;   // atom
    int objects_section;
    PVec *objects_spritematrix;
    PVec collisionLayers;   // IVec*
    int tokenIndex;
    IVec wip;               // legend/collisionlayers words (atoms)
    int wipSndN, wipSndCap; PSndTok *wipSnd; // sounds wip
    const char *mixedCase;
    ADict objects;          // atom -> PObj*
    PVec objectOrder;       // PObj* in definition order
    PVec legend_synonyms, legend_aggregates, legend_properties;   // PLegend*
    PVec sounds;            // PSound*
    PVec rules;             // PRuleLine*
    ADict names;            // set
    PVec winconditions;     // PWin*
    PVec metadata;          // PMeta*
    ADict metadata_lines;   // atom(key) -> (void*)(intptr_t)line
    ADict original_line_numbers;
    ADict abbrevNames;      // set
    bool levelKeyOk[128];   // ASCII map characters already found in abbrevNames (LEVELS section)
    PVec levels;            // PLevel*

    // ---- compiler state
    int objectCount, layerCount, strideObj, strideMov;
    IVec idDict;            // id -> name atom
    const char *const *colorPalette;   // 24 hex strings in name order
    ADict glyphDict;        // atom -> int* (layerCount entries)
    ADict aggregatesDict, propertiesDict, synonymsDict;   // -> IVec* (shared), synonyms -> (intptr_t)atom+1
    ADict propertiesSingleLayer;   // -> (intptr_t)layer+1
    int backgroundid, backgroundlayer;   // -1 when undefined
    uint32_t **layerMasks;
    ADict objectMasks, aggregateMasks;   // -> uint32_t*
    bool playerAggregate; uint32_t *playerMask;
    bool invalid;
    PVec crules;            // Rule* (rulesToArray output)
    IVec loopsLine, loopsDir;
    struct MaskSlot { const void *p; unsigned hash; int len; } *maskTab; int maskTabN, maskTabCap;   // mask interning (open addressing)
#define PS_MASK_FRONT 256
    struct MaskSlot maskFront[PS_MASK_FRONT];             // recently interned blobs (direct mapped)
#define PS_ZERO_BLOB_WORDS 64
    void *zeroBlob[PS_ZERO_BLOB_WORDS + 1];               // interned all-zero blob per word count
    Arena work;             // per-rule scratch for the expansion passes, reset between rules
    ADict newmeta;          // twiddleMetaData: key atom -> char*
} PSComp;

// logging (parser.js logError & co). line < 0 means undefined.
void ps_log_error(PSComp *c, int line, const char *fmt, ...);
void ps_log_warning(PSComp *c, int line, const char *fmt, ...);
void ps_log_error_urgent(PSComp *c, int line, const char *fmt, ...);
void ps_log_warning_noline(PSComp *c, const char *fmt, ...);

// parser entry: tokenizes the whole file (loadFile's loop). returns false when aborted (too many errors).
bool ps_parse_source(PSComp *c, const char *src, size_t len);
bool ps_word_already_declared(PSComp *c, int name);
PObj *ps_obj(PSComp *c, int name);

// common atoms (filled by ps_parse.c)
extern const char *const ps_sectionNames[7];
extern const char *const ps_color_names[24];
int ps_color_name_index(const char *s);   // -1 when not a palette colour name

#endif
