#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# Refuse to publish a store path whose closure holds something that must not be
# public, by name: a second line behind mortar.public (tests/public-policy.nix),
# for what isn't marked unfree. Takes built paths or .drv files, the latter so it
# can run before anything is built.
#   usage: check-public-closure.sh <store path>...
set -euo pipefail

# Commercial software, and what comes from a user's own files.
deny='-(claude-code|mortar-claude|pico-8|pico8|acgc|GAFE01|openxcom-data|UFO|liar)([-.]|$)'

status=0
for path in "$@"; do
  if found=$(nix-store --query --requisites "$path" | grep -E -- "$deny"); then
    printf '::error::%s holds what must not be public:\n%s\n' "$path" "$found"
    status=1
  fi
done
exit "$status"
