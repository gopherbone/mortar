// SPDX-License-Identifier: MIT
// Port of compiler.js (loadFile and every pass it runs) + colors.js lookups.
// Produces an immutable PSGame (see ps.h). The tokenizer (parser.js) lives in ps_parse.c.
#include "ps_compile_internal.h"
#include "ps_palettes.h"
#include <string.h>
#include <math.h>
#ifndef TARGET_PLAYDATE
#include <stdio.h>
#include <stdlib.h>
#endif

// per-pass memory / time reporting (PS_DEBUG_MEM on host, PROFILE=1 bench on device)
#ifndef TARGET_PLAYDATE
#include <time.h>
static double memstage_t0;
static double memstage_now(void) { struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts); return ts.tv_sec * 1e3 + ts.tv_nsec / 1e6; }
#define MEMSTAGE(n) do { if (getenv("PS_DEBUG_MEM")) { double t_ = memstage_now(); \
        fprintf(stderr, "  after %-28s %8.3fms  scratch %zu game %zu\n", n, memstage_t0 ? t_ - memstage_t0 : 0.0, c->tmp.total, c->out->total); \
        memstage_t0 = memstage_now(); } } while (0)
#elif defined(PS_RULE_PROFILE)
void ps_prof_stage(const char *name);   // ps_bench.c: logs the time since the previous stage
#define MEMSTAGE(n) ps_prof_stage(n)
#else
#define MEMSTAGE(n) do { } while (0)
#endif

// =============================================================================================
// atoms used by the compiler

static struct {
    int empty, no, dots, up, down, left, right, action, moving, stationary, randomdir, random;
    int caret, vee, lt, gt, parallel, perpendicular, horizontal, vertical, orthogonal;
    int horizontal_par, horizontal_perp, vertical_par, vertical_perp;
    int late, rigid, plus, lbr, rbr, bar, arrow, startloop, endloop, background, player, all;
    int message;
} A;

static void init_atoms(PSComp *c)
{
    Atoms *t = &c->atoms;
    A.empty = atom(t, ""); A.no = atom(t, "no"); A.dots = atom(t, "..."); A.up = atom(t, "up");
    A.down = atom(t, "down"); A.left = atom(t, "left"); A.right = atom(t, "right"); A.action = atom(t, "action");
    A.moving = atom(t, "moving"); A.stationary = atom(t, "stationary"); A.randomdir = atom(t, "randomdir");
    A.random = atom(t, "random"); A.caret = atom(t, "^"); A.vee = atom(t, "v"); A.lt = atom(t, "<"); A.gt = atom(t, ">");
    A.parallel = atom(t, "parallel"); A.perpendicular = atom(t, "perpendicular"); A.horizontal = atom(t, "horizontal");
    A.vertical = atom(t, "vertical"); A.orthogonal = atom(t, "orthogonal");
    A.horizontal_par = atom(t, "horizontal_par"); A.horizontal_perp = atom(t, "horizontal_perp");
    A.vertical_par = atom(t, "vertical_par"); A.vertical_perp = atom(t, "vertical_perp");
    A.late = atom(t, "late"); A.rigid = atom(t, "rigid"); A.plus = atom(t, "+"); A.lbr = atom(t, "[");
    A.rbr = atom(t, "]"); A.bar = atom(t, "|"); A.arrow = atom(t, "->"); A.startloop = atom(t, "startloop");
    A.endloop = atom(t, "endloop"); A.background = atom(t, "background"); A.player = atom(t, "player");
    A.all = atom(t, "\nall\n"); A.message = atom(t, "message");
}
#define S(a) ATOM_STR(c, a)

// directionaggregates; returns count (0 when not an aggregate)
static int dir_aggregate(int d, int out[5])
{
    int n = 0;
    if (d == A.horizontal || d == A.horizontal_par || d == A.horizontal_perp) { out[n++] = A.left; out[n++] = A.right; }
    else if (d == A.vertical || d == A.vertical_par || d == A.vertical_perp) { out[n++] = A.up; out[n++] = A.down; }
    else if (d == A.moving) { out[n++] = A.up; out[n++] = A.down; out[n++] = A.left; out[n++] = A.right; out[n++] = A.action; }
    else if (d == A.orthogonal) { out[n++] = A.up; out[n++] = A.down; out[n++] = A.left; out[n++] = A.right; }
    else if (d == A.perpendicular) { out[n++] = A.caret; out[n++] = A.vee; }
    else if (d == A.parallel) { out[n++] = A.lt; out[n++] = A.gt; }
    return n;
}
static bool is_relative_dir(int d)   // relativeDirections
{
    return d == A.caret || d == A.vee || d == A.lt || d == A.gt || d == A.perpendicular || d == A.parallel;
}
static bool is_simple_abs(int d) { return d == A.up || d == A.down || d == A.left || d == A.right; }
static bool is_simple_rel(int d) { return d == A.caret || d == A.vee || d == A.lt || d == A.gt; }
static bool is_directions_only(int d)   // reg_directions_only
{
    return d == A.gt || d == A.lt || d == A.caret || d == A.vee || d == A.up || d == A.down || d == A.left ||
           d == A.right || d == A.moving || d == A.stationary || d == A.no || d == A.randomdir || d == A.random ||
           d == A.horizontal || d == A.vertical || d == A.orthogonal || d == A.perpendicular || d == A.parallel ||
           d == A.action;
}
// dirMasks; -1 when undefined
static int dir_mask(int d)
{
    if (d == A.up) return 1;
    if (d == A.down) return 2;
    if (d == A.left) return 4;
    if (d == A.right) return 8;
    if (d == A.moving) return 15;
    if (d == A.no) return 3;
    if (d == A.randomdir) return 5;
    if (d == A.random) return 18;
    if (d == A.action) return 16;
    if (d == A.empty) return 0;
    return -1;
}

static const char *const commandwords[] = {
    "sfx0", "sfx1", "sfx2", "sfx3", "sfx4", "sfx5", "sfx6", "sfx7", "sfx8", "sfx9", "sfx10", "cancel",
    "checkpoint", "restart", "win", "message", "again", NULL
};
static int command_index(const char *s)
{
    for (int i = 0; commandwords[i]; i++) if (strcmp(commandwords[i], s) == 0) return i;
    return -1;
}

// =============================================================================================
// bit vectors (bitvec.js) as uint32 word arrays

static uint32_t *bv_new(Arena *a, int n) { return ar_alloc(a, sizeof(uint32_t) * (n > 0 ? n : 1)); }
static void bv_ior(uint32_t *a, const uint32_t *b, int n) { for (int i = 0; i < n; i++) a[i] |= b[i]; }
static void bv_iclear(uint32_t *a, const uint32_t *b, int n) { for (int i = 0; i < n; i++) a[i] &= ~b[i]; }
static bool bv_iszero(const uint32_t *a, int n) { for (int i = 0; i < n; i++) if (a[i]) return false; return true; }
static bool bv_any_common(const uint32_t *a, const uint32_t *b, int n) { for (int i = 0; i < n; i++) if (a[i] & b[i]) return true; return false; }
static bool bv_bits_set_in(const uint32_t *a, const uint32_t *arr, int n) { for (int i = 0; i < n; i++) if ((a[i] & arr[i]) != a[i]) return false; return true; }
static void bv_set(uint32_t *a, int ind, int n)
{
    if (ind < 0) ind = 0;              // JS: ibitset(undefined) sets bit 0
    if ((ind >> 5) < n) a[ind >> 5] |= 1u << (ind & 31);
}
static bool bv_get(const uint32_t *a, int ind) { return (a[ind >> 5] >> (ind & 31)) & 1; }
static void bv_shiftor(uint32_t *a, uint32_t mask, int shift, int n)
{
    int inner = shift & 31, outer = shift >> 5;
    if (outer < n) a[outer] |= mask << inner;
    if (inner > 27 && outer + 1 < n) a[outer + 1] |= mask >> (32 - inner);
}

// =============================================================================================
// intermediate rule representation (compiler.js rule objects)

typedef struct { int n; IVec *c; } CRow;     // cell row; cells are [dir, name, dir, name, ...]
typedef struct { int n; CRow *r; } HS;
typedef struct { int type; char *msg; } Cmd;
typedef struct { int n; Cmd *v; } Cmds;
typedef struct { int k1, k2; int a[6]; } RepEnt;
typedef struct { int n, cap; RepEnt *v; } RepDict;

typedef struct Rule {
    IVec directions;
    int direction;
    HS lhs, rhs;
    int lineNumber, groupNumber;
    bool late, rigid, randomRule;
    Cmds *commands;
    bool hasDiscard; int discard0, discard1;
    bool ellipsisAtEdge;
    unsigned repHash;
    RepDict *movingReplacement, *aggregateDirReplacement, *propertyReplacement;
    PSCellPattern **pats;    // rulesToMask output: per lhs row, cells
    // interned by rulesToMask while still in cache: per row the cells and cellRowMasks(_Movements),
    // and the ruleMask (collapseRules reads them in group order, long after)
    PSCellPattern **cellsI; uint32_t **rowMaskI, **rowMaskMovI; uint32_t *ruleMaskI;
} Rule;

static RepEnt *rd_get(RepDict *d, int k1, int k2)
{
    for (int i = 0; i < d->n; i++) if (d->v[i].k1 == k1 && d->v[i].k2 == k2) return &d->v[i];
    return NULL;
}
static RepEnt *rd_add(Arena *a, RepDict *d, int k1, int k2)
{
    if (d->n == d->cap) {
        int nc = d->cap ? d->cap * 2 : 4;
        RepEnt *nv = ar_alloc_raw(a, sizeof(RepEnt) * nc);
        if (d->n) memcpy(nv, d->v, sizeof(RepEnt) * d->n);
        d->v = nv; d->cap = nc;
    }
    RepEnt *e = &d->v[d->n++];
    memset(e, 0, sizeof *e);
    e->k1 = k1; e->k2 = k2;
    return e;
}
static RepDict *rd_clone(Arena *a, const RepDict *s)
{
    RepDict *d = ar_alloc(a, sizeof *d);
    if (s && s->n) { d->n = d->cap = s->n; d->v = ar_alloc_raw(a, sizeof(RepEnt) * s->n); memcpy(d->v, s->v, sizeof(RepEnt) * s->n); }
    return d;
}

static HS hs_clone(Arena *a, const HS *h)
{
    HS r; r.n = h->n; r.r = h->n ? ar_alloc_raw(a, sizeof(CRow) * h->n) : NULL;
    for (int i = 0; i < h->n; i++) {
        r.r[i].n = h->r[i].n;
        r.r[i].c = h->r[i].n ? ar_alloc_raw(a, sizeof(IVec) * h->r[i].n) : NULL;
        for (int j = 0; j < h->r[i].n; j++) r.r[i].c[j] = iv_clone(a, &h->r[i].c[j]);
    }
    return r;
}
// deepCloneRule
static Rule *rule_clone(PSComp *c, const Rule *s)
{
    Rule *r = ar_alloc(&c->tmp, sizeof *r);
    r->direction = s->direction;
    r->lhs = hs_clone(&c->tmp, &s->lhs);
    r->rhs = hs_clone(&c->tmp, &s->rhs);
    r->lineNumber = s->lineNumber; r->late = s->late; r->rigid = s->rigid;
    r->groupNumber = s->groupNumber; r->commands = s->commands; r->randomRule = s->randomRule;
    return r;
}

static IVec *dict_list(ADict *d, int k) { return (IVec *)ad_get(d, k); }
static int syn_get(PSComp *c, int k) { return ad_has(&c->synonymsDict, k) ? (int)(intptr_t)ad_get(&c->synonymsDict, k) - 1 : -1; }
static void syn_set(PSComp *c, int k, int v) { ad_set(&c->synonymsDict, k, (void *)(intptr_t)(v + 1)); }
static int single_layer(PSComp *c, int k, bool *has)   // propertiesSingleLayer
{
    *has = ad_has(&c->propertiesSingleLayer, k);
    return *has ? (int)(intptr_t)ad_get(&c->propertiesSingleLayer, k) - 2 : -1;
}
static uint32_t *obj_mask(PSComp *c, int k) { return (uint32_t *)ad_get(&c->objectMasks, k); }

// =============================================================================================
// colours

