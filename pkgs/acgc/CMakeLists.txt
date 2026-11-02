# SPDX-License-Identifier: MIT
# Mortar (TrimUI Brick, aarch64 Linux, OpenGL ES 3 on SDL2) build of the
# Animal Crossing PC port. Copied into the source tree as mortar/ by
# pkgs/acgc/default.nix.
#
# Adapted from switch/CMakeLists.txt of digdat0/ACGC-Switch-Port (itself
# mirroring the Android build, app/src/main/cpp/CMakeLists.txt): the same
# sources and source filters, built with TARGET_ANDROID's code paths (GLES 3
# context, ES-compatible renderer, 64-bit fixes), plus TARGET_MORTAR for the
# few Brick changes in mortar.patch. mortar_glue.c implements the Android glue
# API, compat/glad/gl.h loads GLES at run time.
cmake_minimum_required(VERSION 3.16)
project(ac_mortar C CXX)

set(CMAKE_C_STANDARD 11)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_C_EXTENSIONS ON)
set(CMAKE_CXX_EXTENSIONS ON)

# The game's ::operator new/delete (JKRHeap) must not capture the GL
# driver's or libstdc++'s allocations: rename them in our C++ objects
# (switch/cxx_launcher.sh, switch/game_new.syms).
set(CMAKE_CXX_COMPILER_LAUNCHER sh "${CMAKE_CURRENT_SOURCE_DIR}/cxx_launcher.sh")

