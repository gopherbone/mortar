// SPDX-License-Identifier: GPL-2.0-or-later
// libretro's retro_log_printf_t is variadic: format here, log in Rust.
#include <stdarg.h>
#include <stdio.h>

void mortar_play_log(int level, const char *msg);

void mortar_play_log_printf(int level, const char *fmt, ...)
{
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    mortar_play_log(level, buf);
}