static bool is_hex_color(const char *s)   // REGEX_HEX
{
    int n = (int)strlen(s);
    if (n != 4 && n != 7) return false;
    if (s[0] != '#') return false;
    for (int i = 1; i < n; i++) {
        char ch = s[i];
        if (!((ch >= '0' && ch <= '9') || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F'))) return false;
    }
    return true;
}
static bool is_color(PSComp *c, const char *str)
{
    const char *s = js_trim(&c->tmp, str, (int)strlen(str));
    return ps_color_name_index(s) >= 0 || is_hex_color(s) || strcmp(s, "transparent") == 0;
}
static const char *color_to_hex(PSComp *c, const char *str)
{
    const char *s = js_trim(&c->tmp, str, (int)strlen(str));
    int i = ps_color_name_index(s);
    return i >= 0 ? c->colorPalette[i] : s;
}
static int hexval(char ch) { return (ch >= '0' && ch <= '9') ? ch - '0' : (ch >= 'a' && ch <= 'f') ? ch - 'a' + 10 : (ch >= 'A' && ch <= 'F') ? ch - 'A' + 10 : 0; }
static uint32_t color_rgb(const char *s)
{
    if (strcmp(s, "transparent") == 0) return PS_COLOR_TRANSPARENT;
    int n = (int)strlen(s);
    if (n == 4) return (uint32_t)(hexval(s[1]) * 0x11) << 16 | (uint32_t)(hexval(s[2]) * 0x11) << 8 | (uint32_t)(hexval(s[3]) * 0x11);
    if (n == 7) {
        uint32_t v = 0;
        for (int i = 1; i < 7; i++) v = v * 16 + (uint32_t)hexval(s[i]);
        return v;
    }
    return 0xFF00FF;
}

// =============================================================================================
// generateExtraMembers

static int obj_layer(PSComp *c, int name) { PObj *o = ps_obj(c, name); return o ? o->layer : -1; }

// expandLegendDictEntries
static bool expand_legend_dict(PSComp *c, ADict *dict, ADict *conflicting, const char *kind, const char *ckind)
{
    bool modified = false;
    int nk; int *keys = ad_keys(c, dict, &nk);
    for (int ki = 0; ki < nk; ki++) {
        int n = keys[ki];
        IVec *values = dict_list(dict, n);
        if (!values) continue;
        for (int i = 0; i < values->n; i++) {
            int value = values->v[i];
            int sv = syn_get(c, value);
            if (sv >= 0) {
                values->v[i] = sv;
                modified = true;
            } else if (ad_has(dict, value)) {
                iv_splice(values, i, 1);
                IVec *nv = dict_list(dict, value);
                for (int j = 0; nv && j < nv->n; j++) {
                    int x = nv->v[j];
                    if (!iv_has(values, x)) iv_push(&c->tmp, values, x);
                }
                modified = true;
            }
            if (ad_has(conflicting, value))
                ps_log_error(c, -1, "Trying to define %s \"%S\" in terms of %s \"%S\".", kind, S(n), ckind, S(value));
        }
    }
    return modified;
}

static void generate_extra_members(PSComp *c)
{
    // ids and layers
    c->idDict.n = 0;
    int idcount = 0;
    for (int li = 0; li < c->collisionLayers.n; li++) {
        IVec *layer = c->collisionLayers.v[li];
        for (int j = 0; j < layer->n; j++) {
            PObj *o = ps_obj(c, layer->v[j]);
            if (o) {
                o->layer = li; o->id = idcount;
                iv_push(&c->tmp, &c->idDict, layer->v[j]);
                idcount++;
            }
        }
    }
    c->objectCount = idcount;
    c->layerCount = c->collisionLayers.n;
    c->strideObj = (idcount + 31) / 32;
    c->strideMov = (c->layerCount + 4) / 5;

    // colour palette
    c->colorPalette = ps_palettes[3];   // arnecolors
    for (int i = 0; i < c->metadata.n; i++) {
        PMeta *m = c->metadata.v[i];
        if (strcmp(m->key, "color_palette") == 0) {
            const char *val = m->val;
            for (int k = 0; ps_palette_aliases[k][0]; k++) if (strcmp(ps_palette_aliases[k][0], val) == 0) val = ps_palette_aliases[k][1];
            int found = -1;
            for (int k = 0; k < PS_PALETTE_COUNT; k++) if (strcmp(ps_palette_names[k], val) == 0) found = k;
            if (found < 0) ps_log_error(c, 0, "Palette \"%s\" not found, defaulting to arnecolors.", val);
            else c->colorPalette = ps_palettes[found];
        }
    }

    // colours to hex, sprites, glyphs
    ad_init(&c->glyphDict, &c->tmp);
    int nk; int *keys = ad_keys(c, &c->objects, &nk);
    for (int ki = 0; ki < nk; ki++) {
        int n = keys[ki];
        PObj *o = ps_obj(c, n);
        if (o->colors.n > 10)
            ps_log_error(c, o->lineNumber + 1, "a sprite cannot have more than 10 colors.  Why you would want more than 10 is beyond me.");
        const char **cols = ar_alloc(&c->tmp, sizeof(char *) * (o->colors.n + 1));
        for (int i = 0; i < o->colors.n; i++) {
            const char *col = o->colors.v[i];
            if (is_color(c, col)) cols[i] = color_to_hex(c, col);
            else {
                ps_log_error(c, o->lineNumber + 1, "Invalid color specified for object \"%s\", namely \"%s\".", S(n), col);
                cols[i] = "#ff00ff";
            }
        }
        int ncol = o->colors.n;
        if (ncol == 0) {
            ps_log_error(c, o->lineNumber, "color not specified for object \"%s\".", S(n));
            cols[0] = "#ff00ff"; ncol = 1;
        }
        o->ncolors = ncol;
        o->rgb = ar_alloc(c->out, sizeof(uint32_t) * ncol);
        for (int i = 0; i < ncol; i++) o->rgb[i] = color_rgb(cols[i]);

        PVec *sm = o->spritematrix;
        if (!sm || sm->n == 0) {
            o->spriteN = 5;
            o->sprite = ar_alloc(c->out, 25);   // all colour 0
        } else {
            bool is5 = sm->n == 5;
            for (int i = 0; i < sm->n && is5; i++) if (strlen(sm->v[i]) != 5) is5 = false;
            if (!is5) ps_log_warning(c, o->lineNumber, "Sprite graphics must be 5 wide and 5 high exactly.");
            int N = sm->n;
            o->spriteN = N;
            o->sprite = ar_alloc(c->out, N * N);
            for (int i = 0; i < N; i++) {
                const char *row = sm->v[i];
                int rl = (int)strlen(row);
                for (int j = 0; j < N; j++) {
                    char ch = j < rl ? row[j] : 0;
                    int v = (ch >= '0' && ch <= '9') ? ch - '0' : -1;   // '.' and NaN are transparent
                    if (v >= ncol) v = -1;
                    o->sprite[i * N + j] = (int8_t)v;
                }
            }
        }
        int *mask = ar_alloc(&c->tmp, sizeof(int) * (c->layerCount + 1));
        for (int i = 0; i < c->layerCount; i++) mask[i] = -1;
        if (o->layer >= 0) mask[o->layer] = o->id;
        ad_set(&c->glyphDict, n, mask);
    }

    bool added = true;
    while (added) {
        added = false;
        for (int i = 0; i < c->legend_synonyms.n; i++) {
            PLegend *d = c->legend_synonyms.v[i];
            if (!ad_has(&c->glyphDict, d->name) && ad_has(&c->glyphDict, d->vals.v[0])) {
                added = true;
                ad_set(&c->glyphDict, d->name, ad_get(&c->glyphDict, d->vals.v[0]));
            }
        }
        for (int i = 0; i < c->legend_aggregates.n; i++) {
            PLegend *d = c->legend_aggregates.v[i];
            bool allFound = true;
            for (int j = 0; j < d->vals.n; j++) if (!ad_has(&c->glyphDict, d->vals.v[j])) { allFound = false; break; }
            if (!ad_has(&c->glyphDict, d->name) && allFound) {
                int *mask = ar_alloc(&c->tmp, sizeof(int) * (c->layerCount + 1));
                for (int k = 0; k < c->layerCount; k++) mask[k] = -1;
                for (int j = 0; j < d->vals.n; j++) {
                    int n = d->vals.v[j];
                    PObj *o = ps_obj(c, n);
                    if (!o) { ps_log_error(c, c->lineNumber, "Object not found with name %s", S(n)); continue; }
                    if (o->layer < 0) {
                        ps_log_error(c, d->lineNumber, "Object \"%S\" has been defined, but not assigned to a layer.", S(n));
                    } else if (mask[o->layer] == -1) {
                        mask[o->layer] = o->id;
                    } else {
                        ps_log_error(c, d->lineNumber, "Trying to create an aggregate object (something defined in the LEGEND section using AND) with both \"%S\" and \"%S\", which are on the same layer and therefore can't coexist.",
                                     S(n), S(c->idDict.v[mask[o->layer]]));
                    }
                }
                added = true;
                ad_set(&c->glyphDict, d->name, mask);
            }
        }
    }

    ad_init(&c->aggregatesDict, &c->tmp);
    for (int i = 0; i < c->legend_aggregates.n; i++) {
        PLegend *e = c->legend_aggregates.v[i];
        IVec *v = ar_alloc(&c->tmp, sizeof(IVec)); *v = iv_clone(&c->tmp, &e->vals);
        ad_set(&c->aggregatesDict, e->name, v);
    }
    ad_init(&c->propertiesDict, &c->tmp);
    for (int i = 0; i < c->legend_properties.n; i++) {
        PLegend *e = c->legend_properties.v[i];
        IVec *v = ar_alloc(&c->tmp, sizeof(IVec)); *v = iv_clone(&c->tmp, &e->vals);
        ad_set(&c->propertiesDict, e->name, v);
    }
    ad_init(&c->synonymsDict, &c->tmp);
    for (int i = 0; i < c->legend_synonyms.n; i++) {
        PLegend *e = c->legend_synonyms.v[i];
        int key = e->name, value = e->vals.v[0];
        if (ad_has(&c->aggregatesDict, value)) ad_set(&c->aggregatesDict, key, ad_get(&c->aggregatesDict, value));
        else if (ad_has(&c->propertiesDict, value)) ad_set(&c->propertiesDict, key, ad_get(&c->propertiesDict, value));
        else if (key != value) syn_set(c, key, value);
    }

    bool modified = true;
    int guard = 0;
    while (modified && guard++ < 1000) {
        modified = false;
        int ns; int *skeys = ad_keys(c, &c->synonymsDict, &ns);
        for (int ki = 0; ki < ns; ki++) {
            int n = skeys[ki];
            if (!ad_has(&c->synonymsDict, n)) continue;
            int value = syn_get(c, n);
            if (ad_has(&c->propertiesDict, value)) {
                ad_del(&c->synonymsDict, n);
                ad_set(&c->propertiesDict, n, ad_get(&c->propertiesDict, value));
                modified = true;
            } else if (ad_has(&c->aggregatesDict, value)) {
                void *v = ad_get(&c->aggregatesDict, value);
                ad_del(&c->aggregatesDict, n);
                ad_set(&c->aggregatesDict, n, v);
                modified = true;
            } else if (ad_has(&c->synonymsDict, value)) {
                syn_set(c, n, syn_get(c, value));
            }
        }
        modified = expand_legend_dict(c, &c->propertiesDict, &c->aggregatesDict, "property", "aggregate") || modified;
        modified = expand_legend_dict(c, &c->aggregatesDict, &c->propertiesDict, "aggregate", "property") || modified;
    }

    // propertiesSingleLayer
    ad_init(&c->propertiesSingleLayer, &c->tmp);
    int np; int *pkeys = ad_keys(c, &c->propertiesDict, &np);
    for (int ki = 0; ki < np; ki++) {
        int key = pkeys[ki];
        IVec *values = dict_list(&c->propertiesDict, key);
        bool sameLayer = true;
        for (int i = 0; i < values->n; i++) {
            PObj *o = ps_obj(c, values->v[i]);
            if (!o) { sameLayer = false; break; }
            if (i > 0 && o->layer != obj_layer(c, values->v[i - 1])) { sameLayer = false; break; }
        }
        if (sameLayer && values->n > 0) {
            PObj *first = ps_obj(c, values->v[0]);
            if (first) ad_set(&c->propertiesSingleLayer, key, (void *)(intptr_t)(first->layer + 2));
        }
    }

    if (c->idDict.n == 0 && c->collisionLayers.n > 0) ps_log_error(c, -1, "You need to have some objects defined");

    // default background object
    c->backgroundid = -1; c->backgroundlayer = -1;
    PObj *bg = ps_obj(c, A.background);
    if (!bg) {
        int bgLine = ad_has(&c->original_line_numbers, A.background) ? (int)(intptr_t)ad_get(&c->original_line_numbers, A.background) - 1 : -1;
        if (syn_get(c, A.background) >= 0) {
            PObj *o = ps_obj(c, syn_get(c, A.background));
            if (o) { c->backgroundid = o->id; c->backgroundlayer = o->layer; }
        } else if (ad_has(&c->propertiesDict, A.background)) {
            IVec *def = dict_list(&c->propertiesDict, A.background);
            PObj *o = def->n ? ps_obj(c, def->v[0]) : NULL;
            if (o) {
                c->backgroundid = o->id; c->backgroundlayer = o->layer;
                for (int i = 1; i < def->n; i++) {
                    PObj *on = ps_obj(c, def->v[i]);
                    if (!on) continue;
                    if (on->layer != c->backgroundlayer) ps_log_error(c, bgLine, "Background objects must be on the same layer");
                }
            }
        } else if (ad_has(&c->aggregatesDict, A.background)) {
            PObj *o = c->idDict.n ? ps_obj(c, c->idDict.v[0]) : NULL;
            if (o) { c->backgroundid = o->id; c->backgroundlayer = o->layer; }
            ps_log_error(c, bgLine, "background cannot be an aggregate (declared with 'and'), it has to be a simple type, or property (declared in terms of others using 'or').");
        } else {
            PObj *o = c->idDict.n ? ps_obj(c, c->idDict.v[0]) : NULL;
            if (o) { c->backgroundid = o->id; c->backgroundlayer = o->layer; }
            ps_log_error(c, -1, "Seriously, you have to define something to be the background.");
        }
    } else {
        c->backgroundid = bg->id; c->backgroundlayer = bg->layer;
    }
}

// =============================================================================================
// generateMasks

static uint32_t *get_mask_from_name(PSComp *c, int name, bool *aggregate)
{
    uint32_t *m = bv_new(c->out, c->strideObj);
    *aggregate = false;
    PObj *o = ps_obj(c, name);
    if (o) bv_set(m, o->id, c->strideObj);
    if (ad_has(&c->aggregatesDict, name)) {
        IVec *v = dict_list(&c->aggregatesDict, name);
        *aggregate = true;
        for (int i = 0; i < v->n; i++) { PObj *x = ps_obj(c, v->v[i]); if (x) bv_set(m, x->id, c->strideObj); }
    }
    if (ad_has(&c->propertiesDict, name)) {
        IVec *v = dict_list(&c->propertiesDict, name);
        for (int i = 0; i < v->n; i++) { PObj *x = ps_obj(c, v->v[i]); if (x) bv_set(m, x->id, c->strideObj); }
    }
    if (syn_get(c, name) >= 0) { PObj *x = ps_obj(c, syn_get(c, name)); if (x) bv_set(m, x->id, c->strideObj); }
    if (bv_iszero(m, c->strideObj))
        ps_log_error(c, -1, "Error, didn't find any object called %s, either in the objects section, or the legends section.", S(name));
    return m;
}

static int cmp_legend_line(const void *a, const void *b)
{
    const PLegend *x = *(PLegend *const *)a, *y = *(PLegend *const *)b;
    return x->lineNumber - y->lineNumber;
}

static void generate_masks(PSComp *c)
{
    int so = c->strideObj;
    c->playerMask = get_mask_from_name(c, A.player, &c->playerAggregate);
    c->layerMasks = ar_alloc(c->out, sizeof(uint32_t *) * (c->layerCount + 1));
    for (int layer = 0; layer < c->layerCount; layer++) {
        uint32_t *lm = bv_new(c->out, so);
        for (int j = 0; j < c->objectCount; j++) {
            PObj *o = ps_obj(c, c->idDict.v[j]);
            if (o->layer == layer) bv_set(lm, o->id, so);
        }
        c->layerMasks[layer] = lm;
    }
    ad_init(&c->objectMasks, &c->tmp);
    int nk; int *keys = ad_keys(c, &c->objects, &nk);
    for (int ki = 0; ki < nk; ki++) {
        PObj *o = ps_obj(c, keys[ki]);
        uint32_t *m = bv_new(c->out, so);
        bv_set(m, o->id, so);
        ad_set(&c->objectMasks, keys[ki], m);
    }
    // synonyms and properties, stable-sorted by line number
    int ns = c->legend_synonyms.n + c->legend_properties.n;
    PLegend **sp = ar_alloc(&c->tmp, sizeof(PLegend *) * (ns + 1));
    int k = 0;
    for (int i = 0; i < c->legend_synonyms.n; i++) sp[k++] = c->legend_synonyms.v[i];
    for (int i = 0; i < c->legend_properties.n; i++) sp[k++] = c->legend_properties.v[i];
    for (int i = 1; i < ns; i++) {   // stable insertion sort
        PLegend *x = sp[i]; int j = i - 1;
        while (j >= 0 && cmp_legend_line(&sp[j], &x) > 0) { sp[j + 1] = sp[j]; j--; }
        sp[j + 1] = x;
    }
    for (int i = 0; i < ns; i++) {
        PLegend *l = sp[i];
        if (l->vals.n == 1) {
            uint32_t *m = obj_mask(c, l->vals.v[0]);
            if (m) ad_set(&c->objectMasks, l->name, m);
        } else {
            uint32_t *val = bv_new(c->out, so);
            for (int j = 0; j < l->vals.n; j++) { uint32_t *m = obj_mask(c, l->vals.v[j]); if (m) bv_ior(val, m, so); }
            ad_set(&c->objectMasks, l->name, val);
        }
    }
    uint32_t *all = bv_new(c->out, so);
    for (int i = 0; i < so; i++) all[i] = 0xFFFFFFFFu;
    ad_set(&c->objectMasks, A.all, all);

    ad_init(&c->aggregateMasks, &c->tmp);
    int na; int *akeys = ad_keys(c, &c->aggregatesDict, &na);
    for (int ki = 0; ki < na; ki++) {
        IVec *v = dict_list(&c->aggregatesDict, akeys[ki]);
        uint32_t *m = bv_new(c->out, so);
        for (int i = 0; i < v->n; i++) { uint32_t *om = obj_mask(c, v->v[i]); if (om) bv_ior(m, om, so); }
        ad_set(&c->aggregateMasks, akeys[ki], m);
    }
}

// =============================================================================================
// levelsToArray / levelFromString

// number of lines wordwrap(str, 34) produces
static int wordwrap_count(PSComp *c, const char *str, int width)
{
    int len = (int)strlen(str);
    if (!len) return 0;
    int *cps = ar_alloc(&c->tmp, sizeof(int) * (len + 1)); int n = 0;
    for (int p = 0; p < len;) { int cp; p += u8_decode(str, len, p, &cp); cps[n++] = cp; }
    int count = 0, pos = 0;
    while (pos < n) {
        int avail = 0;   // chars matching '.'
        while (pos + avail < n && cps[pos + avail] != '\n' && cps[pos + avail] != '\r' && cps[pos + avail] != 0x2028 && cps[pos + avail] != 0x2029) avail++;
        int m = 0;
        for (int k = avail < width ? avail : width; k >= 1; k--) {
            if (pos + k == n) { m = k; break; }
            if (js_is_space(cps[pos + k])) { m = k + 1; break; }
        }
        if (!m && avail >= width) m = width;
        if (!m && avail >= 1 && pos + avail == n) m = avail;
        if (m) { count++; pos += m; } else pos++;
    }
    return count;
}

static void level_from_string(PSComp *c, PLevel *lv, PSLevelDef *o)
{
    int so = c->strideObj;
    int H = lv->rows.n;
    // rows as code point string arrays
    int **rows = ar_alloc(&c->tmp, sizeof(int *) * H);
    int *rlen = ar_alloc(&c->tmp, sizeof(int) * H);
    for (int j = 0; j < H; j++) {
        const char *r = lv->rows.v[j]; int len = (int)strlen(r);
        rows[j] = ar_alloc(&c->tmp, sizeof(int) * (len + 1));
        int k = 0;
        for (int p = 0; p < len;) { int cp; p += u8_decode(r, len, p, &cp); rows[j][k++] = cp; }
        rlen[j] = k;
    }
    int W = rlen[0];
    o->isMessage = false; o->lineNumber = lv->lineNumber; o->width = W; o->height = H;
    o->objects = ar_alloc_raw(c->out, sizeof(uint32_t) * (W * H * so + 1));
    o->objects[W * H * so] = 0;
    ADict cache; ad_init(&cache, &c->tmp);
    uint32_t *ascii[128];   // glyph masks by code point (skips the atom + dictionary lookups per cell)
    memset(ascii, 0, sizeof ascii);
    for (int i = 0; i < W; i++) {
        for (int j = 0; j < H; j++) {
            int cp = i < rlen[j] ? rows[j][i] : (rlen[j] ? rows[j][rlen[j] - 1] : -1);
            uint32_t *maskint = cp >= 0 && cp < 128 ? ascii[cp] : NULL;
            if (maskint) { memcpy(o->objects + so * (i * H + j), maskint, sizeof(uint32_t) * so); continue; }
            char buf[5] = { 0 };
            if (cp >= 0) u8_encode((uint32_t)cp, buf);
            int a = atom(&c->atoms, buf);
            maskint = ad_get(&cache, a);
            if (!maskint) {
                int *mask = ad_get(&c->glyphDict, a);
                if (!mask) {
                    if (!ad_has(&c->propertiesDict, a))
                        ps_log_error(c, lv->lineNumber + j, "Error, symbol \"%s\", used in map, not found.", buf);
                    else
                        ps_log_error(c, lv->lineNumber + j, "Error, symbol \"%s\" is defined using OR, and therefore ambiguous - it cannot be used in a map. Did you mean to define it in terms of AND?", buf);
                    memset(o->objects + so * (i * H + j), 0, sizeof(uint32_t) * (size_t)(W * H - (i * H + j)) * so);   // cells not written yet
                    return;
                }
                maskint = bv_new(&c->tmp, so);
                for (int z = 0; z < c->layerCount; z++) if (mask[z] >= 0) bv_set(maskint, mask[z], so);
                ad_set(&cache, a, maskint);
            }
            if (cp >= 0 && cp < 128) ascii[cp] = maskint;
            memcpy(o->objects + so * (i * H + j), maskint, sizeof(uint32_t) * so);
        }
    }
    // calcBackgroundMask
    int n_tiles = W * H;
    uint32_t *bgmask = bv_new(&c->tmp, so);
    uint32_t *blm = c->backgroundlayer >= 0 && c->backgroundlayer < c->layerCount ? c->layerMasks[c->backgroundlayer] : NULL;
    if (!blm) {
        ps_log_error(c, -1, "you have to have a background layer");
        if (c->backgroundid >= 0) bv_set(bgmask, c->backgroundid, so);
    } else {
        bool found = false;
        for (int i = 0; i < n_tiles && !found; i++) {
            uint32_t *cell = o->objects + so * i;
            for (int w = 0; w < so; w++) bgmask[w] = cell[w] & blm[w];
            if (!bv_iszero(bgmask, so)) found = true;
        }
        if (!found) { memset(bgmask, 0, sizeof(uint32_t) * so); bv_set(bgmask, c->backgroundid, so); }
    }
    for (int i = 0; i < n_tiles; i++) {
        uint32_t *cell = o->objects + so * i;
        if (!blm || !bv_any_common(blm, cell, so)) bv_ior(cell, bgmask, so);
    }
}

static void levels_to_array(PSComp *c)
{
    PSGame *g = c->g;
    int n = 0;
    for (int i = 0; i < c->levels.n; i++) if (((PLevel *)c->levels.v[i])->type != LV_EMPTY) n++;
    g->levels = ar_alloc(c->out, sizeof(PSLevelDef) * (n + 1));
    g->levelCount = 0;
    for (int i = 0; i < c->levels.n; i++) {
        PLevel *lv = c->levels.v[i];
        if (lv->type == LV_EMPTY) continue;
        PSLevelDef *o = &g->levels[g->levelCount++];
        if (lv->type == LV_MESSAGE) {
            o->isMessage = true;
            o->message = ar_strdup(c->out, lv->message);
            o->lineNumber = lv->lineNumber;
            if (wordwrap_count(c, lv->message, PS_TERMINAL_WIDTH) > 12)
                ps_log_warning(c, lv->lineNumber, "Message too long to fit on screen.");
        } else {
            level_from_string(c, lv, o);
        }
    }
}

// =============================================================================================
// processRuleString

static bool cell_row_has_relative_dir(const CRow *row)
{
    for (int j = 0; j < row->n; j++)
        for (int k = 0; k < row->c[j].n; k += 2) if (is_relative_dir(row->c[j].v[k])) return true;
    return false;
}
static bool directional_rule(const Rule *r)
{
    for (int i = 0; i < r->lhs.n; i++) {
        if (r->lhs.r[i].n > 1) return true;
        if (cell_row_has_relative_dir(&r->lhs.r[i])) return true;
    }
    for (int i = 0; i < r->rhs.n; i++) if (cell_row_has_relative_dir(&r->rhs.r[i])) return true;
    return false;
}

typedef struct { int n, cap; IVec *v; } CellList;
typedef struct { int n, cap; CRow *v; } RowList;
static void cl_push(Arena *a, CellList *l, IVec x)
{
    if (l->n == l->cap) { int nc = l->cap ? l->cap * 2 : 4; IVec *nv = ar_alloc(a, sizeof(IVec) * nc); if (l->n) memcpy(nv, l->v, sizeof(IVec) * l->n); l->v = nv; l->cap = nc; }
    l->v[l->n++] = x;
}
static void rl_push(Arena *a, RowList *l, CRow x)
{
    if (l->n == l->cap) { int nc = l->cap ? l->cap * 2 : 4; CRow *nv = ar_alloc(a, sizeof(CRow) * nc); if (l->n) memcpy(nv, l->v, sizeof(CRow) * l->n); l->v = nv; l->cap = nc; }
    l->v[l->n++] = x;
}

// tokenizeRuleLine
static IVec tokenize_rule_line(PSComp *c, const char *line, PVec *toks)
{
    SB b; sb_init(&b, &c->tmp);
    for (const char *p = line; *p; p++) {
        if (*p == '[' || *p == ']' || *p == '|') { sb_putc(&b, ' '); sb_putc(&b, *p); sb_putc(&b, ' '); }
        else if (p[0] == '-' && p[1] == '>') { sb_puts(&b, " -> "); p++; }
        else sb_putc(&b, *p);
    }
    char *s = sb_done(&b);
    char *t = js_trim(&c->tmp, s, (int)strlen(s));
    if (t[0] == '+') t = tfmt(&c->tmp, "+ %s", t + 1);
    IVec out = { 0, 0, NULL };
    int len = (int)strlen(t), p = 0;
    while (p < len) {
        int cp, k = u8_decode(t, len, p, &cp);
        if (js_is_space(cp)) { p += k; continue; }
        int q = p;
        while (q < len) { int cp2, k2 = u8_decode(t, len, q, &cp2); if (js_is_space(cp2)) break; q += k2; }
        iv_push(&c->tmp, &out, atom_n(&c->atoms, t + p, q - p));
        if (toks) pv_push(&c->tmp, toks, ar_strndup(&c->tmp, t + p, q - p));
        p = q;
    }
    return out;
}

// returns NULL on error, or a rule; *bracket gets +1/-1 for startloop/endloop (then NULL returned)
static Rule *process_rule_string(PSComp *c, PRuleLine *rl, PVec *curRules, int *bracket)
{
    const char *line = rl->rule;
    int lineNumber = rl->lineNumber;
    const char *origLine = rl->mixed;
    *bracket = 0;
    PVec tokstr = { 0, 0, NULL };
    IVec tokens = tokenize_rule_line(c, line, &tokstr);
    if (tokens.n == 0) ps_log_error(c, lineNumber, "Spooky error!  Empty line passed to rule function.");

    int parsestate = 0;
    IVec directions = { 0, 0, NULL };
    IVec curcell = { 0, 0, NULL };
    CellList curcellrow = { 0, 0, NULL };
    bool incellrow = false, rhs = false, late = false, rigid = false, randomRule = false, has_plus = false;
    RowList lhs_cells = { 0, 0, NULL }, rhs_cells = { 0, 0, NULL };
    int groupNumber = lineNumber;
    Cmds *commands = ar_alloc(&c->tmp, sizeof(Cmds));
    int cmdcap = 0;

    if (tokens.n == 1) {
        if (tokens.v[0] == A.startloop) { *bracket = 1; return NULL; }
        if (tokens.v[0] == A.endloop) { *bracket = -1; return NULL; }
    }
    if (strchr(origLine, '(') && !strstr(line, "->")) {
        ps_log_error(c, lineNumber, "You can't have comments inside rules, sorry. In PuzzleScript, '(' starts a comment, so everything after the first '(' on this line was ignored before your rule was parsed.");
        return NULL;
    }
    if (!iv_has(&tokens, A.arrow))
        ps_log_error(c, lineNumber, "A rule has to have an arrow in it.  There's no arrow here! Consider reading up about rules - you're clearly doing something weird");

    int bracketbalance = 0;
    for (int i = 0; i < tokens.n; i++) {
        int token = tokens.v[i];
        const char *ts = S(token);
        if (parsestate == 0) {
            int agg[5], na;
            if (token == A.plus) {
                has_plus = true;
                if (groupNumber == lineNumber) {
                    if (curRules->n == 0) {
                        ps_log_error(c, lineNumber, "The \"+\" symbol, for joining a rule with the group of the previous rule, needs a previous rule to be applied to.");
                        has_plus = false;
                    }
                    if (i != 0)
                        ps_log_error(c, lineNumber, "The \"+\" symbol, for joining a rule with the group of the previous rule, must be the first symbol on the line ");
                    if (has_plus) groupNumber = ((Rule *)curRules->v[curRules->n - 1])->groupNumber;
                } else {
                    ps_log_error(c, lineNumber, "Two \"+\"s (the \"append to previous rule group\" symbol) applied to the same rule.");
                }
            } else if (is_relative_dir(token)) {
                ps_log_error(c, lineNumber, "You cannot use relative directions (\"^v<>\") to indicate in which direction(s) a rule applies.  Use absolute directions indicators (Up, Down, Left, Right, Horizontal, or Vertical, for instance), or, if you want the rule to apply in all four directions, do not specify directions");
            } else if ((na = dir_aggregate(token, agg)) > 0) {
                for (int k = 0; k < na; k++) iv_push(&c->tmp, &directions, agg[k]);
            } else if (token == A.late) {
                late = true;
            } else if (token == A.rigid) {
                rigid = true;
            } else if (token == A.random) {
                randomRule = true;
                if (has_plus)
                    ps_log_error(c, lineNumber, "A rule-group can only be marked random by the opening rule in the group (aka, a '+' and 'random' can't appear as rule modifiers on the same line).  Why? Well, you see \"random\" isn't a property of individual rules, but of whole rule groups.  It indicates that a single possible application of some rule from the whole group should be applied at random.");
            } else if (is_simple_abs(token)) {
                iv_push(&c->tmp, &directions, token);
            } else if (is_simple_rel(token)) {
                ps_log_error(c, lineNumber, "You cannot use relative directions (\"^v<>\") to indicate in which direction(s) a rule applies.  Use absolute directions indicators (Up, Down, Left, Right, Horizontal, or Vertical, for instance), or, if you want the rule to apply in all four directions, do not specify directions");
            } else if (token == A.lbr) {
                if (directions.n == 0) {
                    iv_push(&c->tmp, &directions, A.up); iv_push(&c->tmp, &directions, A.down);
                    iv_push(&c->tmp, &directions, A.left); iv_push(&c->tmp, &directions, A.right);
                }
                parsestate = 1;
                i--;
            } else {
                ps_log_error(c, lineNumber, "The start of a rule must consist of some number of directions (possibly 0), before the first bracket, specifying in what directions to look (with no direction specified, it applies in all four directions).  It seems you've just entered \"%S\".", ts);
            }
        } else {
            if (token == A.lbr) {
                bracketbalance++;
                if (bracketbalance > 1)
                    ps_log_warning(c, lineNumber, "Multiple opening brackets without closing brackets.  Something fishy here.  Every '[' has to be closed by a ']', and you can't nest them.");
                if (curcell.n > 0)
                    ps_log_error(c, lineNumber, "Error, malformed cell rule - encountered a \"[\"\" before previous bracket was closed");
                incellrow = true;
                curcell = (IVec){ 0, 0, NULL };
            } else if (is_directions_only(token)) {
                if (curcell.n % 2 == 1)
                    ps_log_error(c, lineNumber, "Error, an item can only have one direction/action at a time, but you're looking for several at once!");
                else if (!incellrow)
                    ps_log_warning(c, lineNumber, "Invalid syntax. Directions should be placed at the start of a rule.");
                else if (late && token != A.no && token != A.random)
                    ps_log_error(c, lineNumber, "Movements cannot appear in late rules.");
                else
                    iv_push(&c->tmp, &curcell, token);
            } else if (token == A.bar) {
                if (!incellrow)
                    ps_log_warning(c, lineNumber, "Janky syntax.  \"|\" should only be used inside cell rows (the square brackety bits).");
                else if (curcell.n % 2 == 1)
                    ps_log_error(c, lineNumber, "In a rule, if you specify a movement, it has to act on an object.");
                else {
                    cl_push(&c->tmp, &curcellrow, curcell);
                    curcell = (IVec){ 0, 0, NULL };
                }
            } else if (token == A.rbr) {
                bracketbalance--;
                if (bracketbalance < 0) {
                    ps_log_warning(c, lineNumber, "Multiple closing brackets without corresponding opening brackets.  Something fishy here.  Every '[' has to be closed by a ']', and you can't nest them.");
                    return NULL;
                }
                if (curcell.n % 2 == 1) {
                    if (curcell.v[0] == A.dots) ps_log_error(c, lineNumber, "Cannot end a rule with ellipses.");
                    else ps_log_error(c, lineNumber, "In a rule, if you specify a movement, it has to act on an object.");
                } else {
                    cl_push(&c->tmp, &curcellrow, curcell);
                    curcell = (IVec){ 0, 0, NULL };
                }
                CRow row = { curcellrow.n, curcellrow.v };
                if (rhs) rl_push(&c->tmp, &rhs_cells, row); else rl_push(&c->tmp, &lhs_cells, row);
                curcellrow = (CellList){ 0, 0, NULL };
                incellrow = false;
            } else if (token == A.arrow) {
                if (groupNumber != lineNumber) {
                    Rule *parent = curRules->v[curRules->n - 1];
                    if (parent->late != late)
                        ps_log_warning(c, lineNumber, "Oh gosh you can mix late and non-late rules in a rule-group if you really want to, but gosh why would you want to do that?  What do you expect to accomplish?");
                }
                if (incellrow)
                    ps_log_warning(c, lineNumber, "Encountered an unexpected \"->\" inside square brackets.  It's used to separate states, it has no place inside them >:| .");
                else if (rhs) {
                    ps_log_error(c, lineNumber, "Error, you can only use \"->\" once in a rule; it's used to separate before and after states.");
                    return NULL;
                } else rhs = true;
            } else if (ad_has(&c->names, token)) {
                if (!incellrow) {
                    ps_log_warning(c, lineNumber, "Invalid token %S. Object names should only be used within cells (square brackets).", ts);
                } else {
                    for (int j = 0; j + 1 < curcell.n; j += 2) {
                        if (curcell.v[j + 1] == token) {
                            int existingDir = curcell.v[j];
                            int newDir = (curcell.n % 2 == 1) ? curcell.v[curcell.n - 1] : A.empty;
                            if (existingDir == A.no && newDir == A.no) {
                                ps_log_warning(c, lineNumber, "I notice that you have NO %S more than once in a single cell. That's redundant innit.", ts);
                            } else {
                                ps_log_error(c, lineNumber, "You cannot specify the same object more than once in a single cell (in this case %S occurs multiple times).", ts);
                                if (ad_has(&c->propertiesDict, token))
                                    ps_log_warning_noline(c, "( However, noticing that you're committing this crime with properties, and not being able to help but acknowledge that you may be trying to do something esoteric and clever with the property inference system,  I might be brought to suggest that you consider this: you can have multiple equivalent properties with different names. )");
                            }
                        }
                    }
                    if (curcell.n % 2 == 0) { iv_push(&c->tmp, &curcell, A.empty); iv_push(&c->tmp, &curcell, token); }
                    else iv_push(&c->tmp, &curcell, token);
                }
            } else if (token == A.dots) {
                if (!incellrow)
                    ps_log_warning(c, lineNumber, "Invalid syntax, ellipses should only be used within cells (square brackets).");
                else { iv_push(&c->tmp, &curcell, token); iv_push(&c->tmp, &curcell, token); }
            } else if (command_index(ts) >= 0) {
                if (!rhs) {
                    ps_log_error(c, lineNumber, "Commands should only appear at the end of rules, not in or before the pattern-detection/-replacement sections.");
                } else {
                    bool rbr = false;
                    for (int k = i; k < tokens.n; k++) if (tokens.v[k] == A.rbr) rbr = true;
                    if (incellrow || rbr)
                        ps_log_warning(c, lineNumber, "Commands should only appear at the end of rules, not in or before the pattern-detection/-replacement sections.");
                }
                if (commands->n == cmdcap) {
                    int nc = cmdcap ? cmdcap * 2 : 4;
                    Cmd *nv = ar_alloc(&c->tmp, sizeof(Cmd) * nc);
                    if (commands->n) memcpy(nv, commands->v, sizeof(Cmd) * commands->n);
                    commands->v = nv; cmdcap = nc;
                }
                Cmd *cmd = &commands->v[commands->n++];
                cmd->type = command_index(ts); cmd->msg = NULL;
                if (token == A.message) {
                    // findIndexAfterToken
                    const char *str = js_lower(&c->tmp, origLine);
                    int curIndex = 0, slen = (int)strlen(str);
                    for (int k = 0; k <= i; k++) {
                        const char *tk = tokstr.v[k];
                        const char *f = curIndex <= slen ? strstr(str + curIndex, tk) : NULL;
                        curIndex = (f ? (int)(f - str) : -1) + (int)strlen(tk);
                    }
                    if (curIndex < 0) curIndex = 0;
                    if (curIndex > (int)strlen(origLine)) curIndex = (int)strlen(origLine);
                    char *msg = js_trim(&c->tmp, origLine + curIndex, (int)strlen(origLine + curIndex));
                    if (!msg[0]) msg = " ";
                    cmd->msg = msg;
                    i = tokens.n;
                } else {
                    if (cmd->type <= 10) {
                        bool found = false;
                        for (int j = 0; j < c->sounds.n; j++) {
                            PSound *snd = c->sounds.v[j];
                            if (snd->n > 0 && snd->t[0].text && strcmp(snd->t[0].text, ts) == 0) found = true;
                        }
                        if (!found) ps_log_warning(c, lineNumber, "Sound effect \"%s\" not defined.", ts);
                    }
                }
            } else {
                ps_log_error(c, lineNumber, "Error, malformed cell rule - was looking for cell contents, but found \"%s\".  What am I supposed to do with this, eh, please tell me that.", ts);
            }
        }
    }

    if (late && rigid)
        ps_log_error(c, lineNumber, "Late rules cannot be marked as rigid (rigid rules are all about dealing with the consequences of unresolvable movements, and late rules can't even have movements).");

    if (lhs_cells.n != rhs_cells.n) {
        if (!(commands->n > 0 && rhs_cells.n == 0)) {
            ps_log_warning(c, lineNumber, "Error, when specifying a rule, the number of matches (square bracketed bits) on the left hand side of the arrow must equal the number on the right");
            return NULL;
        }
    } else {
        for (int i = 0; i < lhs_cells.n; i++) {
            if (lhs_cells.v[i].n != rhs_cells.v[i].n) {
                ps_log_error(c, lineNumber, "In a rule, each pattern to match on the left must have a corresponding pattern on the right of equal length (number of cells).");
                c->invalid = true;
            }
            if (lhs_cells.v[i].n == 0)
                ps_log_error(c, -1, "You have an totally empty pattern on the left-hand side.  This will match *everything*.  You certainly don't want this.");
        }
    }
    if (lhs_cells.n == 0) ps_log_error(c, lineNumber, "This rule refers to nothing.  What the heck? :O");

    Rule *r = ar_alloc(&c->tmp, sizeof *r);
    r->directions = directions;
    r->lhs.n = lhs_cells.n; r->lhs.r = lhs_cells.v;
    r->rhs.n = rhs_cells.n; r->rhs.r = rhs_cells.v;
    r->lineNumber = lineNumber; r->late = late; r->rigid = rigid; r->groupNumber = groupNumber;
    r->commands = commands; r->randomRule = randomRule;
    if (!directional_rule(r) && r->directions.n > 1) r->directions.n = 1;
    return r;
}

// =============================================================================================
// checkSuperfluousCoincidences

static void check_superfluous_coincidences(PSComp *c, PVec *rules)
{
    int so = c->strideObj, sl = (c->layerCount + 31) / 32;
    uint32_t *required_layers = bv_new(&c->tmp, sl), *required_objects = bv_new(&c->tmp, so);
    uint32_t *objects_present_mask = bv_new(&c->tmp, so), *no_object_layer_mask = bv_new(&c->tmp, sl);
    for (int ri = 0; ri < rules->n; ri++) {
        Rule *rule = rules->v[ri];
        for (int j = 0; j < rule->rhs.n; j++) {
            CRow *g = &rule->rhs.r[j];
            for (int k = 0; k < g->n; k++) {
                IVec *cell = &g->c[k];
                IVec present = { 0, 0, NULL };
                memset(objects_present_mask, 0, sizeof(uint32_t) * so);
                for (int l = 0; l + 1 < cell->n; l += 2) {
                    if (cell->v[l] != A.no) {
                        int nm = cell->v[l + 1];
                        PObj *o = ps_obj(c, nm);
                        if (o) { iv_push(&c->tmp, &present, nm); bv_set(objects_present_mask, o->id, so); }
                    }
                }
                for (int l = 0; l + 1 < cell->n; l += 2) {
                    if (cell->v[l] == A.no) {
                        int nm = cell->v[l + 1];
                        uint32_t *m = obj_mask(c, nm);
                        if (!m) continue;
                        if (bv_any_common(m, objects_present_mask, so)) {
                            SB b; sb_init(&b, &c->tmp);
                            for (int q = 0; q < present.n; q++) { if (q) sb_puts(&b, ", "); sb_puts(&b, S(present.v[q])); }
                            ps_log_error(c, rule->lineNumber, "You have specified that there should be NO %S but there is also a requirement that %S be here.  This is a mistake right?", S(nm), sb_done(&b));
                        }
                    }
                }
            }
        }
        for (int j = 0; j < rule->rhs.n; j++) {
            CRow *g = &rule->rhs.r[j];
            for (int k = 0; k < g->n; k++) {
                IVec *cell = &g->c[k];
                for (int l = 0; l < cell->n; l += 2) {
                    if (cell->v[l] == A.no && l + 1 < cell->n) {
                        int nm = cell->v[l + 1];
                        if (j >= rule->lhs.n || k >= rule->lhs.r[j].n) continue;
                        IVec *lc = &rule->lhs.r[j].c[k];
                        for (int m = 0; m + 1 < lc->n; m += 2) {
                            if (lc->v[m] == A.no && lc->v[m + 1] == nm) { iv_splice(cell, l, 2); break; }
                        }
                    }
                }
            }
        }
        for (int j = 0; j < rule->lhs.n; j++) {
            CRow *g = &rule->lhs.r[j];
            for (int k = 0; k < g->n; k++) {
                IVec *cell = &g->c[k];
                memset(required_layers, 0, sizeof(uint32_t) * sl);
                memset(required_objects, 0, sizeof(uint32_t) * so);
                int *occupier = ar_alloc(&c->tmp, sizeof(int) * (c->layerCount + 1));
                for (int q = 0; q < c->layerCount; q++) occupier[q] = -1;
                for (int l = 0; l + 1 < cell->n; l += 2) {
                    if (cell->v[l] == A.no) continue;
                    int en = cell->v[l + 1];
                    bool has; int layer = single_layer(c, en, &has);
                    if (has) {
                        bv_set(required_layers, layer, sl);
                        IVec *po = dict_list(&c->propertiesDict, en);
                        for (int m = 0; po && m < po->n; m++) { PObj *od = ps_obj(c, po->v[m]); if (od) bv_set(required_objects, od->id, so); }
                        if (layer >= 0) occupier[layer] = en;
                    } else if (ps_obj(c, en)) {
                        PObj *o = ps_obj(c, en);
                        bv_set(required_layers, o->layer, sl);
                        bv_set(required_objects, o->id, so);
                        if (o->layer >= 0) occupier[o->layer] = en;
                    } else if (ad_has(&c->aggregatesDict, en)) {
                        IVec *ao = dict_list(&c->aggregatesDict, en);
                        for (int m = 0; m < ao->n; m++) {
                            PObj *oi = ps_obj(c, ao->v[m]);
                            if (!oi) continue;
                            bv_set(required_layers, oi->layer, sl);
                            bv_set(required_objects, oi->id, so);
                            if (oi->layer >= 0) occupier[oi->layer] = en;
                        }
                    }
                }
                for (int l = 0; l + 1 < cell->n; l += 2) {
                    if (cell->v[l] != A.no) continue;
                    int nm = cell->v[l + 1];
                    uint32_t *nom = obj_mask(c, nm);
                    if (!nom) continue;
                    memset(no_object_layer_mask, 0, sizeof(uint32_t) * sl);
                    for (int m = 0; m < c->layerCount; m++)
                        if (bv_any_common(c->layerMasks[m], nom, so)) bv_set(no_object_layer_mask, m, sl);
                    bool disjoint = !bv_any_common(nom, required_objects, so);
                    bool covered = bv_bits_set_in(no_object_layer_mask, required_layers, sl);
                    if (covered && disjoint) {
                        SB ob, lb; sb_init(&ob, &c->tmp); sb_init(&lb, &c->tmp);
                        int cnt = 0;
                        for (int m = 0; m < c->layerCount; m++) if (occupier[m] >= 0) {
                            if (cnt++) { sb_puts(&ob, ", "); sb_puts(&lb, ", "); }
                            sb_puts(&ob, S(occupier[m])); sb_printf(&lb, "%d", m + 1);
                        }
                        ps_log_warning(c, rule->lineNumber, "You have specified that there should be NO %S but there is also a requirement that %S be there, which collectively occupies the same layers (Layers %s), so you can leave this out.",
                                       S(nm), sb_done(&ob), sb_done(&lb));
                        iv_splice(cell, l, 2);
                        l -= 2;
                    }
                }
            }
        }
    }
}

// =============================================================================================
// rule expansion passes

static void reverse_row(CRow *r)
{
    for (int i = 0, j = r->n - 1; i < j; i++, j--) { IVec t = r->c[i]; r->c[i] = r->c[j]; r->c[j] = t; }
}
// rewriteUpLeftRules
static void rewrite_up_left_rules(Rule *rule)
{
    for (int i = 0; i < rule->lhs.n; i++)
        for (int j = 0; j < rule->lhs.r[i].n; j++)
            if (rule->lhs.r[i].c[j].n > 1 && rule->lhs.r[i].c[j].v[1] == A.dots) return;
    if (rule->direction == A.up) rule->direction = A.down;
    else if (rule->direction == A.left) rule->direction = A.right;
    else return;
    for (int i = 0; i < rule->lhs.n; i++) {
        reverse_row(&rule->lhs.r[i]);
        if (i < rule->rhs.n) reverse_row(&rule->rhs.r[i]);
    }
}

// convertRelativeDirsToAbsolute / absolutifyRuleCell
static void absolutify_cell(int forward, IVec *cell)
{
    int rel[6] = { A.caret, A.vee, A.lt, A.gt, A.parallel, A.perpendicular };
    int tab[6];
    if (forward == A.right) { int t[6] = { A.up, A.down, A.left, A.right, A.horizontal_par, A.vertical_perp }; memcpy(tab, t, sizeof tab); }
    else if (forward == A.up) { int t[6] = { A.left, A.right, A.down, A.up, A.vertical_par, A.horizontal_perp }; memcpy(tab, t, sizeof tab); }
    else if (forward == A.down) { int t[6] = { A.right, A.left, A.up, A.down, A.vertical_par, A.horizontal_perp }; memcpy(tab, t, sizeof tab); }
    else if (forward == A.left) { int t[6] = { A.down, A.up, A.right, A.left, A.horizontal_par, A.vertical_perp }; memcpy(tab, t, sizeof tab); }
    else return;
    for (int i = 0; i < cell->n; i += 2)
        for (int k = 0; k < 6; k++) if (cell->v[i] == rel[k]) { cell->v[i] = tab[k]; break; }
}
static void convert_relative_dirs(Rule *rule)
{
    for (int i = 0; i < rule->lhs.n; i++) for (int j = 0; j < rule->lhs.r[i].n; j++) absolutify_cell(rule->direction, &rule->lhs.r[i].c[j]);
    for (int i = 0; i < rule->rhs.n; i++) for (int j = 0; j < rule->rhs.r[i].n; j++) absolutify_cell(rule->direction, &rule->rhs.r[i].c[j]);
}

// atomizeAggregates / atomizeCellAggregates
static void atomize_cell(PSComp *c, IVec *cell, int lineNumber)
{
    for (int i = 0; i + 1 < cell->n; i += 2) {
        int dir = cell->v[i], name = cell->v[i + 1];
        IVec *eq = dict_list(&c->aggregatesDict, name);
        if (eq) {
            if (dir == A.no)
                ps_log_error(c, lineNumber, "You cannot use 'no' to exclude the aggregate object %S (defined using 'AND'), only regular objects, or properties (objects defined using 'OR').  If you want to do this, you'll have to write it out yourself the long way.", S(name));
            if (eq->n == 0) continue;
            cell->v[i + 1] = eq->v[0];
            for (int j = 1; j < eq->n; j++) { iv_push(&c->tmp, cell, dir); iv_push(&c->tmp, cell, eq->v[j]); }
        }
    }
}
static void atomize_aggregates(PSComp *c, Rule *rule)
{
    for (int i = 0; i < rule->lhs.n; i++) for (int j = 0; j < rule->lhs.r[i].n; j++) atomize_cell(c, &rule->lhs.r[i].c[j], rule->lineNumber);
    for (int i = 0; i < rule->rhs.n; i++) for (int j = 0; j < rule->rhs.r[i].n; j++) atomize_cell(c, &rule->rhs.r[i].c[j], rule->lineNumber);
}

// rephraseSynonyms
static void rephrase_cell(PSComp *c, IVec *cell)
{
    for (int i = 1; i < cell->n; i += 2) { int s = syn_get(c, cell->v[i]); if (s >= 0) cell->v[i] = s; }
}
static void rephrase_synonyms(PSComp *c, Rule *rule)
{
    for (int i = 0; i < rule->lhs.n; i++) {
        for (int j = 0; j < rule->lhs.r[i].n; j++) {
            rephrase_cell(c, &rule->lhs.r[i].c[j]);
            if (i < rule->rhs.n && j < rule->rhs.r[i].n) rephrase_cell(c, &rule->rhs.r[i].c[j]);
        }
    }
}

// getMovings: first moving (name, dir) pair in cell; returns false if none
static bool first_moving(const IVec *cell, int *name, int *dir)
{
    int agg[5];
    for (int j = 0; j + 1 < cell->n; j += 2)
        if (dir_aggregate(cell->v[j], agg)) { *name = cell->v[j + 1]; *dir = cell->v[j]; return true; }
    return false;
}
static void concretize_moving_in_cell(IVec *cell, int amb, int name, int concrete)
{
    for (int j = 0; j + 1 < cell->n; j += 2) if (cell->v[j] == amb && cell->v[j + 1] == name) cell->v[j] = concrete;
}
static void concretize_moving_by_name(IVec *cell, int amb, int concrete)
{
    for (int j = 0; j < cell->n; j += 2) if (cell->v[j] == amb) cell->v[j] = concrete;
}

// concretizeMovingRule
static void concretize_moving_rule(PSComp *c, Rule *rule, PVec *out)
{
    PVec result = { 0, 0, NULL };
    pv_push(&c->tmp, &result, rule);
    bool modified = true;
    while (modified) {
        modified = false;
        for (int i = 0; i < result.n; i++) {
            Rule *cur = result.v[i];
            bool shouldremove = false;
            for (int j = 0; j < cur->lhs.n; j++) {
                for (int k = 0; k < cur->lhs.r[j].n; k++) {
                    int cand_name, amb;
                    if (!first_moving(&cur->lhs.r[j].c[k], &cand_name, &amb)) continue;
                    shouldremove = true; modified = true;
                    int dirs[5]; int nd = dir_aggregate(amb, dirs);
                    for (int l = 0; l < nd; l++) {
                        int cd = dirs[l];
                        Rule *nr = rule_clone(c, cur);
                        nr->movingReplacement = rd_clone(&c->tmp, cur->movingReplacement);
                        nr->aggregateDirReplacement = rd_clone(&c->tmp, cur->aggregateDirReplacement);
                        concretize_moving_in_cell(&nr->lhs.r[j].c[k], amb, cand_name, cd);
                        if (nr->rhs.n > 0) concretize_moving_in_cell(&nr->rhs.r[j].c[k], amb, cand_name, cd);
                        RepEnt *mr = rd_get(nr->movingReplacement, cand_name, amb);
                        if (!mr) {
                            mr = rd_add(&c->tmp, nr->movingReplacement, cand_name, amb);
                            mr->a[0] = cd; mr->a[1] = 1; mr->a[2] = amb; mr->a[3] = cand_name; mr->a[4] = j; mr->a[5] = k;
                        } else if (j != mr->a[4] || k != mr->a[5]) {
                            mr->a[1]++;
                        }
                        RepEnt *ar = rd_get(nr->aggregateDirReplacement, amb, 0);
                        if (!ar) { ar = rd_add(&c->tmp, nr->aggregateDirReplacement, amb, 0); ar->a[0] = cd; ar->a[1] = 1; ar->a[2] = amb; }
                        else ar->a[1]++;
                        pv_push(&c->tmp, &result, nr);
                    }
                }
            }
            if (shouldremove) { pv_splice(&result, i, 1); i--; }
        }
    }

    for (int i = 0; i < result.n; i++) {
        Rule *cur = result.v[i];
        if (!cur->movingReplacement) continue;
        for (int q = 0; q < cur->movingReplacement->n; q++) {
            RepEnt *e = &cur->movingReplacement->v[q];
            if (e->a[1] == 1)
                for (int j = 0; j < cur->rhs.n; j++)
                    for (int k = 0; k < cur->rhs.r[j].n; k++) concretize_moving_in_cell(&cur->rhs.r[j].c[k], e->a[2], e->a[3], e->a[0]);
        }
        // ambiguous_movement_names_dict
        RepDict amd = { 0, 0, NULL };
        for (int q = 0; q < cur->aggregateDirReplacement->n; q++) {
            RepEnt *e = &cur->aggregateDirReplacement->v[q];
            RepEnt *x = rd_get(&amd, e->a[2], 0);
            if (x || e->a[1] != 1) { if (!x) x = rd_add(&c->tmp, &amd, e->a[2], 0); x->a[0] = -1; }
            else { x = rd_add(&c->tmp, &amd, e->a[2], 0); x->a[0] = e->a[0]; }
        }
        for (int q = 0; q < amd.n; q++) {
            if (amd.v[q].a[0] < 0) continue;
            for (int j = 0; j < cur->rhs.n; j++)
                for (int k = 0; k < cur->rhs.r[j].n; k++) concretize_moving_by_name(&cur->rhs.r[j].c[k], amd.v[q].k1, amd.v[q].a[0]);
        }
    }

    int remain = -1;
    for (int i = 0; i < result.n && remain < 0; i++) {
        Rule *cur = result.v[i];
        cur->movingReplacement = NULL;
        for (int j = 0; j < cur->rhs.n && remain < 0; j++)
            for (int k = 0; k < cur->rhs.r[j].n && remain < 0; k++) {
                int nm, d;
                if (first_moving(&cur->rhs.r[j].c[k], &nm, &d)) remain = d;
            }
    }
    if (remain >= 0) {
        ps_log_error(c, rule->lineNumber, "This rule has an ambiguous movement on the right-hand side, \"%s\", that can't be inferred from the left-hand side.  (either for every ambiguous movement associated to an entity on the right there has to be a corresponding one on the left attached to the same entity, OR, if there's a single occurrence of a particular ambiguous movement on the left, all properties of the same movement attached to the same object on the right are assumed to be the same (or something like that)).", S(remain));
        c->invalid = true;
    }
    for (int i = 0; i < result.n; i++) pv_push(&c->tmp, out, result.v[i]);
}

// getPropertiesFromCell
static void properties_from_cell(PSComp *c, const IVec *cell, IVec *out)
{
    out->n = 0;
    for (int j = 0; j + 1 < cell->n; j += 2) {
        if (cell->v[j] == A.random) continue;
        if (ad_has(&c->propertiesDict, cell->v[j + 1])) iv_push(&c->tmp, out, cell->v[j + 1]);
    }
}
static void concretize_property_in_cell(IVec *cell, int property, int concrete)
{
    for (int j = 0; j + 1 < cell->n; j += 2) if (cell->v[j + 1] == property && cell->v[j] != A.random) cell->v[j + 1] = concrete;
}
// expandNoPrefixedProperties
static IVec expand_no_prefixed(PSComp *c, const IVec *cell)
{
    IVec e = { 0, 0, NULL };
    for (int i = 0; i + 1 < cell->n; i += 2) {
        int dir = cell->v[i], name = cell->v[i + 1];
        IVec *al = dir == A.no ? dict_list(&c->propertiesDict, name) : NULL;
        if (al) for (int j = 0; j < al->n; j++) { iv_push(&c->tmp, &e, dir); iv_push(&c->tmp, &e, al->v[j]); }
        else { iv_push(&c->tmp, &e, dir); iv_push(&c->tmp, &e, name); }
    }
    return e;
}

// concretizePropertyRule
static void concretize_property_rule(PSComp *c, Rule *rule, PVec *out)
{
    for (int i = 0; i < rule->lhs.n; i++)
        for (int j = 0; j < rule->lhs.r[i].n; j++) {
            rule->lhs.r[i].c[j] = expand_no_prefixed(c, &rule->lhs.r[i].c[j]);
            if (rule->rhs.n > 0 && i < rule->rhs.n && j < rule->rhs.r[i].n) rule->rhs.r[i].c[j] = expand_no_prefixed(c, &rule->rhs.r[i].c[j]);
        }

    IVec ambiguous = { 0, 0, NULL };
    IVec pl = { 0, 0, NULL }, pr = { 0, 0, NULL };
    for (int j = 0; j < rule->rhs.n; j++) {
        for (int k = 0; k < rule->rhs.r[j].n; k++) {
            properties_from_cell(c, &rule->lhs.r[j].c[k], &pl);
            properties_from_cell(c, &rule->rhs.r[j].c[k], &pr);
            for (int q = 0; q < pr.n; q++) if (!iv_has(&pl, pr.v[q]) && !iv_has(&ambiguous, pr.v[q])) iv_push(&c->tmp, &ambiguous, pr.v[q]);
        }
    }

    PVec result = { 0, 0, NULL };
    pv_push(&c->tmp, &result, rule);
    bool modified = true;
    IVec props = { 0, 0, NULL };
    while (modified) {
        modified = false;
        for (int i = 0; i < result.n; i++) {
            Rule *cur = result.v[i];
            bool shouldremove = false;
            for (int j = 0; j < cur->lhs.n && !shouldremove; j++) {
                for (int k = 0; k < cur->lhs.r[j].n && !shouldremove; k++) {
                    properties_from_cell(c, &cur->lhs.r[j].c[k], &props);
                    for (int pn = 0; pn < props.n; pn++) {
                        int property = props.v[pn];
                        bool has; single_layer(c, property, &has);
                        if (has && !iv_has(&ambiguous, property)) continue;
                        IVec *aliases = dict_list(&c->propertiesDict, property);
                        shouldremove = true; modified = true;
                        for (int l = 0; l < aliases->n; l++) {
                            int ct = aliases->v[l];
                            Rule *nr = rule_clone(c, cur);
                            nr->propertyReplacement = rd_clone(&c->tmp, cur->propertyReplacement);
                            concretize_property_in_cell(&nr->lhs.r[j].c[k], property, ct);
                            if (nr->rhs.n > 0) concretize_property_in_cell(&nr->rhs.r[j].c[k], property, ct);
                            RepEnt *e = rd_get(nr->propertyReplacement, property, 0);
                            if (!e) { e = rd_add(&c->tmp, nr->propertyReplacement, property, 0); e->a[0] = ct; e->a[1] = 1; }
                            else e->a[1]++;
                            pv_push(&c->tmp, &result, nr);
                        }
                        break;
                    }
                }
            }
            if (shouldremove) { pv_splice(&result, i, 1); i--; }
        }
    }

    for (int i = 0; i < result.n; i++) {
        Rule *cur = result.v[i];
        if (!cur->propertyReplacement) continue;
        for (int q = 0; q < cur->propertyReplacement->n; q++) {
            RepEnt *e = &cur->propertyReplacement->v[q];
            if (e->a[1] == 1)
                for (int j = 0; j < cur->rhs.n; j++)
                    for (int k = 0; k < cur->rhs.r[j].n; k++) concretize_property_in_cell(&cur->rhs.r[j].c[k], e->k1, e->a[0]);
        }
    }

    int remains = -1;
    for (int i = 0; i < result.n; i++) {
        Rule *cur = result.v[i];
        cur->propertyReplacement = NULL;
        for (int j = 0; j < cur->rhs.n; j++)
            for (int k = 0; k < cur->rhs.r[j].n; k++) {
                properties_from_cell(c, &cur->rhs.r[j].c[k], &props);
                for (int q = 0; q < props.n; q++) if (iv_has(&ambiguous, props.v[q])) remains = props.v[q];
            }
    }
    if (remains >= 0) {
        ps_log_error(c, rule->lineNumber, "This rule has a property on the right-hand side, \"%S\", that can't be inferred from the left-hand side.  (either for every property on the right there has to be a corresponding one on the left in the same cell, OR, if there's a single occurrence of a particular property name on the left, all properties of the same name on the right are assumed to be the same).", S(remains));
        return;
    }
    for (int i = 0; i < result.n; i++) pv_push(&c->tmp, out, result.v[i]);
}

// makeSpawnedObjectsStationary
// layer of each atom that names an object, NO_OBJ otherwise (one compact array instead of a
// dictionary lookup + PObj dereference per alias: properties can have dozens of aliases)
#define NO_OBJ (-0x7fffffff)
static int *atom_layers(PSComp *c)
{
    int n = c->atoms.n;
    int *t = ar_alloc_raw(&c->tmp, sizeof(int) * (n + 1));
    for (int a = 0; a < n; a++) { PObj *o = ps_obj(c, a); t[a] = o ? o->layer : NO_OBJ; }
    return t;
}
static void make_spawned_objects_stationary(PSComp *c, Rule *rule, const int *layerOf)
{
    if (rule->late) return;
    for (int j = 0; j < rule->rhs.n; j++) {
        for (int k = 0; k < rule->rhs.r[j].n; k++) {
            IVec *cell = &rule->rhs.r[j].c[k];
            IVec *lc = &rule->lhs.r[j].c[k];
            // getPossibleObjectsFromCell
            // (stack-backed: these are per cell temporaries; iv_push moves them to the arena if they outgrow it)
            int objsBuf[128], layersBuf[128];
            IVec objs = { 0, 128, objsBuf }, layers = { 0, 128, layersBuf };
            for (int q = 0; q + 1 < lc->n; q += 2) {
                int nm = lc->v[q + 1];
                if (layerOf[nm] != NO_OBJ) iv_push(&c->tmp, &objs, nm);
                else if (ad_has(&c->propertiesDict, nm)) {
                    IVec *al = dict_list(&c->propertiesDict, nm);
                    for (int z = 0; z < al->n; z++) iv_push(&c->tmp, &objs, al->v[z]);
                }
            }
            for (int q = 0; q < objs.n; q++) { int ly = layerOf[objs.v[q]]; if (ly != NO_OBJ) iv_push(&c->tmp, &layers, ly); }
            for (int l = 0; l + 1 < cell->n; l += 2) {
                if (cell->v[l] != A.empty) continue;
                int nm = cell->v[l + 1];
                if (ad_has(&c->propertiesDict, nm) || iv_has(&objs, nm)) continue;
                if (layerOf[nm] == NO_OBJ) continue;
                if (!iv_has(&layers, layerOf[nm])) cell->v[l] = A.stationary;
            }
        }
    }
}

static void expand_in_work_arena(PSComp *c, Rule *r, void (*fn)(PSComp *, Rule *, PVec *), PVec *out)
{
    Arena main = c->tmp;
    c->tmp = c->work;          // reused between rules (allocating a fresh 64KB block per rule is slow on device)
    PVec res = { 0, 0, NULL };
    fn(c, r, &res);
    c->work = c->tmp;
    c->tmp = main;
    for (int i = 0; i < res.n; i++) pv_push(&c->tmp, out, rule_clone(c, res.v[i]));
    ar_reset(&c->work);
}

// rulesToArray
static void rules_to_array(PSComp *c)
{
    PVec rules = { 0, 0, NULL };
    c->loopsLine.n = c->loopsDir.n = 0;
    for (int i = 0; i < c->rules.n; i++) {
        PRuleLine *rl = c->rules.v[i];
        int bracket;
        Rule *nr = process_rule_string(c, rl, &rules, &bracket);
        if (bracket) { iv_push(&c->tmp, &c->loopsLine, rl->lineNumber); iv_push(&c->tmp, &c->loopsDir, bracket); continue; }
        if (!nr) continue;
        pv_push(&c->tmp, &rules, nr);
    }
    MEMSTAGE("r2a: process_rule_string");
    check_superfluous_coincidences(c, &rules);
    MEMSTAGE("r2a: superfluous");

    // each generation of rules lives in its own arena, freed once the next one is built
    Arena mainA = c->tmp, a2 = { 0 }, a3 = { 0 };
    c->tmp = a2;
    PVec rules2 = { 0, 0, NULL };
    for (int i = 0; i < rules.n; i++) {
        Rule *r = rules.v[i];
        for (int j = 0; j < r->directions.n; j++) {
            Rule *m = rule_clone(c, r);
            m->direction = r->directions.v[j];
            pv_push(&c->tmp, &rules2, m);
        }
    }
    for (int i = 0; i < rules2.n; i++) {
        Rule *r = rules2.v[i];
        convert_relative_dirs(r);
        rewrite_up_left_rules(r);
        atomize_aggregates(c, r);
        if (c->invalid) { a2 = c->tmp; c->tmp = mainA; ar_free_all(&a2); return; }
        rephrase_synonyms(c, r);
    }
    MEMSTAGE("r2a: dirs/atomize/synonyms");
    a2 = c->tmp;
    c->tmp = a3;
    PVec rules3 = { 0, 0, NULL };
    for (int i = 0; i < rules2.n; i++) expand_in_work_arena(c, rules2.v[i], concretize_moving_rule, &rules3);
    a3 = c->tmp;
    ar_free_all(&a2);
    MEMSTAGE("r2a: concretize_moving");
    c->tmp = mainA;
    PVec rules4 = { 0, 0, NULL };
    for (int i = 0; i < rules3.n; i++) expand_in_work_arena(c, rules3.v[i], concretize_property_rule, &rules4);
    MEMSTAGE("r2a: concretize_property");
    ar_free_all(&a3);
    MEMSTAGE("r2a: free a3");
    const int *layerOf = atom_layers(c);
    for (int i = 0; i < rules4.n; i++) make_spawned_objects_stationary(c, rules4.v[i], layerOf);
    c->crules = rules4;
}

// =============================================================================================
// cacheRuleStringRep / removeDuplicateRules

// cacheRuleStringRep only feeds removeDuplicateRules, which compares the strings for equality.
// Equal strings <=> equal printed fields, so compare those fields structurally instead (saves
// building megabytes of strings for big games).
static bool rule_directed(const Rule *r)
{
    for (int i = 0; i < r->lhs.n; i++) if (r->lhs.r[i].n > 1) return true;
    return false;
}
static unsigned hs_hash(unsigned h, const HS *hs)
{
    for (int i = 0; i < hs->n; i++) {
        h = (h ^ 0x5bd1e995u) * 16777619u;
        for (int j = 0; j < hs->r[i].n; j++) {
            h = (h ^ 0x27d4eb2du) * 16777619u;
            const IVec *cell = &hs->r[i].c[j];
            for (int k = 0; k < cell->n; k++) h = (h ^ (unsigned)cell->v[k]) * 16777619u;
        }
    }
    return h;
}
static void cache_rule_string_rep(PSComp *c, Rule *rule)
{
    (void)c;
    unsigned h = 2166136261u ^ (unsigned)rule->lineNumber;
    h = (h ^ (unsigned)(rule->late | rule->rigid << 1 | rule->randomRule << 2)) * 16777619u;
    if (rule_directed(rule)) h = (h ^ (unsigned)rule->direction) * 16777619u;
    h = hs_hash(h, &rule->lhs);
    h = (h ^ 0xdeadbeefu) * 16777619u;
    h = hs_hash(h, &rule->rhs);
    rule->repHash = h;
}
static bool cells_print_equal(const IVec *a, const IVec *b)
{
    if (a->n != b->n) return false;
    for (int k = 0; k < a->n; k++) if (a->v[k] != b->v[k]) return false;
    return true;
}
static bool hs_print_equal(const HS *a, const HS *b)
{
    if (a->n != b->n) return false;
    for (int i = 0; i < a->n; i++) {
        if (a->r[i].n != b->r[i].n) return false;
        for (int j = 0; j < a->r[i].n; j++) if (!cells_print_equal(&a->r[i].c[j], &b->r[i].c[j])) return false;
    }
    return true;
}
static bool commands_equal(const Cmds *a, const Cmds *b)
{
    if (a == b) return true;
    if (a->n != b->n) return false;
    for (int i = 0; i < a->n; i++) {
        if (a->v[i].type != b->v[i].type) return false;
        if ((a->v[i].msg == NULL) != (b->v[i].msg == NULL)) return false;
        if (a->v[i].msg && strcmp(a->v[i].msg, b->v[i].msg) != 0) return false;
    }
    return true;
}
static bool rule_rep_equal(const Rule *a, const Rule *b)
{
    if (a->repHash != b->repHash || a->lineNumber != b->lineNumber) return false;
    if (a->late != b->late || a->rigid != b->rigid || a->randomRule != b->randomRule) return false;
    bool da = rule_directed(a), db = rule_directed(b);
    if (da != db || (da && a->direction != b->direction)) return false;
    return hs_print_equal(&a->lhs, &b->lhs) && hs_print_equal(&a->rhs, &b->rhs) && commands_equal(a->commands, b->commands);
}

static void remove_duplicate_rules(PSComp *c)
{
    PVec *rules = &c->crules;
    bool *keep = ar_alloc(&c->tmp, rules->n + 1);
    for (int i = rules->n - 1; i >= 0; i--) {
        Rule *r = rules->v[i];
        // record = kept rules later in the same contiguous group run
        bool found = false;
        for (int j = i + 1; j < rules->n; j++) {
            Rule *o = rules->v[j];
            if (o->groupNumber != r->groupNumber) break;
            if (keep[j] && rule_rep_equal(o, r)) { found = true; break; }
        }
        if (!found) keep[i] = true;
    }
    PVec out = { 0, 0, NULL };
    for (int i = 0; i < rules->n; i++) if (keep[i]) pv_push(&c->tmp, &out, rules->v[i]);
    c->crules = out;
}

// =============================================================================================
// rulesToMask

static const char *overlap_names(PSComp *c, const uint32_t *a, const uint32_t *b)
{
    SB s; sb_init(&s, &c->tmp);
    int cnt = 0;
    for (int i = 0; i < c->objectCount; i++)
        if (bv_get(a, i) && bv_get(b, i)) { if (cnt++) sb_puts(&s, ", "); sb_printf(&s, "%s NO %s", S(c->idDict.v[i]), S(c->idDict.v[i])); }
    return sb_done(&s);
}

// game data built here is read-only, so identical masks / replacements / pointer lists share
// storage (big games repeat them a lot). n is a byte count.
static unsigned blob_hash(const void *p, int n)
{
    unsigned h = 2166136261u ^ (unsigned)n;
    if ((n & 3) == 0 && ((uintptr_t)p & 3) == 0) {   // word at a time (blobs are masks / pointer arrays)
        const uint32_t *w = p;
        for (int i = 0; i < n / 4; i++) { h ^= w[i]; h *= 16777619u; h ^= h >> 15; }
        return h;
    }
    const unsigned char *b = p;
    for (int i = 0; i < n; i++) { h ^= b[i]; h *= 16777619u; }
    return h;
}
static void *intern_blob(PSComp *c, const void *m, int n)
{
    if (c->maskTabN * 2 >= c->maskTabCap) {
        int nc = c->maskTabCap ? c->maskTabCap * 2 : 1024;
        struct MaskSlot *nt = ar_alloc(&c->tmp, sizeof(struct MaskSlot) * nc);
        for (int i = 0; i < c->maskTabCap; i++) {
            if (!c->maskTab[i].p) continue;
            unsigned j = c->maskTab[i].hash & (unsigned)(nc - 1);
            while (nt[j].p) j = (j + 1) & (unsigned)(nc - 1);
            nt[j] = c->maskTab[i];
        }
        c->maskTab = nt; c->maskTabCap = nc;
    }
    // all-zero blobs are the most common by far: remember the interned one per size
    bool zero = n > 0 && n <= 4 * PS_ZERO_BLOB_WORDS && (n & 3) == 0;
    if (zero) {
        const uint32_t *w = m;
        for (int i = 0; i < n / 4; i++) if (w[i]) { zero = false; break; }
        if (zero && c->zeroBlob[n / 4]) return c->zeroBlob[n / 4];
    }
    unsigned h = blob_hash(m, n);
    // recently interned blobs first: consecutive rules (expansions of one source rule) repeat the
    // same masks, and both this slot and the blob are then likely still in the device's cache
    struct MaskSlot *fe = &c->maskFront[(h ^ (h >> 16)) & (PS_MASK_FRONT - 1)];
    if (fe->p && fe->hash == h && fe->len == n && memcmp(fe->p, m, n) == 0) return (void *)fe->p;
    unsigned j = h & (unsigned)(c->maskTabCap - 1);
    while (c->maskTab[j].p) {
        if (c->maskTab[j].hash == h && c->maskTab[j].len == n && memcmp(c->maskTab[j].p, m, n) == 0) {
            if (zero) c->zeroBlob[n / 4] = (void *)c->maskTab[j].p;
            *fe = c->maskTab[j];
            return (void *)c->maskTab[j].p;
        }
        j = (j + 1) & (unsigned)(c->maskTabCap - 1);
    }
    void *r = ar_alloc_raw(c->out, n);
    if (n) memcpy(r, m, n);
    c->maskTab[j].p = r; c->maskTab[j].hash = h; c->maskTab[j].len = n; c->maskTabN++;
    if (zero) c->zeroBlob[n / 4] = r;
    *fe = c->maskTab[j];
    return r;
}
static uint32_t *intern_mask(PSComp *c, const uint32_t *m, int n) { return intern_blob(c, m, (int)sizeof(uint32_t) * n); }

// what rulesToMask looks up per name, in one compact table (three dictionaries otherwise)
typedef struct { PObj *obj; uint32_t *mask; int single; bool hasSingle; } NameInfo;
static NameInfo *name_infos(PSComp *c)
{
    int n = c->atoms.n;
    NameInfo *t = ar_alloc_raw(&c->tmp, sizeof(NameInfo) * (n + 1));
    for (int a = 0; a < n; a++) {
        t[a].obj = ps_obj(c, a);
        t[a].mask = obj_mask(c, a);
        bool has;
        t[a].single = single_layer(c, a, &has);
        t[a].hasSingle = has;
    }
    return t;
}

static void rules_to_mask(PSComp *c)
{
    const NameInfo *ni = name_infos(c);
    int so = c->strideObj, sm = c->strideMov, L = c->layerCount;
    PVec *rules = &c->crules;
    int *layersUsed_l = ar_alloc(&c->tmp, sizeof(int) * (L + 1));
    int *layersUsed_r = ar_alloc(&c->tmp, sizeof(int) * (L + 1));
    int *layersUsedRand_r = ar_alloc(&c->tmp, sizeof(int) * (L + 1));
    // scratch masks, reused for every cell; results are interned into the game arena. On the
    // stack when they fit (device stack is fast TCM, the heap is write-through SDRAM).
    uint32_t stk[13 * 24];
    uint32_t *sp = so <= 24 && sm <= 24 ? stk : NULL;
#define SCRATCH(n) (sp ? (sp += 24, sp - 24) : bv_new(&c->tmp, (n)))
    uint32_t *objectsPresent = SCRATCH(so), *objectsMissing = SCRATCH(so);
    uint32_t *movementsPresent = SCRATCH(sm), *movementsMissing = SCRATCH(sm);
    uint32_t *objectlayers_l = SCRATCH(sm);
    uint32_t *objectsClear = SCRATCH(so), *objectsSet = SCRATCH(so);
    uint32_t *movementsClear = SCRATCH(sm), *movementsSet = SCRATCH(sm);
    uint32_t *objectlayers_r = SCRATCH(sm), *randomMask_r = SCRATCH(so);
    uint32_t *postMovementsLayerMask_r = SCRATCH(sm), *randomDirMask_r = SCRATCH(sm);
#undef SCRATCH
    uint32_t *zeroObj = bv_new(&c->tmp, so);
    uint32_t *rowMask_r = bv_new(&c->tmp, so), *rowMaskMov_r = bv_new(&c->tmp, sm), *ruleMask_r = bv_new(&c->tmp, so);
    PVec any = { 0, 0, NULL };
    for (int ri = 0; ri < rules->n; ri++) {
        Rule *rule = rules->v[ri];
        bool removed = false;
        rule->pats = ar_alloc(&c->tmp, sizeof(PSCellPattern *) * (rule->lhs.n + 1));
        for (int row = 0; row < rule->lhs.n && !removed; row++) {
            CRow *cl = &rule->lhs.r[row];
            CRow *cr = row < rule->rhs.n ? &rule->rhs.r[row] : NULL;
            rule->pats[row] = ar_alloc(&c->tmp, sizeof(PSCellPattern) * (cl->n + 1));
            for (int col = 0; col < cl->n && !removed; col++) {
                IVec *cell_l = &cl->c[col];
                PSCellPattern *pat = &rule->pats[row][col];
                for (int q = 0; q < L; q++) layersUsed_l[q] = -1;
                memset(objectsPresent, 0, 4 * so); memset(objectsMissing, 0, 4 * so);
                memset(movementsPresent, 0, 4 * sm); memset(movementsMissing, 0, 4 * sm);
                memset(objectlayers_l, 0, 4 * sm);
                any.n = 0;
                bool isEllipsis = false;
                for (int i = 0; i + 1 < cell_l->n; i += 2) {
                    int dir = cell_l->v[i], name = cell_l->v[i + 1];
                    if (dir == A.dots) {
                        if (cell_l->n != 2) {
                            ps_log_error(c, rule->lineNumber, "You can't have anything in with an ellipsis. Sorry.");
                            removed = true; break;
                        }
                        if (col == 0 || col == cl->n - 1) {
                            ps_log_error(c, rule->lineNumber, "There's no point in putting an ellipsis at the very start or the end of a rule");
                            rule->ellipsisAtEdge = true;
                        }
                        if (rule->rhs.n > 0) {
                            IVec *rc = &cr->c[col];
                            if (rc->n != 2 || rc->v[0] != A.dots)
                                ps_log_error(c, rule->lineNumber, "An ellipsis on the left must be matched by one in the corresponding place on the right.");
                        }
                        isEllipsis = true;
                        break;
                    }
                    if (dir == A.random || dir == A.randomdir) {
                        ps_log_error(c, rule->lineNumber, "%S cannot be matched on the left-hand side; it can only appear on the right", S(dir));
                        removed = true; break;
                    }
                    PObj *object = ni[name].obj;
                    uint32_t *objectMask = ni[name].mask;
                    bool has = true;
                    int layerIndex = object ? (object->layer < 0 ? 0 : object->layer) : (has = ni[name].hasSingle, ni[name].single);
                    if (!has || layerIndex < 0) {
                        ps_log_error(c, rule->lineNumber, "Oops! %S not assigned to a layer.", S(name));
                        continue;
                    }
                    if (dir == A.no) {
                        if (objectMask) bv_ior(objectsMissing, objectMask, so);
                    } else {
                        int existing = layersUsed_l[layerIndex];
                        if (existing >= 0) { rule->hasDiscard = true; rule->discard0 = name; rule->discard1 = existing; }
                        layersUsed_l[layerIndex] = name;
                        if (object) {
                            if (objectMask) bv_ior(objectsPresent, objectMask, so);
                            bv_shiftor(objectlayers_l, 0x1f, 5 * layerIndex, sm);
                        } else {
                            pv_push(&c->tmp, &any, objectMask ? objectMask : intern_mask(c, zeroObj, so));
                        }
                        if (dir == A.stationary) bv_shiftor(movementsMissing, 0x1f, 5 * layerIndex, sm);
                        else { int dm = dir_mask(dir); bv_shiftor(movementsPresent, dm < 0 ? 0 : (uint32_t)dm, 5 * layerIndex, sm); }
                    }
                }
                if (removed) break;
                if (isEllipsis) {
                    memset(objectsPresent, 0, sizeof(uint32_t) * so);
                    pat->objectsPresent = intern_mask(c, objectsPresent, so); pat->objectsMissing = intern_mask(c, objectsMissing, so);
                    pat->movementsPresent = intern_mask(c, movementsPresent, sm); pat->movementsMissing = intern_mask(c, movementsMissing, sm);
                    pat->isEllipsis = true;
                    pat->anyCount = 0; pat->anyObjectsPresent = NULL; pat->replacement = NULL;
                    continue;
                }
                pat->objectsPresent = intern_mask(c, objectsPresent, so); pat->objectsMissing = intern_mask(c, objectsMissing, so);
                pat->movementsPresent = intern_mask(c, movementsPresent, sm); pat->movementsMissing = intern_mask(c, movementsMissing, sm);
                pat->anyCount = any.n;
                pat->anyObjectsPresent = any.n ? intern_blob(c, any.v, (int)sizeof(uint32_t *) * any.n) : NULL;

                if (!bv_iszero(objectsPresent, so) && bv_any_common(objectsPresent, objectsMissing, so)) {
                    ps_log_warning(c, rule->lineNumber, "This rule has something amounting to \"%S\" on the left-hand-side,which can never match, so the rule is getting removed during compilation.",
                                   overlap_names(c, objectsPresent, objectsMissing));
                    removed = true; break;
                }
                if (rule->rhs.n == 0) continue;
                IVec *cell_r = &cr->c[col];
                if (cell_r->n > 0 && cell_r->v[0] == A.dots && !(cell_l->n > 0 && cell_l->v[0] == A.dots))
                    ps_log_error(c, rule->lineNumber, "An ellipsis on the right must be matched by one in the corresponding place on the left.");
                for (int i = 0; i < cell_r->n; i += 2)
                    if (cell_r->v[i] == A.dots && cell_r->n != 2) ps_log_error(c, rule->lineNumber, "You can't have anything in with an ellipsis. Sorry.");

                for (int q = 0; q < L; q++) { layersUsed_r[q] = -1; layersUsedRand_r[q] = -1; }
                memset(objectsClear, 0, 4 * so); memset(objectsSet, 0, 4 * so); memset(randomMask_r, 0, 4 * so);
                memset(movementsClear, 0, 4 * sm); memset(movementsSet, 0, 4 * sm); memset(objectlayers_r, 0, 4 * sm);
                memset(postMovementsLayerMask_r, 0, 4 * sm); memset(randomDirMask_r, 0, 4 * sm);
                for (int i = 0; i + 1 < cell_r->n; i += 2) {
                    int dir = cell_r->v[i], name = cell_r->v[i + 1];
                    if (dir == A.dots) break;
                    if (dir == A.random) {
                        uint32_t *m = obj_mask(c, name);
                        if (m) {
                            bv_ior(randomMask_r, m, so);
                            IVec single = { 1, 1, &name };
                            IVec *values = ad_has(&c->propertiesDict, name) ? dict_list(&c->propertiesDict, name) : &single;
                            if (values->n == 1)
                                ps_log_warning(c, rule->lineNumber, "In this rule you're asking me to spawn a random %S for you, but that's already a concrete single object. You wanna be using random with properties (things defined in terms of OR in the legend) so there's some things to select between.", S(name));
                            for (int q = 0; q < values->n; q++) {
                                int sub = values->v[q];
                                PObj *so2 = ps_obj(c, sub);
                                if (!so2 || so2->layer < 0) continue;
                                int li = so2->layer;
                                int existing = layersUsed_r[li];
                                if (existing >= 0 && strcmp(js_upper(&c->tmp, S(sub)), js_upper(&c->tmp, S(existing))) != 0)
                                    ps_log_warning(c, rule->lineNumber, "This rule may try to spawn a %S with random, but also requires a %S be here, which is on the same layer - they shouldn't be able to coexist!", S(sub), S(existing));
                                layersUsedRand_r[li] = sub;
                            }
                        } else {
                            ps_log_error(c, rule->lineNumber, "You want to spawn a random \"%S\", but I don't know how to do that", S(name));
                        }
                        continue;
                    }
                    PObj *object = ni[name].obj;
                    uint32_t *objectMask = ni[name].mask;
                    bool has = true;
                    int layerIndex = object ? (object->layer < 0 ? 0 : object->layer) : (has = ni[name].hasSingle, ni[name].single);
                    if (!has || layerIndex < 0) {
                        ps_log_error(c, rule->lineNumber, "Oops! %S not assigned to a layer.", S(name));
                        continue;
                    }
                    if (dir == A.no) {
                        if (objectMask) bv_ior(objectsClear, objectMask, so);
                    } else {
                        int existing = layersUsed_r[layerIndex] >= 0 ? layersUsed_r[layerIndex] : layersUsedRand_r[layerIndex];
                        if (existing >= 0 && !rule->hasDiscard)
                            ps_log_error(c, rule->lineNumber, "Rule matches object types that can't overlap: \"%S\" and \"%S\".", S(name), S(existing));
                        layersUsed_r[layerIndex] = name;
                        if (dir != A.empty) bv_shiftor(postMovementsLayerMask_r, 0x1f, 5 * layerIndex, sm);
                        if (object) {
                            bv_set(objectsSet, object->id, so);
                            bv_ior(objectsClear, c->layerMasks[layerIndex], so);
                            bv_shiftor(objectlayers_r, 0x1f, 5 * layerIndex, sm);
                        }
                        if (dir == A.stationary) bv_shiftor(movementsClear, 0x1f, 5 * layerIndex, sm);
                        else if (dir == A.randomdir) bv_shiftor(randomDirMask_r, 5, 5 * layerIndex, sm);
                        else { int dm = dir_mask(dir); bv_shiftor(movementsSet, dm < 0 ? 0 : (uint32_t)dm, 5 * layerIndex, sm); }
                    }
                }
                if (!bv_bits_set_in(objectsPresent, objectsSet, so)) bv_ior(objectsClear, objectsPresent, so);
                if (!bv_bits_set_in(movementsPresent, movementsSet, sm)) bv_ior(movementsClear, movementsPresent, sm);
                for (int li = 0; li < L; li++) {
                    if (layersUsed_l[li] >= 0 && layersUsed_r[li] < 0) {
                        bv_ior(objectsClear, c->layerMasks[li], so);
                        bv_shiftor(postMovementsLayerMask_r, 0x1f, 5 * li, sm);
                    }
                }
                bv_iclear(objectlayers_l, objectlayers_r, sm);
                bv_ior(postMovementsLayerMask_r, objectlayers_l, sm);
                bool changes = !bv_iszero(objectsClear, so) || !bv_iszero(objectsSet, so) || !bv_iszero(movementsClear, sm) ||
                               !bv_iszero(movementsSet, sm) || !bv_iszero(postMovementsLayerMask_r, sm) ||
                               !bv_iszero(randomMask_r, so) || !bv_iszero(randomDirMask_r, sm);
                if (changes) {
                    PSReplacement rpv, *rp = &rpv;
                    memset(rp, 0, sizeof rpv);
                    rp->objectsClear = intern_mask(c, objectsClear, so); rp->objectsSet = intern_mask(c, objectsSet, so);
                    rp->movementsClear = intern_mask(c, movementsClear, sm); rp->movementsSet = intern_mask(c, movementsSet, sm);
                    rp->movementsLayerMask = intern_mask(c, postMovementsLayerMask_r, sm);
                    rp->randomEntityMask = intern_mask(c, randomMask_r, so); rp->randomDirMask = intern_mask(c, randomDirMask_r, sm);
                    pat->replacement = intern_blob(c, rp, sizeof rpv);
                }
            }
        }
        if (removed || rule->ellipsisAtEdge) { pv_splice(rules, ri, 1); ri--; continue; }
        int nr = rule->lhs.n;
        rule->cellsI = ar_alloc_raw(&c->tmp, sizeof(PSCellPattern *) * (nr + 1));
        rule->rowMaskI = ar_alloc_raw(&c->tmp, sizeof(uint32_t *) * (nr + 1));
        rule->rowMaskMovI = ar_alloc_raw(&c->tmp, sizeof(uint32_t *) * (nr + 1));
        memset(ruleMask_r, 0, 4 * so);
        for (int j = 0; j < nr; j++) {
            int len = rule->lhs.r[j].n;
            rule->cellsI[j] = intern_blob(c, rule->pats[j], (int)sizeof(PSCellPattern) * (len > 0 ? len : 1));
            memset(rowMask_r, 0, 4 * so); memset(rowMaskMov_r, 0, 4 * sm);
            for (int k = 0; k < len; k++) {
                if (rule->pats[j][k].isEllipsis) continue;
                bv_ior(rowMask_r, rule->pats[j][k].objectsPresent, so);
                bv_ior(rowMaskMov_r, rule->pats[j][k].movementsPresent, sm);
            }
            rule->rowMaskI[j] = intern_mask(c, rowMask_r, so);
            rule->rowMaskMovI[j] = intern_mask(c, rowMaskMov_r, sm);
            bv_ior(ruleMask_r, rowMask_r, so);
        }
        rule->ruleMaskI = intern_mask(c, ruleMask_r, so);
    }
}

// =============================================================================================
// arrangeRulesByGroupNumber / ruleGroupDiscardOverlappingTest / collapseRules

static void discard_overlapping_test(PSComp *c, PVec *group)
{
    if (group->n == 0) return;
    IVec d0 = { 0, 0, NULL }, d1 = { 0, 0, NULL };
    for (int i = 0; i < group->n; i++) {
        Rule *rule = group->v[i];
        if (!rule->hasDiscard) continue;
        bool beforesame = i == 0 ? false : ((Rule *)group->v[i - 1])->lineNumber == rule->lineNumber;
        bool aftersame = i == group->n - 1 ? false : ((Rule *)group->v[i + 1])->lineNumber == rule->lineNumber;
        pv_splice(group, i, 1);
        bool found = false;
        for (int j = 0; j < d0.n; j++) if (d0.v[j] == rule->discard0 && d1.v[j] == rule->discard1) { found = true; break; }
        if (!found) { iv_push(&c->tmp, &d0, rule->discard0); iv_push(&c->tmp, &d1, rule->discard1); }
        if (!(beforesame || aftersame) || group->n == 0) {
            SB p; sb_init(&p, &c->tmp);
            if (d0.n > 1) {
                sb_puts(&p, " (ditto for ");
                for (int j = 1; j < d0.n; j++) {
                    if (j > 1) { sb_puts(&p, ", "); if (j == d0.n - 1) sb_puts(&p, "and "); }
                    sb_printf(&p, "%S/%S", S(d0.v[j]), S(d1.v[j]));
                    if (j == 3 && d0.n > 4) { sb_puts(&p, " etc."); break; }
                }
                sb_puts(&p, ")");
            }
            ps_log_error(c, rule->lineNumber, "%S and %S can never overlap%s. This rule requires that to happen, so it impossible it will ever trigger.",
                         S(d0.v[0]), S(d1.v[0]), sb_done(&p));
        }
        i--;
    }
}

static int collapse_groups(PSComp *c, PVec *rules, bool late, PSRuleGroup **outGroups)
{
    // group numbers ascending (JS object integer-key order), rules in insertion order
    // group key per rule in one compact array (-1 = other lateness): scanning the Rule structs
    // once per group is a cache miss per rule on device
    int *key = ar_alloc_raw(&c->tmp, sizeof(int) * (rules->n + 1));
    for (int i = 0; i < rules->n; i++) { Rule *r = rules->v[i]; key[i] = r->late == late ? r->groupNumber : -1; }
    IVec gnums = { 0, 0, NULL };
    for (int i = 0; i < rules->n; i++)
        if (key[i] >= 0 && (gnums.n == 0 || gnums.v[gnums.n - 1] != key[i]) && !iv_has(&gnums, key[i])) iv_push(&c->tmp, &gnums, key[i]);
    for (int i = 1; i < gnums.n; i++) { int x = gnums.v[i], j = i - 1; while (j >= 0 && gnums.v[j] > x) { gnums.v[j + 1] = gnums.v[j]; j--; } gnums.v[j + 1] = x; }
    PSRuleGroup *groups = ar_alloc(c->out, sizeof(PSRuleGroup) * (gnums.n + 1));
    int ng = 0;
    for (int gi = 0; gi < gnums.n; gi++) {
        PVec grp = { 0, 0, NULL };
        for (int i = 0; i < rules->n; i++) if (key[i] == gnums.v[gi]) pv_push(&c->tmp, &grp, rules->v[i]);
        discard_overlapping_test(c, &grp);
        if (grp.n == 0) continue;
        PSRuleGroup *G = &groups[ng++];
        G->count = grp.n;
        G->rules = ar_alloc(c->out, sizeof(PSRule) * grp.n);
        for (int i = 0; i < grp.n; i++) {
            Rule *o = grp.v[i];
            PSRule *R = &G->rules[i];
            R->direction = dir_mask(o->direction);
            R->hasReplacements = o->rhs.n > 0;
            R->lineNumber = o->lineNumber;
            R->rowCount = o->lhs.n;
            R->rows = ar_alloc(c->out, sizeof(PSCellRow) * (o->lhs.n + 1));
            R->cellRowMasks = ar_alloc(c->out, sizeof(uint32_t *) * (o->lhs.n + 1));
            R->cellRowMasks_Movements = ar_alloc(c->out, sizeof(uint32_t *) * (o->lhs.n + 1));
            for (int j = 0; j < o->lhs.n; j++) {
                PSCellRow *row = &R->rows[j];
                row->len = o->lhs.r[j].n;
                row->cells = o->cellsI[j];
                int ell = 0;
                for (int k = 0; k < row->len; k++) {
                    if (row->cells[k].isEllipsis) {
                        ell++;
                        if (ell > 2) ps_log_error(c, o->lineNumber, "You can't use more than two ellipses in a single cell match pattern.");
                        else if (k > 0 && row->cells[k - 1].isEllipsis)
                            ps_log_warning(c, o->lineNumber, "Why would you go and have two ellipses in a row like that? It's exactly the same as just having a single ellipsis, right?");
                    }
                }
                row->ellipsisCount = ell;
                R->cellRowMasks[j] = o->rowMaskI[j]; R->cellRowMasks_Movements[j] = o->rowMaskMovI[j];
            }
            R->ruleMask = o->ruleMaskI;
            R->groupNumber = o->groupNumber;
            R->rigid = o->rigid;
            R->isRandom = o->randomRule;
            R->commandCount = o->commands->n;
            R->commands = ar_alloc(c->out, sizeof(PSCommand) * (o->commands->n + 1));
            for (int k = 0; k < o->commands->n; k++) {
                R->commands[k].type = o->commands->v[k].type;
                R->commands[k].message = o->commands->v[k].msg ? ar_strdup(c->out, o->commands->v[k].msg) : NULL;
            }
        }
    }
    *outGroups = groups;
    return ng;
}

// generateRigidGroupList
static void generate_rigid_group_list(PSComp *c)
{
    PSGame *g = c->g;
    int maxg = 0;
    for (int i = 0; i < g->ruleGroupCount; i++)
        for (int j = 0; j < g->rules[i].count; j++) if (g->rules[i].rules[j].groupNumber > maxg) maxg = g->rules[i].rules[j].groupNumber;
    for (int i = 0; i < g->lateRuleGroupCount; i++)
        for (int j = 0; j < g->lateRules[i].count; j++) if (g->lateRules[i].rules[j].groupNumber > maxg) maxg = g->lateRules[i].rules[j].groupNumber;
    g->groupNumberCount = maxg + 1;
    g->groupNumber_to_RigidGroupIndex = ar_alloc(c->out, sizeof(int) * (maxg + 1));
    for (int i = 0; i <= maxg; i++) g->groupNumber_to_RigidGroupIndex[i] = -1;
    g->rigidGroupIndex_to_GroupIndex = ar_alloc(c->out, sizeof(int) * (g->ruleGroupCount + 1));
    g->rigidGroupCount = 0;
    for (int i = 0; i < g->ruleGroupCount; i++) {
        bool found = false;
        for (int j = 0; j < g->rules[i].count; j++) if (g->rules[i].rules[j].rigid) found = true;
        if (found) {
            int gn = g->rules[i].rules[0].groupNumber;
            g->groupNumber_to_RigidGroupIndex[gn] = g->rigidGroupCount;
            g->rigidGroupIndex_to_GroupIndex[g->rigidGroupCount++] = i;
        }
    }
    if (g->rigidGroupCount > 30) {
        int gi = g->rigidGroupIndex_to_GroupIndex[30];
        ps_log_error(c, g->rules[gi].rules[0].lineNumber, "There can't be more than 30 rigid groups (rule groups containing rigid members).");
    }
    g->rigid = g->rigidGroupCount > 0;
}

// =============================================================================================
// processWinConditions / checkObjectsAreLayered / twiddleMetaData

static void process_win_conditions(PSComp *c)
{
    PSGame *g = c->g;
    g->winconditions = ar_alloc(c->out, sizeof(PSWinCondition) * (c->winconditions.n + 1));
    g->winconditionCount = 0;
    for (int i = 0; i < c->winconditions.n; i++) {
        PWin *w = c->winconditions.v[i];
        int num = 0;
        if (w->n > 0 && strcmp(w->w[0], "no") == 0) num = -1;
        else if (w->n > 0 && strcmp(w->w[0], "all") == 0) num = 1;
        int lineNumber = w->lineNumber;
        int total = w->n + 1;   // JS array includes the line number
        if (total <= 2) {
            ps_log_error(c, lineNumber, "Win conditions is badly formatted - needs to look something like \"No Fruit\", \"All Target On Crate\", \"Some Fruit\", \"Some Gold on Chest\", \"No Gold on Chest\", or the like.");
            continue;
        }
        int n1 = atom(&c->atoms, w->w[1]);
        int n2 = total == 5 ? atom(&c->atoms, w->w[3]) : A.all;
        uint32_t *m[2] = { NULL, NULL }; bool ag[2] = { false, false };
        int ns[2] = { n1, n2 };
        bool bad = false;
        for (int k = 0; k < 2; k++) {
            if (ad_has(&c->objectMasks, ns[k])) m[k] = obj_mask(c, ns[k]);
            else if (ad_has(&c->aggregateMasks, ns[k])) { m[k] = ad_get(&c->aggregateMasks, ns[k]); ag[k] = true; }
            else {
                ps_log_error(c, lineNumber, "Unwelcome term \"%s\" found in win condition. I don't know what I'm supposed to do with this. ", S(ns[k]));
                bad = true;
            }
        }
        if (bad) continue;
        PSWinCondition *wc = &g->winconditions[g->winconditionCount++];
        wc->quantifier = num; wc->filter1 = m[0]; wc->filter2 = m[1];
        wc->lineNumber = lineNumber; wc->aggr1 = ag[0]; wc->aggr2 = ag[1];
    }
}

static void check_objects_are_layered(PSComp *c)
{
    int nk; int *keys = ad_keys(c, &c->objects, &nk);
    for (int ki = 0; ki < nk; ki++) {
        bool found = false;
        for (int i = 0; i < c->collisionLayers.n && !found; i++) if (iv_has(c->collisionLayers.v[i], keys[ki])) found = true;
        if (!found) ps_log_error(c, ps_obj(c, keys[ki])->lineNumber, "Object \"%S\" has been defined, but not assigned to a layer.", S(keys[ki]));
    }
}

static bool js_isfinite_str(const char *s) { double v = js_number(s); return !isnan(v) && !isinf(v); }
static bool js_isint_str(const char *s) { double x = js_parsefloat(s); return !isnan(js_number(s)) && !isnan(x) && (double)(int32_t)(int64_t)x == x; }

static double get_int_checked_positive(PSComp *c, const char *s, int line)
{
    if (!js_isfinite_str(s) || !js_isint_str(s)) {
        ps_log_warning(c, line, "Wasn't able to make sense of \"%s\" as a (whole number) dimension.", s);
        return NAN;
    }
    double r = js_parseint(s);
    if (isnan(r)) ps_log_warning(c, line, "Wasn't able to make sense of \"%s\" as a dimension.", s);
    if (r <= 0) ps_log_warning(c, line, "The dimension given to me (you gave \"%s\") is baad - it should be greater than 0.", s);
    return r;
}
// getCoords; returns false when invalid
static bool get_coords(PSComp *c, const char *val, int line, int out[2])
{
    const char *x = strchr(val, 'x');
    if (!x || strchr(x + 1, 'x')) { ps_log_warning(c, line, "Dimensions must be of the form AxB."); return false; }
    char *a = ar_strndup(&c->tmp, val, x - val), *b = ar_strdup(&c->tmp, x + 1);
    double i0 = get_int_checked_positive(c, a, line), i1 = get_int_checked_positive(c, b, line);
    if (!js_isfinite_str(a) || !js_isfinite_str(b) || isnan(i0) || isnan(i1)) {
        ps_log_warning(c, line, "Couldn't understand the dimensions given to me (you gave \"%s\") - should be of the form AxB.", val);
        return false;
    }
    if (i0 <= 0 || i1 <= 0) {
        ps_log_warning(c, line, "The dimensions given to me (you gave \"%s\") are baad - they should be > 0.", val);
        return false;
    }
    out[0] = (int)i0; out[1] = (int)i1;
    return true;
}

static const char *meta(PSComp *c, const char *key)
{
    int a = atom_find(&c->atoms, key);
    return a >= 0 ? ad_get(&c->newmeta, a) : NULL;
}
static int meta_line(PSComp *c, const char *key)
{
    int a = atom_find(&c->atoms, key);
    return a >= 0 && ad_has(&c->metadata_lines, a) ? (int)(intptr_t)ad_get(&c->metadata_lines, a) : -1;
}

static void twiddle_metadata(PSComp *c)
{
    PSMetadata *M = &c->g->metadata;
    ad_init(&c->newmeta, &c->tmp);
    for (int i = 0; i < c->metadata.n; i++) {
        PMeta *m = c->metadata.v[i];
        ad_set(&c->newmeta, atom(&c->atoms, m->key), m->val);
    }
    const char *v;
    if ((v = meta(c, "flickscreen"))) M->has_flickscreen = get_coords(c, v, meta_line(c, "flickscreen"), M->flickscreen);
    if ((v = meta(c, "zoomscreen"))) M->has_zoomscreen = get_coords(c, v, meta_line(c, "zoomscreen"), M->zoomscreen);
}

// formatHomePage + copying metadata into PSMetadata
static void format_home_page(PSComp *c)
{
    PSMetadata *M = &c->g->metadata;
    const char *bg = meta(c, "background_color"), *fg = meta(c, "text_color");
    const char *bgc = bg ? color_to_hex(c, bg) : "#000000";
    const char *fgc = fg ? color_to_hex(c, fg) : "#FFFFFF";
    if (!is_color(c, fgc)) {
        ps_log_error(c, meta_line(c, "text_color"), "text_color in incorrect format - found %s, but I expect a color name (like 'pink') or hex-formatted color (like '#1412FA').  Defaulting to white.", fgc);
        fgc = "#FFFFFF";
    }
    if (!is_color(c, bgc)) {
        ps_log_error(c, meta_line(c, "background_color"), "background_color in incorrect format - found %s, but I expect a color name (like 'pink') or hex-formatted color (like '#1412FA').  Defaulting to black.", bgc);
        bgc = "#000000";
    }
    M->background_color = color_rgb(bgc);
    M->text_color = color_rgb(fgc);
    if (M->background_color == PS_COLOR_TRANSPARENT) M->background_color = 0x000000;
    if (M->text_color == PS_COLOR_TRANSPARENT) M->text_color = 0xFFFFFF;
    M->background_color_str = bg ? ar_strdup(c->out, bg) : NULL;
    M->text_color_str = fg ? ar_strdup(c->out, fg) : NULL;
    const char *hp = meta(c, "homepage");
    if (hp) {
        char *h = ar_strdup(&c->tmp, hp), *f;
        if ((f = strstr(h, "http://"))) memmove(f, f + 7, strlen(f + 7) + 1);
        if ((f = strstr(h, "https://"))) memmove(f, f + 8, strlen(f + 8) + 1);
        M->homepage = ar_strdup(c->out, h);
    }
    M->title = ar_strdup(c->out, meta(c, "title"));
    M->author = ar_strdup(c->out, meta(c, "author"));
    M->color_palette = ar_strdup(c->out, meta(c, "color_palette"));
    M->again_interval = meta(c, "again_interval") ? js_number(meta(c, "again_interval")) : 0.15;
    M->key_repeat_interval = meta(c, "key_repeat_interval") ? js_number(meta(c, "key_repeat_interval")) : 0.15;
    M->realtime_interval = meta(c, "realtime_interval") ? js_number(meta(c, "realtime_interval")) : 0;
    M->noaction = meta(c, "noaction") != NULL;
    M->noundo = meta(c, "noundo") != NULL;
    M->norestart = meta(c, "norestart") != NULL;
    M->norepeat_action = meta(c, "norepeat_action") != NULL;
    M->require_player_movement = meta(c, "require_player_movement") != NULL;
    M->run_rules_on_level_start = meta(c, "run_rules_on_level_start") != NULL;
    M->throttle_movement = meta(c, "throttle_movement") != NULL;
    M->verbose_logging = meta(c, "verbose_logging") != NULL;
    M->scanline = meta(c, "scanline") != NULL;
}

// =============================================================================================
// generateLoopPoints / calculateLoopPoints

static int *calculate_loop_points(PSComp *c, PSRuleGroup *groups, int ng)
{
    int *lp = ar_alloc(c->out, sizeof(int) * (ng + 1));
    for (int i = 0; i <= ng; i++) lp[i] = -1;
    for (int j = 0; j + 1 < c->loopsLine.n; j += 2) {
        int ls = c->loopsLine.v[j], le = c->loopsLine.v[j + 1];
        int init = -1;
        for (int gi = 0; gi < ng; gi++) {
            int first = groups[gi].rules[0].lineNumber, last = groups[gi].rules[groups[gi].count - 1].lineNumber;
            if (first <= ls && ls <= last)
                ps_log_error(c, ls, "Found a loop point in the middle of a rule. You probably don't want to do this, right?");
            else if (first <= le && le <= last)
                ps_log_error(c, ls, "Found a loop point in the middle of a rule. You probably don't want to do this, right?");
            bool in_loop = ls <= first && first <= le;
            bool after = le < first;
            if (after) break;
            if (in_loop) {
                if (init == -1) init = gi;
                if (gi > 0 && lp[gi - 1] != -1 && lp[gi - 1] == init) lp[gi - 1] = -1;
                lp[gi] = init;
            }
        }
    }
    return lp;
}
static void generate_loop_points(PSComp *c)
{
    for (int i = 0; i < c->loopsLine.n; i++) {
        if (i % 2 == 0) { if (c->loopsDir.v[i] == -1) ps_log_error(c, c->loopsLine.v[i], "Found an ENDLOOP, but I'm not in a loop?"); }
        else { if (c->loopsDir.v[i] == 1) ps_log_error(c, c->loopsLine.v[i], "Found a STARTLOOP, but I'm already inside a loop? (Puzzlescript can't nest loops, FWIW)."); }
    }
    if (c->loopsLine.n % 2 != 0) {
        int ll = c->loopsLine.v[c->loopsLine.n - 1];
        ps_log_error(c, ll, "Yo I found a STARTLOOP without a corresponding ENDLOOP.");
        iv_push(&c->tmp, &c->loopsLine, ll); iv_push(&c->tmp, &c->loopsDir, -1);
    }
    c->g->loopPoint = calculate_loop_points(c, c->g->rules, c->g->ruleGroupCount);
    c->g->lateLoopPoint = calculate_loop_points(c, c->g->lateRules, c->g->lateRuleGroupCount);
}

// =============================================================================================
// generateSoundData

static int64_t parse_seed(const char *s)
{
    int64_t v = 0;
    for (; *s >= '0' && *s <= '9'; s++) {
        if (v > (INT64_MAX - 9) / 10) return INT64_MAX;
        v = v * 10 + (*s - '0');
    }
    return v;
}
static int sound_dir_mask(const char *d)
{
    static const char *const n[] = { "up", "down", "left", "right", "horizontal", "vertical", "orthogonal", "___action____" };
    static const int m[] = { 1, 2, 4, 8, 12, 3, 15, 16 };
    for (int i = 0; i < 8; i++) if (strcmp(n[i], d) == 0) return m[i];
    return -1;
}
static const char *const sfx_event_names[PS_SFX_EVENT_COUNT] = {
    "undo", "restart", "titlescreen", "startgame", "cancel", "endgame", "startlevel", "endlevel", "showmessage",
    "closemessage", "sfx0", "sfx1", "sfx2", "sfx3", "sfx4", "sfx5", "sfx6", "sfx7", "sfx8", "sfx9", "sfx10"
};

static void generate_sound_data(PSComp *c)
{
    PSGame *g = c->g;
    int L = c->layerCount, so = c->strideObj, sm = c->strideMov;
    int ns = c->sounds.n;
    g->sfx_CreationMasks = ar_alloc(c->out, sizeof(PSSfxObjectMask) * (ns + 1));
    g->sfx_DestructionMasks = ar_alloc(c->out, sizeof(PSSfxObjectMask) * (ns + 1));
    PVec failures = { 0, 0, NULL };
    PVec *moves = ar_alloc(&c->tmp, sizeof(PVec) * (L + 1));
    bool declared[PS_SFX_EVENT_COUNT] = { false };
    for (int i = 0; i < ns; i++) {
        PSound *snd = c->sounds.v[i];
        int len = snd->n + 1;    // JS row includes the line number
        int lineNumber = snd->lineNumber;
        if (len <= 1) continue;
        if (len == 2) { ps_log_warning(c, lineNumber, "incorrect sound declaration."); continue; }
        const char *v0 = snd->t[0].text, *v1 = snd->t[1].text;
        int t0 = snd->t[0].type;
        PSndTok *seedt = &snd->t[snd->n - 1];
        if (seedt->type != SND_SOUND) ps_log_error(c, lineNumber, "Expecting sfx data, instead found \"%s\".", seedt->text);
        int64_t seed = parse_seed(seedt->text);
        if (t0 == SND_SOUNDEVENT) {
            if (len > 4) ps_log_error(c, lineNumber, "too much stuff to define a sound event.");
            else if (len > 3) ps_log_warning(c, lineNumber, "too much stuff to define a sound event.");
            for (int k = 0; k < PS_SFX_EVENT_COUNT; k++) if (strcmp(sfx_event_names[k], v0) == 0) {
                if (declared[k]) ps_log_warning(c, lineNumber, "%S already declared.", v0);
                declared[k] = true;
                g->sfx_Events[k] = seed;
            }
            continue;
        }
        int target = atom(&c->atoms, v0);
        const char *verb = v1;
        PVec dirs = { 0, 0, NULL };
        for (int j = 2; j < snd->n - 1; j++) {
            if (snd->t[j].type == SND_DIRECTION) pv_push(&c->tmp, &dirs, snd->t[j].text);
            else ps_log_error(c, lineNumber, "Expected a direction here, but found instead \"%s\".", snd->t[j].text);
        }
        if (dirs.n > 0 && strcmp(verb, "move") != 0 && strcmp(verb, "cantmove") != 0)
            ps_log_error(c, lineNumber, "Incorrect sound declaration - cannot have directions (UP/DOWN/etc.) attached to non-directional sound verbs (CREATE is not directional, but MOVE is directional).");
        if (strcmp(verb, "action") == 0) { verb = "move"; dirs.n = 0; pv_push(&c->tmp, &dirs, "___action____"); }
        if (dirs.n == 0) pv_push(&c->tmp, &dirs, "orthogonal");
        if (ad_has(&c->aggregatesDict, target)) {
            ps_log_error(c, lineNumber, "cannot assign sound events to aggregate objects (declared with \"and\"), only to regular objects, or properties, things defined in terms of \"or\" (\"%s\").", v0);
            continue;
        } else if (!ad_has(&c->objectMasks, target)) {
            ps_log_error(c, lineNumber, "Object \"%s\" not found.", v0);
        }
        uint32_t *objectMask = obj_mask(c, target);
        int directionMask = 0;
        for (int j = 0; j < dirs.n; j++) {
            int dm = sound_dir_mask(dirs.v[j]);
            if (dm < 0) ps_log_error(c, lineNumber, "Was expecting a direction, instead found \"%s\".", (const char *)dirs.v[j]);
            else directionMask |= dm;
        }
        IVec targets = { 0, 0, NULL };
        iv_push(&c->tmp, &targets, target);
        bool modified = true;
        int guard = 0;
        while (modified && guard++ < 1000) {
            modified = false;
            for (int k = 0; k < targets.n; k++) {
                int t = targets.v[k];
                if (syn_get(c, t) >= 0) { targets.v[k] = syn_get(c, t); modified = true; }
                else if (ad_has(&c->propertiesDict, t)) {
                    modified = true;
                    IVec *props = dict_list(&c->propertiesDict, t);
                    iv_splice(&targets, k, 1); k--;
                    for (int l = 0; l < props->n; l++) iv_push(&c->tmp, &targets, props->v[l]);
                }
            }
        }
        if (strcmp(verb, "move") == 0 || strcmp(verb, "cantmove") == 0) {
            for (int j = 0; j < targets.n; j++) {
                PObj *td = ps_obj(c, targets.v[j]);
                if (!td || td->layer < 0) continue;
                PSSfxMovementMask *o = ar_alloc(c->out, sizeof *o);
                o->objectMask = bv_new(c->out, so);
                bv_set(o->objectMask, td->id, so);
                o->directionMask = bv_new(c->out, sm);
                bv_shiftor(o->directionMask, (uint32_t)directionMask, 5 * td->layer, sm);
                o->seed = seed;
                if (strcmp(verb, "move") == 0) pv_push(&c->tmp, &moves[td->layer], o);
                else pv_push(&c->tmp, &failures, o);
            }
        }
        if (!objectMask) continue;
        if (strcmp(verb, "create") == 0) {
            PSSfxObjectMask *o = &g->sfx_CreationMasks[g->sfx_CreationMaskCount++];
            o->objectMask = objectMask; o->seed = seed;
        } else if (strcmp(verb, "destroy") == 0) {
            PSSfxObjectMask *o = &g->sfx_DestructionMasks[g->sfx_DestructionMaskCount++];
            o->objectMask = objectMask; o->seed = seed;
        }
    }
    g->sfx_MovementFailureMaskCount = failures.n;
    g->sfx_MovementFailureMasks = ar_alloc(c->out, sizeof(PSSfxMovementMask) * (failures.n + 1));
    for (int i = 0; i < failures.n; i++) g->sfx_MovementFailureMasks[i] = *(PSSfxMovementMask *)failures.v[i];
    g->sfx_MovementMaskCounts = ar_alloc(c->out, sizeof(int) * (L + 1));
    g->sfx_MovementMasks = ar_alloc(c->out, sizeof(PSSfxMovementMask *) * (L + 1));
    for (int l = 0; l < L; l++) {
        g->sfx_MovementMaskCounts[l] = moves[l].n;
        g->sfx_MovementMasks[l] = ar_alloc(c->out, sizeof(PSSfxMovementMask) * (moves[l].n + 1));
        for (int i = 0; i < moves[l].n; i++) g->sfx_MovementMasks[l][i] = *(PSSfxMovementMask *)moves[l].v[i];
    }
    g->hasSounds = ns > 0;
}

// =============================================================================================
// output

static void fill_objects(PSComp *c)
{
    PSGame *g = c->g;
    g->strideObj = c->strideObj; g->strideMov = c->strideMov; g->layerCount = c->layerCount;
    g->objectCount = c->objectCount;
    g->objects = ar_alloc(c->out, sizeof(PSObject) * (c->objectCount + 1));
    for (int i = 0; i < c->objectCount; i++) {
        PObj *o = ps_obj(c, c->idDict.v[i]);
        PSObject *P = &g->objects[i];
        P->name = ar_strdup(c->out, S(o->name));
        P->id = i; P->layer = o->layer; P->lineNumber = o->lineNumber;
        P->ncolors = o->ncolors; P->colors = o->rgb;
        P->spriteW = P->spriteH = o->spriteN; P->sprite = o->sprite;
    }
    g->layerMasks = c->layerMasks;
    g->playerMask = c->playerMask;
    g->playerMaskAggregate = c->playerAggregate;
    g->backgroundid = c->backgroundid;
    g->backgroundlayer = c->backgroundlayer;
}

static void copy_errors(PSComp *c)
{
    PSGame *g = c->g;
    g->errorCount = c->errorCount;
    g->errorStringCount = c->errs.n;
    g->errorStrings = ar_alloc(c->out, sizeof(char *) * (c->errs.n + 1));
    for (int i = 0; i < c->errs.n; i++) g->errorStrings[i] = ar_strdup(c->out, c->errs.v[i]);
}

// loadFile; returns false when JS would return null/undefined
static bool load_file(PSComp *c, const char *src, size_t len)
{
    if (!ps_parse_source(c, src, len)) return false;
    MEMSTAGE("parse");
    if (!ps_word_already_declared(c, A.player))
        ps_log_error(c, -1, "Error, didn't find any object called player, either in the objects section, or the legends section. There must be a player!");
    if (!ps_word_already_declared(c, A.background))
        ps_log_error(c, -1, "Error, didn't find any object called background, either in the objects section, or the legends section. There must be a background!");
    if (c->collisionLayers.n == 0) {
        ps_log_error(c, -1, "No collision layers defined.  All objects need to be in collision layers.");
        return false;
    }
    generate_extra_members(c);
    generate_masks(c);
    MEMSTAGE("generate_masks");
    fill_objects(c);
    MEMSTAGE("fill_objects");
    levels_to_array(c);
    MEMSTAGE("levels_to_array");
    rules_to_array(c);
    MEMSTAGE("rules_to_array");
    if (c->invalid) return false;
    for (int i = 0; i < c->crules.n; i++) cache_rule_string_rep(c, c->crules.v[i]);
    MEMSTAGE("cache_rule_string_rep");
    remove_duplicate_rules(c);
    MEMSTAGE("remove_duplicate_rules");
    rules_to_mask(c);
    MEMSTAGE("rules_to_mask");
    c->g->ruleGroupCount = collapse_groups(c, &c->crules, false, &c->g->rules);
    c->g->lateRuleGroupCount = collapse_groups(c, &c->crules, true, &c->g->lateRules);
    MEMSTAGE("collapse_groups");
    generate_rigid_group_list(c);
    MEMSTAGE("generate_rigid_group_list");
    process_win_conditions(c);
    check_objects_are_layered(c);
    twiddle_metadata(c);
    generate_loop_points(c);
    generate_sound_data(c);
    format_home_page(c);
    MEMSTAGE("rest");
    return true;
}

// make every pointer the engine/shell may touch valid, even when compilation bailed out early
static void ensure_defaults(PSComp *c)
{
    PSGame *g = c->g;
    Arena *o = c->out;
    int L = g->layerCount, so = g->strideObj > 0 ? g->strideObj : 1;
    if (!g->objects) { g->objects = ar_alloc(o, sizeof(PSObject)); g->objectCount = 0; }
    if (!g->layerMasks) { g->layerMasks = ar_alloc(o, sizeof(uint32_t *) * (L + 1)); for (int i = 0; i < L; i++) g->layerMasks[i] = bv_new(o, so); }
    if (!g->playerMask) g->playerMask = bv_new(o, so);
    if (!g->rules) { g->rules = ar_alloc(o, sizeof(PSRuleGroup)); g->ruleGroupCount = 0; }
    if (!g->lateRules) { g->lateRules = ar_alloc(o, sizeof(PSRuleGroup)); g->lateRuleGroupCount = 0; }
    if (!g->loopPoint) { g->loopPoint = ar_alloc(o, sizeof(int) * (g->ruleGroupCount + 1)); for (int i = 0; i <= g->ruleGroupCount; i++) g->loopPoint[i] = -1; }
    if (!g->lateLoopPoint) { g->lateLoopPoint = ar_alloc(o, sizeof(int) * (g->lateRuleGroupCount + 1)); for (int i = 0; i <= g->lateRuleGroupCount; i++) g->lateLoopPoint[i] = -1; }
    if (!g->rigidGroupIndex_to_GroupIndex) { g->rigidGroupIndex_to_GroupIndex = ar_alloc(o, sizeof(int)); g->rigidGroupCount = 0; g->rigid = false; }
    if (!g->groupNumber_to_RigidGroupIndex) { g->groupNumber_to_RigidGroupIndex = ar_alloc(o, sizeof(int)); g->groupNumber_to_RigidGroupIndex[0] = -1; g->groupNumberCount = 1; }
    if (!g->winconditions) { g->winconditions = ar_alloc(o, sizeof(PSWinCondition)); g->winconditionCount = 0; }
    if (!g->levels) { g->levels = ar_alloc(o, sizeof(PSLevelDef)); g->levelCount = 0; }
    if (!g->sfx_CreationMasks) { g->sfx_CreationMasks = ar_alloc(o, sizeof(PSSfxObjectMask)); g->sfx_CreationMaskCount = 0; }
    if (!g->sfx_DestructionMasks) { g->sfx_DestructionMasks = ar_alloc(o, sizeof(PSSfxObjectMask)); g->sfx_DestructionMaskCount = 0; }
    if (!g->sfx_MovementFailureMasks) { g->sfx_MovementFailureMasks = ar_alloc(o, sizeof(PSSfxMovementMask)); g->sfx_MovementFailureMaskCount = 0; }
    if (!g->sfx_MovementMaskCounts) g->sfx_MovementMaskCounts = ar_alloc(o, sizeof(int) * (L + 1));
    if (!g->sfx_MovementMasks) {
        g->sfx_MovementMasks = ar_alloc(o, sizeof(PSSfxMovementMask *) * (L + 1));
        for (int i = 0; i < L; i++) g->sfx_MovementMasks[i] = ar_alloc(o, sizeof(PSSfxMovementMask));
    }
}

PSGame *ps_compile(const char *src, size_t len)
{
    PSGame *g = ps_malloc(sizeof(PSGame));
    Arena *out = ps_malloc(sizeof(Arena));
    if (!g || !out) { ps_free(g); ps_free(out); return NULL; }
    g->priv = out;
    for (int i = 0; i < PS_SFX_EVENT_COUNT; i++) g->sfx_Events[i] = -1;
    g->metadata.background_color = 0x000000; g->metadata.text_color = 0xFFFFFF;
    g->metadata.again_interval = 0.15; g->metadata.key_repeat_interval = 0.15;
    g->backgroundid = g->backgroundlayer = -1;

    PSComp *c = ps_malloc(sizeof(PSComp));
    if (!c) return g;
    c->out = out; c->g = g;
    c->atoms.a = &c->perm;
    ad_init(&c->objects, &c->tmp); ad_init(&c->names, &c->tmp); ad_init(&c->metadata_lines, &c->tmp);
    ad_init(&c->original_line_numbers, &c->tmp); ad_init(&c->abbrevNames, &c->tmp);
    ad_init(&c->glyphDict, &c->tmp); ad_init(&c->aggregatesDict, &c->tmp); ad_init(&c->propertiesDict, &c->tmp);
    ad_init(&c->synonymsDict, &c->tmp); ad_init(&c->propertiesSingleLayer, &c->tmp);
    ad_init(&c->objectMasks, &c->tmp); ad_init(&c->aggregateMasks, &c->tmp); ad_init(&c->newmeta, &c->tmp);
    c->backgroundid = c->backgroundlayer = -1;
    init_atoms(c);
    c->compiling = true;

    bool ok = false;
    if (setjmp(c->abort_jb) == 0) {
        ok = load_file(c, src, len);
        c->compiling = false;
        if (ok && g->levelCount == 0) ps_log_error_urgent(c, -1, "No levels found.  Add some levels!");
    }
    c->compiling = false;
    if (!ok) {   // JS: state is null/undefined, nothing playable
        g->levelCount = 0; g->ruleGroupCount = 0; g->lateRuleGroupCount = 0; g->winconditionCount = 0;
        g->sfx_CreationMaskCount = g->sfx_DestructionMaskCount = g->sfx_MovementFailureMaskCount = 0;
        if (g->errorCount == 0 && c->errorCount == 0) c->errorCount = 1;
    }
    ensure_defaults(c);
    copy_errors(c);
#ifndef TARGET_PLAYDATE
    if (getenv("PS_DEBUG_MEM")) fprintf(stderr, "compile memory: scratch %zu bytes, game %zu bytes\n", c->tmp.total, out->total);
#endif
    ar_free_all(&c->tmp);
    ar_free_all(&c->work);
    ar_free_all(&c->perm);
    ps_free(c);
    return g;
}

void ps_game_free(PSGame *g)
{
    if (!g) return;
    Arena *out = g->priv;
    if (out) { ar_free_all(out); ps_free(out); }
    ps_free(g);
}
