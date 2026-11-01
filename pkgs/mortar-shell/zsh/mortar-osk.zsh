# SPDX-License-Identifier: GPL-2.0-or-later
# Suggestion chips above mortar-shell's on-screen keyboard (osk_suggest.rs).
#
# Inside a terminal the shell started (MORTAR_APP set), zsh connects to
# $XDG_RUNTIME_DIR/mortar-osk.sock once. Nothing else runs until the shell
# asks: while the OSK is open over this terminal it sends "q SEQ" a moment
# after the last key it typed. The fd handler then runs zsh's own
# completion system for the word at the cursor with compadd captured (the
# technique fzf-tab uses: nothing is inserted or listed) and answers with
# one JSON line:
#
#   {"seq":SEQ,"chips":[{"k":KIND,"d":DEL,"t":TEXT,"l":LABEL},...]}
#
# KIND: h history (the zsh-autosuggestions suggestion), c command, d
# directory, f file, o option, a anything else. The shell types DEL
# backspaces, then TEXT, through the terminal's keyboard. While the OSK is
# closed the shell sends nothing, so this costs nothing per keystroke.
#
# Sourced from /etc/zshrc after compinit and zsh-autosuggestions.

[[ -o interactive && -n ${MORTAR_APP-} && -S ${XDG_RUNTIME_DIR-}/mortar-osk.sock ]] || return 0
zmodload zsh/net/socket zsh/system 2>/dev/null || return 0
zsocket ${XDG_RUNTIME_DIR}/mortar-osk.sock 2>/dev/null || return 0
typeset -gi _mosk_fd=$REPLY
typeset -ga _mosk_m      # captured matches: "kind<TAB>del<TAB>text<TAB>label"
typeset -gi _mosk_cmdpos # they complete the command word
typeset -gi MORTAR_OSK_MAX=${MORTAR_OSK_MAX:-12}
# Commands whose completion is too slow to run in every pause (it runs on
# the A53 between keys): they get the history chip only.
(( ${+MORTAR_OSK_SKIP} )) || typeset -ga MORTAR_OSK_SKIP=(nix nix-build nix-shell nix-env nixos-rebuild)

