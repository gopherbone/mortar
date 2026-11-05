#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# The list of terminal tools from the nixpkgs this flake pins, the one the systems are
# built from, so what's installed shares what it can with them. Adds it to the store as
# a path with no references and prints `<path> <count>`. About 3 GiB per worker, for
# minutes. Details: make-tools-index.py, docs/packages.md.
#   usage: make-tools-index.sh [system] [workers]
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
system=${1:-aarch64-linux}
workers=${2:-3}
rev=$(jq -r '.nodes[.nodes[.root].inputs.nixpkgs].locked.rev' "$here/../flake.lock")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/mortar-tools"

deny=$(bash "$here/check-public-closure.sh" --print-deny)
nix shell "github:NixOS/nixpkgs/$rev#nix-eval-jobs" "github:NixOS/nixpkgs/$rev#python3" -c bash -c '
  set -euo pipefail
  nix-eval-jobs --flake "github:NixOS/nixpkgs/$1#legacyPackages.$2" --meta --no-instantiate \
    --workers "$3" --max-memory-size 3000 2> "$4/eval.err" \
  | python3 -I "$5/make-tools-index.py" --system "$2" --deny="$6" --min 50000 > "$4/mortar-tools/tools.tsv"
' _ "$rev" "$system" "$workers" "$work" "$here" "$deny"
chmod -R a-w "$work/mortar-tools"
path=$(nix-store --add "$work/mortar-tools")
chmod -R u+w "$work/mortar-tools"
count=$(($(wc -l < "$work/mortar-tools/tools.tsv") - 1))
echo "$path $count"
