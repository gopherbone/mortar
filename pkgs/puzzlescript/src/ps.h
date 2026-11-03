// SPDX-License-Identifier: MIT
// PuzzleScript for Playdate: shared types and public API.
//
// This is a native C port of PuzzleScript (https://github.com/increpare/puzzlescript,
// reference checkout in ../puzzlescript). The port mirrors the JS closely:
//
//   ps_compile.c  parser.js + compiler.js   source text -> PSGame (immutable compiled game)
//   ps_engine.c   engine.js + the non-DOM parts of inputoutput.js (checkKey/update)
//                 PSGame -> PSSession (all mutable play state)
//   ps_rng.c      rng.js (RC4-seeded RNG; must match JS bit-for-bit, tests use seeds)
//   ps_sfx.c      sfxr.js (seed -> PCM), so sounds match the web player
//   ps_font.c     font.js glyphs
//   main.c, ps_ui.c, ps_render.c   Playdate shell (only these include pd_api.h)
//
// Everything outside the shell is plain C99 + libc-free-ish (no stdio), so it builds both for the
// device and for the host test runner (tests/). Allocate only through ps_malloc/ps_realloc/ps_free.
//
// Bit layout conventions are exactly the JS ones:
//   * An object mask is strideObj uint32 words; object id i is bit (i & 31) of word (i >> 5).
//   * A movement mask is strideMov words; layer L uses the 5 bits starting at bit 5*L
//     (layers may straddle words exactly as in bitvec.js's ishiftor/getshiftor):
//     up=1, down=2, left=4, right=8, action=16 (so "?"/moving=15, no=3 in dirMasksDelta).
//   * Level cells are column-major: index = x*height + y (as in JS).
//   * level.objects is n_tiles*strideObj words, level.movements n_tiles*strideMov words.
#ifndef PS_H
#define PS_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

// ---------------------------------------------------------------------------------------------
// memory / platform hooks (set by the host before anything else; default to libc on host builds)
extern void *(*ps_realloc_hook)(void *ptr, size_t size);
void *ps_malloc(size_t n);            // zeroed
void *ps_malloc_raw(size_t n);        // not zeroed (device heap stores are slow: don't zero what gets overwritten)
void *ps_realloc(void *p, size_t n);
void  ps_free(void *p);
char *ps_strdup(const char *s);
// optional log sink for console messages (consolePrint / logError / logWarning). may be NULL.
extern void (*ps_log_hook)(const char *msg);
// Device safety valve: when set and it returns true, the engine's rule loops stop early as if
// they had hit their iteration caps (the Playdate kills a game whose frame runs over 10 s).
// Never set by the test runner, so it can't change tested behaviour.
extern bool (*ps_over_budget_hook)(void);

// ---------------------------------------------------------------------------------------------
// RNG (rng.js). seed is the JS seed *string* (JS does `new RNG(seedString)`; a null seed in JS
// means Math.random()+Date.now() -> pass NULL and the implementation picks something).
// The seed is UTF-8; it is converted to UTF-16 code units like JS getBytes() sees it.
// Non-string JS seeds are JSON.stringify'ed first (a number n -> its JS toString, e.g. "123").
typedef struct {
    uint8_t s[256];
    int i, j;
} PSRng;
void   ps_rng_init(PSRng *r, const char *seed);
double ps_rng_uniform(PSRng *r);       // RNG.prototype.uniform
int    ps_rng_next_byte(PSRng *r);     // RNG.prototype.nextByte (0..255)
int    ps_rng_random(PSRng *r, int n, int m);   // RNG.prototype.random(n, m): n + floor(uniform*(m-n))
int    ps_rng_index(PSRng *r, uint32_t n);      // Math.floor(uniform() * n), exactly, in integer arithmetic
// Mix extra entropy (e.g. the current time) into the seeds chosen for ps_rng_init(r, NULL).
void   ps_rng_set_entropy(uint32_t e);

// ---------------------------------------------------------------------------------------------
// compiled game (output of ps_compile; read-only afterwards). Everything hangs off one arena
// freed by ps_game_free. Identical masks, replacements and cell-pattern rows are shared between
// rules (hash-consed), so never write through these pointers.

enum { PS_DIR_UP = 1, PS_DIR_DOWN = 2, PS_DIR_LEFT = 4, PS_DIR_RIGHT = 8, PS_DIR_ACTION = 16 };