# JSON string body (no quotes): escape \ and ", drop control characters.
_mosk_js() {
  local s=${1//\\/\\\\}
  s=${s//\"/\\\"}
  REPLY=${s//[[:cntrl:]]/}
}

# compadd stand-in while _mosk-cap runs: record what would be added.
_mosk_compadd() {
  # queries (-O/-A/-D) are other completion code asking, not adding
  if (( ${@[(I)-[OAD]*]} )); then
    builtin compadd "$@"
    return
  fi
  local -a hits
  builtin compadd -O hits "$@"
  (( $#hits )) || { builtin compadd "$@"; return }
  local -A o
  zparseopts -E -A o P: p: S: s: i: I: W: f Q q U r: R: J: V: X: x: o:: d: l a k F: M: E: e n 1 2 C
  local pre=${o[-i]-}${o[-P]-}${o[-p]-} suf=${o[-s]-} kind=a
  # where -f matches live: -W (the directory) or else the hidden prefix
  local fdir=${(Q)${o[-p]-}}
  [[ -n ${o[-W]-} && ${o[-W]} != '('* ]] && fdir=${o[-W]%/}/
  case $curtag in
    (commands|builtins|aliases|functions|reserved-words|suffix-aliases|*-commands|commands-*) kind=c ;;
    (*option*) kind=o ;;
  esac
  local m full t del lbl k
  for m in $hits; do
    (( $#_mosk_m >= 400 )) && break
    k=$kind lbl=$m
    [[ $m == -* && $k == a ]] && k=o
    if (( ${+o[-Q]} )); then full=$pre$m; else full=$pre${(q)m}; fi
    if (( ${+o[-S]} )); then
      t=${o[-S]}
    elif (( ${+o[-f]} )) && [[ -d $fdir$m ]]; then
      t=/ k=d lbl=$m/
    elif (( ${+o[-f]} )); then
      t=' ' k=f
    elif [[ $k == o && $m == *= ]]; then
      t=
    else
      t=' '
    fi
    full+=$suf$t
    if (( ! ${+o[-U]} )) && [[ $full == "$PREFIX"* ]]; then
      del=0 full=${full#"$PREFIX"}
    else
      del=${#PREFIX}
    fi
    [[ -n ${full// } ]] || continue
    _mosk_m+=("$k"$'\t'"$del"$'\t'"$full"$'\t'"$lbl")
  done
  builtin compadd "$@"
}

# The completion widget: the real completion with compadd captured; it
# inserts and lists nothing.
_mosk_capfn() {
  _mosk_m=()
  compstate[insert]='' compstate[list]=''
  # the empty command word would be every command: not useful
  (( CURRENT == 1 )) && [[ -z $PREFIX ]] && return 1
  (( CURRENT > 1 && ${MORTAR_OSK_SKIP[(I)${words[1]-}]} )) && return 1
  (( CURRENT == 1 )) && _mosk_cmdpos=1
  functions[compadd]='_mosk_compadd "$@"'
  {
    _main_complete
  } always {
    unfunction compadd 2>/dev/null
  }
  compstate[insert]='' compstate[list]='' compstate[old_list]=''
  return 0
}
zle -C _mosk-cap complete-word _mosk_capfn

# REPLY = the JSON answer for the line as it is now.
_mosk_answer() {
  local seq=$1 c f t l
  local -a chips keyed parts fs
  local -A seen
  # 1. the history suggestion (zsh-autosuggestions keeps it in POSTDISPLAY)
  if [[ -n $POSTDISPLAY && $CURSOR -eq $#BUFFER ]]; then
    chips+=("h"$'\t'"0"$'\t'"$POSTDISPLAY"$'\t'"$BUFFER$POSTDISPLAY")
  fi
  # 2. completions for the word at the cursor, no repeats. Command names
  # come shortest first (closest to what is typed); everything else keeps
  # the completion's own order (git: the everyday subcommands first).
  if [[ -n $BUFFER ]]; then
    _mosk_cmdpos=0
    # the completion widget redraws the line without its highlighting (the
    # grey suggestion, syntax colours): put it back and redraw
    local -a rh=("${region_highlight[@]}")
    zle _mosk-cap 2>/dev/null
    region_highlight=("${rh[@]}")
    zle -R
    integer i=0
    for c in $_mosk_m; do
      fs=("${(@ps:\t:)c}")
      f=$fs[2]:$fs[3]
      (( ${+seen[$f]} )) && continue
      seen[$f]=1
      (( i++ ))
      if (( _mosk_cmdpos )); then
        keyed+=("${(l:4::0:)#fs[4]}${(l:4::0:)i}"$'\t'"$c")
      else
        keyed+=("${(l:4::0:)i}"$'\t'"$c")
      fi
    done
    for c in ${(o)keyed}; do
      (( $#chips >= MORTAR_OSK_MAX )) && break
      chips+=("${c#*$'\t'}")
    done
  fi
  for c in $chips; do
    fs=("${(@ps:\t:)c}")
    _mosk_js "$fs[3]"; t=$REPLY
    _mosk_js "$fs[4]"; l=$REPLY
    parts+=("{\"k\":\"$fs[1]\",\"d\":$fs[2],\"t\":\"$t\",\"l\":\"$l\"}")
  done
  REPLY="{\"seq\":$seq,\"chips\":[${(j:,:)parts}]}"
}

# fd handler (a widget, so it may run completion): answer the newest "q".
_mosk-read() {
  local fd=$1 buf= chunk q= line
  if [[ -n ${2-} ]]; then   # hup / error: the shell went away
    zle -F $fd; exec {fd}>&-
    return 0
  fi
  while :; do
    sysread -i $fd -t 0 chunk
    case $? in
      (0) buf+=$chunk ;;
      (5) zle -F $fd; exec {fd}>&-; return 0 ;;   # EOF
      (*) break ;;
    esac
  done
  for line in ${(f)buf}; do
    [[ $line == 'q '<-> ]] && q=${line#q }
  done
  [[ -n $q ]] || return 0
  if (( PENDING || KEYS_QUEUED_COUNT )); then
    # keys are still coming: ask again after them
    print -r -u $fd -- "{\"seq\":$q,\"retry\":true}" 2>/dev/null
    return 0
  fi
  _mosk_answer $q
  print -r -u $fd -- "$REPLY" 2>/dev/null || { zle -F $fd; exec {fd}>&- }
  return 0
}
zle -N _mosk-read
zle -F -w $_mosk_fd _mosk-read

# zsh-autosuggestions must not wrap these widgets (it would refetch).
(( ${+ZSH_AUTOSUGGEST_IGNORE_WIDGETS} )) && ZSH_AUTOSUGGEST_IGNORE_WIDGETS+=(_mosk-read _mosk-cap)
return 0
