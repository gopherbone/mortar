/* SPDX-License-Identifier: MIT
 * mortar_decls.h: forced into every TU (CMakeLists.txt), like the Switch
 * build's switch_decls.h. glibc declares bcmp/bcopy/bzero in <strings.h>
 * (libultra.h skips its own declarations for TARGET_MORTAR, mortar.patch),
 * and some decomp headers (JUTFont.h) rely on <string.h> coming along. */
#ifndef AC_MORTAR_DECLS_H
#define AC_MORTAR_DECLS_H
#include <string.h>
#include <strings.h>
/* bionic and newlib pull these in indirectly; JKRArchivePri.cpp needs tolower */
#include <ctype.h>
/* jaudio's `typedef struct link_ link` clashes with POSIX link(2), which
 * glibc's <unistd.h> declares (pulled in by the SDL2 headers here). Declare
 * the POSIX one first, then give the game's type another name everywhere. */
#include <unistd.h>
#define link ac_link
#endif