typedef struct {
    char    *name;          // lower-case, as JS stores it
    int      id;
    int      layer;
    int      lineNumber;
    int      ncolors;
    uint32_t *colors;       // 0xRRGGBB, or PS_COLOR_TRANSPARENT
    int      spriteW, spriteH;   // usually 5x5; JS supports other square sizes
    int8_t  *sprite;        // spriteW*spriteH, row-major, -1 = transparent, else index into colors
} PSObject;
#define PS_COLOR_TRANSPARENT 0xFF000000u

typedef struct {            // CellReplacement (engine.js)
    uint32_t *objectsClear, *objectsSet;             // strideObj
    uint32_t *movementsClear, *movementsSet;         // strideMov
    uint32_t *movementsLayerMask;                    // strideMov
    uint32_t *randomEntityMask;                      // strideObj
    uint32_t *randomDirMask;                         // strideMov
} PSReplacement;

typedef struct {            // CellPattern (engine.js); an ellipsis is a pattern with isEllipsis set
    bool      isEllipsis;
    uint32_t *objectsPresent, *objectsMissing;       // strideObj
    int       anyCount;
    uint32_t **anyObjectsPresent;                    // anyCount masks of strideObj
    uint32_t *movementsPresent, *movementsMissing;   // strideMov
    PSReplacement *replacement;                      // NULL when the JS has null
} PSCellPattern;

typedef struct {
    int            len;     // number of cells, ellipses included
    PSCellPattern *cells;
    int            ellipsisCount;   // 0, 1 or 2
} PSCellRow;

enum {
    PS_CMD_SFX0 = 0,        // ..PS_CMD_SFX10 = 10
    PS_CMD_SFX10 = 10,
    PS_CMD_CANCEL, PS_CMD_CHECKPOINT, PS_CMD_RESTART, PS_CMD_WIN, PS_CMD_MESSAGE, PS_CMD_AGAIN,
    PS_CMD_COUNT
};
typedef struct { int type; char *message; } PSCommand;   // message only for PS_CMD_MESSAGE

typedef struct {            // Rule (engine.js)
    int        direction;   // PS_DIR_*
    int        rowCount;
    PSCellRow *rows;        // "patterns"
    bool       hasReplacements;
    int        lineNumber;
    int        groupNumber;
    bool       rigid;
    int        commandCount;
    PSCommand *commands;
    bool       isRandom;
    uint32_t **cellRowMasks;            // rowCount masks of strideObj
    uint32_t **cellRowMasks_Movements;  // rowCount masks of strideMov
    uint32_t  *ruleMask;                // OR of cellRowMasks
} PSRule;

typedef struct { int count; PSRule *rules; } PSRuleGroup;

typedef struct {            // processWinConditions output: [num, mask1, mask2, lineNumber, aggr1, aggr2]
    int       quantifier;   // -1 no, 0 some, 1 all
    uint32_t *filter1, *filter2;
    int       lineNumber;
    bool      aggr1, aggr2;
} PSWinCondition;

typedef struct {
    bool      isMessage;
    char     *message;      // when isMessage
    int       lineNumber;
    int       width, height;
    uint32_t *objects;      // width*height*strideObj, column-major
} PSLevelDef;

typedef struct { uint32_t *objectMask; uint32_t *directionMask; int64_t seed; } PSSfxMovementMask;
typedef struct { uint32_t *objectMask; int64_t seed; } PSSfxObjectMask;

// game events with a sound seed (state.sfx_Events); -1 when unset
enum {
    PS_SFX_UNDO, PS_SFX_RESTART, PS_SFX_TITLESCREEN, PS_SFX_STARTGAME, PS_SFX_CANCEL,
    PS_SFX_ENDGAME, PS_SFX_STARTLEVEL, PS_SFX_ENDLEVEL, PS_SFX_SHOWMESSAGE, PS_SFX_CLOSEMESSAGE,
    PS_SFX_EVENT_SFX0, // .. PS_SFX_EVENT_SFX0+10
    PS_SFX_EVENT_COUNT = PS_SFX_EVENT_SFX0 + 11
};

typedef struct {
    // metadata (state.metadata); strings NULL when absent, numbers -1/0 when absent
    char   *title, *author, *homepage;
    char   *background_color_str, *text_color_str;
    uint32_t background_color, text_color;      // resolved 0xRRGGBB (defaults black / white)
    double  again_interval;         // seconds = Number(value) (may be NaN); 0.15 (JS 150ms) when absent
    double  realtime_interval;      // 0 when absent
    double  key_repeat_interval;    // 0.15 when absent
    bool    noaction, noundo, norestart, norepeat_action, require_player_movement;
    bool    run_rules_on_level_start, throttle_movement, verbose_logging;
    bool    has_flickscreen, has_zoomscreen;
    int     flickscreen[2], zoomscreen[2];
    char   *color_palette;          // raw value, already applied to colors
    // keep anything else the JS reads from state.metadata (e.g. text_controls, text_message_continue,
    // youtube is ignored)
    char   *text_controls, *text_message_continue;
    bool    scanline;               // "scanline" in state.metadata (graphics.js halves pixel height)
} PSMetadata;

