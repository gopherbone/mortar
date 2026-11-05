# SPDX-License-Identifier: GPL-2.0-or-later
# scripts/make-tools-index.py on made-up nix-eval-jobs output: which output is listed,
# what's left out, and the columns pkgs/mortar-index/src/compact.rs reads. Asks no cache.
{ runCommand, python3 }:

runCommand "tools-index-check" { nativeBuildInputs = [ python3 ]; } ''
  h() { printf '%032d' 0 | tr 0 "$1"; }
  a=$(h a) b=$(h b) c=$(h c) d=$(h d) f=$(h f)
  cat > jobs <<JSON
  {"attr":"zed","system":"aarch64-linux","outputs":{"out":"/nix/store/$a-zed-1"},"meta":{"description":"two\tcolumns\nand lines","mainProgram":"z"}}
  {"attr":"jq","system":"aarch64-linux","outputs":{"bin":"/nix/store/$b-jq-1-bin","out":"/nix/store/$c-jq-1"},"meta":{"outputsToInstall":["bin","man"],"mainProgram":"jq"}}
  {"attr":"Upper","system":"aarch64-linux","outputs":{"out":"/nix/store/$d-upper-1"},"meta":{}}
  {"attr":"broken","error":"nope"}
  {"attr":"other","system":"x86_64-linux","outputs":{"out":"/nix/store/$f-other-1"},"meta":{}}
  {"attr":"claude-code","system":"aarch64-linux","outputs":{"out":"/nix/store/$f-claude-code-1"},"meta":{}}
  {"attr":"claude-code-router","system":"aarch64-linux","outputs":{"out":"/nix/store/$f-claude-code-router-1"},"meta":{}}
  {"attr":"pico8-thing","system":"aarch64-linux","outputs":{"out":"/nix/store/$f-renamed-1"},"meta":{}}
  {"attr":"gen-1-link","system":"aarch64-linux","outputs":{"out":"/nix/store/$f-gen-1"},"meta":{}}
  JSON
  sed -i 's/^  //' jobs
  deny=$(bash ${../scripts/check-public-closure.sh} --print-deny)
  python3 -I ${../scripts/make-tools-index.py} --system aarch64-linux --deny="$deny" --offline < jobs > got
  {
    printf '#mortar-tools\t1\n'
    printf 'Upper\tupper-1\t%s\t\t\n' "$d"
    printf 'jq\tjq-1-bin\t%s\tjq\t\n' "$b"
    printf 'zed\tzed-1\t%s\tz\ttwo columns and lines\n' "$a"
  } > want
  diff want got

  if python3 -I ${../scripts/make-tools-index.py} --system aarch64-linux --deny="$deny" --offline --min 4 < jobs > /dev/null 2>&1; then
    echo "a list under --min wasn't refused"; exit 1
  fi
  touch $out
''
