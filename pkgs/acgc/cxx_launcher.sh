# SPDX-License-Identifier: MIT
# CMAKE_CXX_COMPILER_LAUNCHER (as switch/cxx_launcher.sh): compile, then
# rename the global operator new/delete in the object (switch/game_new.syms).
# The game replaces them with JKRHeap allocators, which return NULL before
# the game has made its heaps. In a dynamic Linux executable those symbols
# would be exported and interpose libstdc++'s for every shared library (SDL,
# PipeWire, the GL driver); renamed, only the game's own code uses JKRHeap.
"$@" || exit $?
out=
while [ $# -gt 0 ]; do
    [ "$1" = "-o" ] && out="$2"
    shift
done
[ -n "$out" ] && "${OBJCOPY:-objcopy}" --redefine-syms="$(dirname "$0")/../switch/game_new.syms" "$out"