set(DECOMP_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/..")
set(ANDROID_CPP "${DECOMP_ROOT}/app/src/main/cpp")

find_package(PkgConfig REQUIRED)
pkg_check_modules(SDL2 REQUIRED IMPORTED_TARGET sdl2)

add_compile_definitions(
    TARGET_PC
    TARGET_ANDROID
    TARGET_MORTAR
    VERSION=0
    F3DEX_GBI_2
    NDEBUG
    BUGFIXES
    _LANGUAGE_C
    KEYBOARD_TYPING
)

add_compile_options(
    -O2
    -fno-strict-aliasing
    -fwrapv
    -mcpu=cortex-a53
    "$<$<COMPILE_LANGUAGE:C>:-Wno-error=implicit-function-declaration>"
    "$<$<COMPILE_LANGUAGE:C>:-Wno-error=int-conversion>"
    "$<$<COMPILE_LANGUAGE:C>:-Wno-error=incompatible-pointer-types>"
    "$<$<COMPILE_LANGUAGE:C>:-Wno-error=int-to-pointer-cast>"
    "SHELL:-include ${CMAKE_CURRENT_SOURCE_DIR}/mortar_decls.h"
)

include_directories(
    ${CMAKE_CURRENT_SOURCE_DIR}/compat   # glad/gl.h -> GLES 3 loader (first!)
    ${ANDROID_CPP}/android               # android_glue.h (implemented by mortar_glue.c)
    ${DECOMP_ROOT}/include
    ${DECOMP_ROOT}/src
    ${DECOMP_ROOT}
    ${DECOMP_ROOT}/pc/include
    ${DECOMP_ROOT}/pc/lib/fixnes
)

# =============================================================================
# Game sources (same rules as the Android and Switch builds)
# =============================================================================
file(GLOB_RECURSE GAME_C_SOURCES CONFIGURE_DEPENDS "${DECOMP_ROOT}/src/*.c")
file(GLOB_RECURSE GAME_CXX_SOURCES CONFIGURE_DEPENDS "${DECOMP_ROOT}/src/*.cpp")

foreach(pat
        ".*/AUS/.*" ".*/dolphin/gx/.*" ".*/dolphin/os/.*" ".*/dolphin/vi/.*" ".*/dolphin/pad/.*"
        ".*/dolphin/card/.*" ".*/dolphin/dvd/.*" ".*/dolphin/ai/.*" ".*/dolphin/ar/.*" ".*/dolphin/dsp/.*"
        ".*/dolphin/exi/.*" ".*/dolphin/si/.*" ".*/dolphin/db/.*" ".*/dolphin/mtx/.*" ".*/dolphin/base/.*"
        ".*/dolphin/gba/.*" ".*/dolphin/amcstubs/.*" ".*/dolphin/OdemuExi2/.*" ".*/dolphin/odenotstub/.*"
        ".*/TRK_MINNOW_DOLPHIN/.*" ".*/Runtime\\.PPCEABI\\.H/.*" ".*/MSL_C\\.PPCEABI\\.bare\\.H/.*"
        ".*/src/furniture/.*" ".*/src/data/model/obj_e_boat/.*" ".*/ac_npc_shop_common\\.c$"
        ".*/ac_animal_logo_misc\\.c$" ".*/m_item_debug\\.c$" ".*/actor/npc/.*_anime\\.c$" ".*/Famicom/.*"
        ".*/MSL_C/.*" ".*/jaudio_NES/internal/dsp_cardunlock\\.c$" ".*/jaudio_NES/internal/dsp_GBAKey\\.c$"
        ".*/libc64/malloc\\.c$" ".*/libc64/sprintf\\.c$" ".*/libc64/aprintf\\.c$" ".*/libultra/.*"
        ".*/ReconfigBATs\\.c$" ".*/executor\\.c$" ".*/ef_effect_lib\\.c$" ".*/m_card\\.c$"
        ".*/osreport\\.c$" ".*/emu64_utility\\.c$")
    list(FILTER GAME_C_SOURCES EXCLUDE REGEX "${pat}")
endforeach()
list(REMOVE_DUPLICATES GAME_C_SOURCES)

foreach(pat
        ".*/AUS/.*" ".*/dolphin/.*" ".*/TRK_MINNOW_DOLPHIN/.*" ".*/Runtime\\.PPCEABI\\.H/.*"
        ".*/MSL_C\\.PPCEABI\\.bare\\.H/.*" ".*/Famicom/.*" ".*/emu64_print\\.cpp$"
        ".*/jsyswrapper_ext\\.cpp$" ".*/jsyswrapper_main\\.cpp$")
    list(FILTER GAME_CXX_SOURCES EXCLUDE REGEX "${pat}")
endforeach()
list(APPEND GAME_CXX_SOURCES
    ${DECOMP_ROOT}/src/static/Famicom/famicom.cpp
    ${DECOMP_ROOT}/src/static/Famicom/famicom_nesinfo.cpp
    ${DECOMP_ROOT}/src/static/Famicom/ks_nes_core.cpp
    ${DECOMP_ROOT}/src/static/Famicom/ks_nes_draw.cpp
)

# =============================================================================
# PC port sources
# =============================================================================
set(PC_SOURCES)
foreach(f
        pc_main pc_gx pc_gx_tev pc_gx_texture pc_gbi_runtime pc_os pc_dvd pc_pad pc_typing pc_card pc_vi
        pc_audio pc_aram pc_misc pc_mtx pc_m_card pc_save_bswap pc_model_viewer pc_texture_pack pc_settings
        pc_profiler pc_pause_menu pc_settings_menu pc_menu_util pc_text_draw pc_keybindings pc_nes_fixnes
        pc_stubs pc_assets pc_msg_eur pc_disc)
    list(APPEND PC_SOURCES ${DECOMP_ROOT}/pc/src/${f}.c)
endforeach()
list(APPEND PC_SOURCES
    ${DECOMP_ROOT}/src/static/dolphin/pad/Padclamp.c
    ${DECOMP_ROOT}/pc/src/pc_stubs_cpp.cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/mortar_glue.c
)

# fixNES (the in-game NES emulator)
set(FIXNES_DIR ${DECOMP_ROOT}/pc/lib/fixnes)
file(GLOB FIXNES_MAPPER_SOURCES ${FIXNES_DIR}/mapper/*.c)
set(FIXNES_SOURCES ${FIXNES_MAPPER_SOURCES})
foreach(f cpu ppu apu mem input mapper mapperList vrc_irq audio fm2play audio_fds audio_mmc5 audio_vrc6
          audio_vrc7 audio_n163 audio_s5b)
    list(APPEND FIXNES_SOURCES ${FIXNES_DIR}/${f}.c)
endforeach()
set_source_files_properties(${FIXNES_SOURCES} PROPERTIES
    COMPILE_OPTIONS "-w;-O2"
    COMPILE_DEFINITIONS "COL_TEX_BSWAP;COL_GL_BSWAP;DO_INLINE_ATTRIBS=1")
set_source_files_properties(${DECOMP_ROOT}/pc/src/pc_nes_fixnes.c PROPERTIES COMPILE_OPTIONS "-w;-O2")

# main() renames: the game's and boot.c's main() step aside for pc_main.c's
set_source_files_properties("${DECOMP_ROOT}/src/main.c" PROPERTIES COMPILE_DEFINITIONS "main=ac_entry")
set_source_files_properties("${DECOMP_ROOT}/src/static/boot.c" PROPERTIES COMPILE_DEFINITIONS "main=boot_main")

# .c files that contain C++ code
set_source_files_properties(
    "${DECOMP_ROOT}/src/static/libforest/emu64/emu64.c"
    "${DECOMP_ROOT}/src/static/jaudio_NES/internal/ja_calc.c"
    "${DECOMP_ROOT}/src/static/jaudio_NES/internal/jammain_2.c"
    "${DECOMP_ROOT}/src/static/jaudio_NES/game/game64.c"
    PROPERTIES LANGUAGE CXX)

add_compile_options($<$<COMPILE_LANGUAGE:C>:-Dnullptr=NULL>)

set_source_files_properties(${GAME_C_SOURCES} ${GAME_CXX_SOURCES} PROPERTIES COMPILE_OPTIONS "-w;-fpermissive")
set(PC_C_SOURCES ${PC_SOURCES})
list(FILTER PC_C_SOURCES EXCLUDE REGEX "\\.cpp$")
# the GX -> GL translation layer is the hot path: -O3 there (as the Switch build)
set_source_files_properties(${PC_C_SOURCES} PROPERTIES COMPILE_OPTIONS "-w;-O3")
set_source_files_properties("${DECOMP_ROOT}/src/static/libforest/emu64/emu64.c" PROPERTIES COMPILE_OPTIONS "-w;-fpermissive;-O3")
set_source_files_properties(${DECOMP_ROOT}/pc/src/pc_stubs_cpp.cpp PROPERTIES COMPILE_OPTIONS "-w;-fpermissive")

add_executable(AnimalCrossing
    ${PC_SOURCES}
    ${FIXNES_SOURCES}
    ${GAME_C_SOURCES}
    ${GAME_CXX_SOURCES}
)
# no GL/EGL link: SDL loads them, compat/glad/gl.h resolves through SDL
target_link_libraries(AnimalCrossing PkgConfig::SDL2 dl pthread m)

install(TARGETS AnimalCrossing RUNTIME DESTINATION libexec/acgc)
install(FILES ${DECOMP_ROOT}/pc/shaders/default.vert ${DECOMP_ROOT}/pc/shaders/default.frag
    DESTINATION share/acgc/shaders)
