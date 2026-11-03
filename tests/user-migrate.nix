# SPDX-License-Identifier: GPL-2.0-or-later
# The move from root's session to the session user (modules/core/user.nix), with
# and without the AI pack, in a fixture: core's own paths always move; Claude
# Code's move and get their paths fixed only with the pack that knows them.
{ runCommand, coreMigrate, aiMigrate }:

runCommand "mortar-user-migrate-check" { } ''
  run() {
    from=$PWD/$1/root; home=$PWD/$1/home
    mkdir -p $from/.claude/projects/-root-ai $from/Pictures $home
    echo '{"projects":{"/root/ai":{}},"x":"/root/ai/y"}' > $from/.claude.json
    echo '{"cwd":"/root/ai"}' > $from/.claude/projects/-root-ai/s.jsonl
    echo png > $from/Pictures/a.png
    MORTAR_MIGRATE_USER=$(id -u) MORTAR_MIGRATE_GROUP=$(id -g) MORTAR_MIGRATE_HOME=$home \
      MORTAR_MIGRATE_FROM=$from MORTAR_MIGRATE_STAMP=$PWD/$1/stamp MORTAR_MIGRATE_PLAY= "$2"
  }

  run core ${coreMigrate}/bin/mortar-user-migrate
  test -f core/home/Pictures/a.png
  test ! -e core/root/Pictures
  # core leaves Claude Code's files where they are
  test ! -e core/home/.claude.json
  test -f core/root/.claude.json

  run ai ${aiMigrate}/bin/mortar-user-migrate
  test -f ai/home/Pictures/a.png
  grep -q "\"$PWD/ai/home/ai\"" ai/home/.claude.json
  grep -q "$PWD/ai/home/ai/y" ai/home/.claude.json
  enc=$(printf %s "$PWD/ai/home" | tr / -)
  grep -q "\"cwd\":\"$PWD/ai/home/ai\"" "ai/home/.claude/projects/$enc-ai/s.jsonl"

  echo PASS > $out
''