typedef struct PSGame {
    PSMetadata metadata;

    int strideObj, strideMov, layerCount;
    int objectCount;
    PSObject *objects;      // indexed by id (state.idDict order)
    uint32_t **layerMasks;  // layerCount masks
    uint32_t *playerMask;   // state.playerMask[1]
    bool      playerMaskAggregate;  // state.playerMask[0]
    int       backgroundid, backgroundlayer;

    int          ruleGroupCount;
    PSRuleGroup *rules;
    int         *loopPoint;          // ruleGroupCount+1 entries, -1 = undefined
    int          lateRuleGroupCount;
    PSRuleGroup *lateRules;
    int         *lateLoopPoint;      // lateRuleGroupCount+1 entries
    bool         rigid;              // state.rigid
    int          rigidGroupCount;
    int         *rigidGroupIndex_to_GroupIndex;
    int         *groupNumber_to_RigidGroupIndex;   // indexed by groupNumber; size groupNumberCount
    int          groupNumberCount;

    int             winconditionCount;
    PSWinCondition *winconditions;

    int         levelCount;
    PSLevelDef *levels;

    // sounds
    int64_t sfx_Events[PS_SFX_EVENT_COUNT];   // seeds are int64: JS allows any digit count
    int sfx_CreationMaskCount, sfx_DestructionMaskCount, sfx_MovementFailureMaskCount;
    PSSfxObjectMask   *sfx_CreationMasks, *sfx_DestructionMasks;
    PSSfxMovementMask *sfx_MovementFailureMasks;
    int               *sfx_MovementMaskCounts;   // per layer
    PSSfxMovementMask **sfx_MovementMasks;       // per layer
    bool hasSounds;                              // state.sounds.length > 0

    // glyph dictionary for convertLevelToString / level editor-ish needs (optional)

    // diagnostics
    int    errorCount;
    char **errorStrings;     // plain text (HTML stripped)
    int    errorStringCount;
    // implementation-private data (arena etc.)
    void  *priv;
} PSGame;

// Compile PuzzleScript source. Never returns NULL; check game->errorCount. A game with errors
// may still be partially usable exactly as in the web editor, but the shell refuses to run it.
PSGame *ps_compile(const char *src, size_t len);
void    ps_game_free(PSGame *g);

// ---------------------------------------------------------------------------------------------
// play session (engine.js globals + inputoutput.js timing/keys)

#define PS_TERMINAL_WIDTH 34
#define PS_TERMINAL_HEIGHT 13

typedef struct PSLevel {    // Level (level.js) + RebuildLevelArrays
    int lineNumber, width, height, n_tiles, layerCount;
    uint32_t *objects, *movements;
    uint32_t *rigidGroupIndexMask, *rigidMovementAppliedMask;   // n_tiles*strideMov, when rigid
    // row: height*strideObj, col: width*strideObj, map: strideObj (movements: same with strideMov)
    uint32_t *rowCellContents, *colCellContents, *mapCellContents;
    uint32_t *rowCellContents_Movements, *colCellContents_Movements, *mapCellContents_Movements;
    int commandQueue[PS_CMD_COUNT]; int commandQueueLen;
    // commandQueueSourceRules as needed
} PSLevel;

// Keys, as checkKey() sees them.
enum {
    PS_KEY_UP, PS_KEY_LEFT, PS_KEY_DOWN, PS_KEY_RIGHT, PS_KEY_ACTION,
    PS_KEY_UNDO, PS_KEY_RESTART, PS_KEY_QUIT, PS_KEY_COUNT
};

typedef struct PSSession PSSession;   // opaque-ish; defined in ps_engine.c

// Storage hooks for progress (JS storage_set/get/remove keyed by document.URL). The shell
// implements these with the Playdate datastore; the test runner leaves them NULL.
typedef struct {
    void  *ctx;
    char *(*get)(void *ctx, const char *key);          // returns ps_malloc'ed string or NULL
    void  (*set)(void *ctx, const char *key, const char *value);
    void  (*remove)(void *ctx, const char *key);
} PSStorage;

