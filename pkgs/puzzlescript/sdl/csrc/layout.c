// SPDX-License-Identifier: MIT
// Struct layout probe for the hand-written Rust bindings (src/ffi.rs): the Rust side checks
// these sizes/offsets at startup and in `cargo test`, so a change to ps.h can't silently
// desynchronise the FFI. This is the only C in the SDL shell.
#include <stddef.h>
#include "ps.h"

const size_t ps_ffi_layout[] = {
    sizeof(PSMetadata), offsetof(PSMetadata, scanline),
    sizeof(PSObject), offsetof(PSObject, sprite),
    sizeof(PSLevelDef), offsetof(PSLevelDef, objects),
    sizeof(PSGame), offsetof(PSGame, objects), offsetof(PSGame, backgroundid),
    offsetof(PSGame, levelCount), offsetof(PSGame, levels), offsetof(PSGame, sfx_Events),
    offsetof(PSGame, sfx_MovementMasks), offsetof(PSGame, errorCount),
    offsetof(PSGame, errorStringCount), offsetof(PSGame, priv),
    offsetof(PSLevel, width), offsetof(PSLevel, height), offsetof(PSLevel, objects),
    sizeof(PSSfxObjectMask), sizeof(PSSfxMovementMask), sizeof(PSStorage),
    0,
};