// setGameState(state, ["restart"]): title screen. storage may be NULL.
PSSession *ps_session_new(const PSGame *g, const PSStorage *storage, const char *seed);
void       ps_session_free(PSSession *s);

// --- headless/test entry points (mirror testingFrameWork.js runTest) ---
void ps_set_unit_testing(PSSession *s, bool on);   // unitTesting global
void ps_load_level(PSSession *s, int levelIndex, const char *seed);  // compile(["loadLevel",n],..,seed)
bool ps_process_input(PSSession *s, int dir);      // processInput(dir): -1 tick, 0 up,1 left,2 down,3 right,4 action
bool ps_againing(PSSession *s);
void ps_clear_againing(PSSession *s);
void ps_undo(PSSession *s);                        // DoUndo(false,true)
void ps_restart(PSSession *s);                     // DoRestart()
char *ps_level_to_string(PSSession *s);            // convertLevelToString(); ps_free the result
// soundHistory as the tests see it (debug.js pushes every played seed)
int  ps_sound_history(PSSession *s, const int64_t **seeds);

// --- interactive entry points (mirror inputoutput.js) ---
// checkKey(e, justPressed) for a key press (justPressed=true) or a held repeat (false; the session
// does its own key-repeat timing in ps_update when a key is held, see ps_key_held).
void ps_key_down(PSSession *s, int key);
void ps_key_up(PSSession *s, int key);
// update(): advances timers (again_interval, realtime_interval, win delay, title/message quit
// delays, key repeat). dt in seconds.
void ps_update(PSSession *s, double dt);

// What the shell needs to draw.
bool ps_in_text_mode(const PSSession *s);          // textMode (title screen or message)
bool ps_on_title_screen(const PSSession *s);
// The 34x13 terminal text (titleImage) for text mode, as UTF-8 lines (may contain non-ASCII).
const char *const *ps_text_lines(const PSSession *s);
const PSLevel *ps_current_level(const PSSession *s);
int  ps_current_level_index(const PSSession *s);   // curlevel
// visible window in cells for flickscreen/zoomscreen (oldflickscreendat), or the whole level.
void ps_view_rect(const PSSession *s, int *x0, int *y0, int *x1, int *y1);
// set when the screen needs redrawing since the last call (clears the flag)
bool ps_take_dirty(PSSession *s);
// sound seeds to play since the last call (clears the queue). n <= 32.
int  ps_take_sounds(PSSession *s, int64_t *out, int max);
// level select support: jump straight to a level (like the editor's "levelline" / loadLevel)
void ps_goto_level(PSSession *s, int levelIndex);
// back to title screen (goToTitleScreen)
void ps_goto_title(PSSession *s);
// Warnings the engine raises while running that JS records in errorStrings (engine.js logWarning,
// e.g. the title screen's "Game title is too long to fit on screen; truncating to fit."), each
// recorded once. metaKeys[i] names the metadata key whose line JS reports (state.metadata_lines);
// PSMetadata does not carry metadata line numbers. They do not count towards errorCount.
int  ps_session_warnings(const PSSession *s, const char *const **messages, const char *const **metaKeys);
bool ps_is_winning(const PSSession *s);           // `winning`: level won, waiting 0.5s before nextLevel
bool ps_is_againing(const PSSession *s);          // an `again` turn is pending (input is ignored)
// Storage keys used: "" (= document.URL: current level index) and "_checkpoint" (JSON of the
// checkpoint, same shape as the JS). The shell should namespace them per game.

// ---------------------------------------------------------------------------------------------
// sfxr (sfxr.js): generate the sound for a seed. Returns ps_malloc'ed int16 mono PCM at
// *sampleRate (22050: the rate the web player's AudioBuffer uses), length *nsamples. The
// web player's 3x 1600Hz lowpass biquads are applied. NULL on failure / negative seed.
int16_t *ps_sfx_generate(int64_t seed, int *nsamples, int *sampleRate);
// Same, for the seed token exactly as written in the SOUNDS section (any number of digits;
// JS keeps seeds as strings, and seeds > 2^31 behave like JS: ((Number(s)/100)|0) wraps).
int16_t *ps_sfx_generate_str(const char *seed, int *nsamples, int *sampleRate);

// ---------------------------------------------------------------------------------------------
// font (font.js): glyph bitmap for a code point, or NULL when the font lacks it.
// rows are top-to-bottom, each row's bits are the pixels left-to-right (bit (w-1-x)).
typedef struct { int w, h; const uint8_t *rows; } PSGlyph;
const PSGlyph *ps_font_glyph(uint32_t codepoint);

#endif
