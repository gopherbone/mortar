# SPDX-License-Identifier: GPL-2.0-or-later
# zsh for the session user (modules/core/user.nix) in the shell's
# Terminal. Root keeps bash (consoles, SSH deploys), and scripts are not
# touched. Everything is system-wide (/etc/zshrc); the user's home is
# migrated data, so nothing is written there except zsh's own history and
# caches (and, once, the zsh history seeded from ~/.bash_history).
#
#   - completion: compinit with a cached dump that is rebuilt only when the
#     system generation (or fpath) changes; menu selection, case-insensitive
#   - zsh-autosuggestions (history, synchronous: no fork per key) and
#     zsh-syntax-highlighting (it redraws through zle-line-pre-redraw on
#     zsh 5.9, so it does not wrap every widget)
#   - history: 50k lines in ~/.local/state/zsh/history, shared between
#     terminals, no duplicates, lines starting with a space are not kept
#   - a hand-written prompt (no forks): directory, git branch (read from
#     .git/HEAD), the exit status, and a lime key cap (orange after an
#     error); how long the last command took on the right when over 3 s
#   - suggestion chips above the on-screen keyboard
#     (pkgs/mortar-shell/zsh/mortar-osk.zsh)
#   - eza for ls/ll/la/lt, ▲▼ search history by what is typed
#   - a one-line greeting in a new Terminal window (battery, uptime, a tip)
{ config, options, lib, pkgs, ... }:

let
  inherit (lib) mkOption types;
  cfg = config.mortar.zsh;
  hasUser = options ? mortar && options.mortar ? user;
  hasShell = options ? mortar && options.mortar ? shell;
  shellPkg = config.mortar.shell.package;

  # `dircolors -b` at build time instead of a fork in every shell
  lsColors = pkgs.runCommand "mortar-ls-colors" { } "${pkgs.coreutils}/bin/dircolors -b > $out";
in
{
  options.mortar.zsh = {
    enable = mkOption {
      type = types.bool;
      default = hasShell && config.mortar.shell.enable;
      defaultText = lib.literalExpression "config.mortar.shell.enable";
      description = "zsh as the session user's shell, with the Mortar prompt, completion and the OSK suggestion chips.";
    };
    eza = mkOption {
      type = types.bool;
      default = true;
      description = "eza for ls, ll, la and lt.";
    };
  };

  config = lib.mkIf cfg.enable {
    users.users = lib.optionalAttrs hasUser {
      ${config.mortar.user.name}.shell = pkgs.zsh;
    };

    environment.systemPackages = lib.optional cfg.eza pkgs.eza;

    programs.zsh = {
      enable = true;
      # no zsh-newuser-install menu for a home without ~/.zshrc
      shellInit = ''
        zsh-newuser-install() { :; }
      '';
      enableCompletion = true;
      enableGlobalCompInit = false; # cached below
      enableLsColors = false; # precomputed (lsColors)
      histSize = 50000;
      histFile = "\${XDG_STATE_HOME:-$HOME/.local/state}/zsh/history";
      setOptions = [
        "EXTENDED_HISTORY"
        "SHARE_HISTORY"
        "HIST_FCNTL_LOCK"
        "HIST_IGNORE_ALL_DUPS"
        "HIST_IGNORE_SPACE"
        "HIST_REDUCE_BLANKS"
        "HIST_VERIFY"
        "AUTO_CD"
        "AUTO_PUSHD"
        "PUSHD_IGNORE_DUPS"
        "PUSHD_SILENT"
        "INTERACTIVE_COMMENTS"
        "COMPLETE_IN_WORD"
        "NO_BEEP"
      ];
      autosuggestions = {
        enable = true;
        strategy = [ "history" ];
        # mid grey (WCAG 4.7:1 day, 5.1:1 night) and italic: readable, and
        # still clearly not typed text
        highlightStyle = "fg=8,italic";
        async = false;
        extraConfig = {
          ZSH_AUTOSUGGEST_BUFFER_MAX_SIZE = "80";
          ZSH_AUTOSUGGEST_MANUAL_REBIND = "1";
        };
      };
      syntaxHighlighting = {
        enable = true;
        highlighters = [ "main" ];
        styles = {
          # the default red for unknown commands is loud while typing
          unknown-token = "fg=red";
          comment = "fg=8";
        };
      };

      shellAliases = {
        ".." = "cd ..";
        "..." = "cd ../..";
      }
      // lib.optionalAttrs cfg.eza {
        ls = "eza --group-directories-first";
        l = "eza -la --group-directories-first";
        ll = "eza -l --group-directories-first --git --time-style=relative";
        la = "eza -la --group-directories-first --git --time-style=relative";
        lt = "eza --tree --level=2 --group-directories-first";
      }
      // lib.optionalAttrs (!cfg.eza) {
        ll = "ls -lh --group-directories-first";
        la = "ls -lAh --group-directories-first";
      };

      interactiveShellInit = lib.mkMerge [
        (lib.mkBefore ''
          source ${lsColors}

          # History: the directory, and the bash history once (migrated
          # homes have one).
          [[ -d ''${HISTFILE:h} ]] || mkdir -p ''${HISTFILE:h}
          if [[ ! -e $HISTFILE && -r $HOME/.bash_history ]]; then
            <$HOME/.bash_history >$HISTFILE
          fi

          # Completion. The dump is rebuilt (and checked, the slow part)
          # only when the system generation or fpath changed.
          zmodload zsh/complist
          autoload -Uz compinit
          () {
            local dir=''${XDG_CACHE_HOME:-$HOME/.cache}/zsh sys=/run/current-system
            local dump=$dir/zcompdump-$ZSH_VERSION stamp="''${sys:A} $fpath"
            [[ -d $dir ]] || mkdir -p $dir
            if [[ -r $dump && -r $dump.stamp && "$(<$dump.stamp)" == "$stamp" ]]; then
              compinit -C -d $dump
            else
              compinit -i -d $dump
              print -r -- "$stamp" >| $dump.stamp
              zcompile -U $dump
            fi
          }
          zstyle ':completion:*' menu select
          zstyle ':completion:*' matcher-list "" 'm:{a-z}={A-Za-z}'
          zstyle ':completion:*' list-colors ''${(s.:.)LS_COLORS}
          zstyle ':completion:*' group-name ""
          zstyle ':completion:*:descriptions' format '%F{8}%d%f'
          zstyle ':completion:*' use-cache on
          zstyle ':completion:*' cache-path ''${XDG_CACHE_HOME:-$HOME/.cache}/zsh/compcache

          # Up/Down (the D-pad) search history by what is typed
          autoload -Uz up-line-or-beginning-search down-line-or-beginning-search
          zle -N up-line-or-beginning-search
          zle -N down-line-or-beginning-search
          bindkey '^[[A' up-line-or-beginning-search '^[OA' up-line-or-beginning-search
          bindkey '^[[B' down-line-or-beginning-search '^[OB' down-line-or-beginning-search
          bindkey '^[[1;5C' forward-word '^[[1;5D' backward-word
        '')
        # after zsh-autosuggestions: the chips read its suggestion
        (lib.mkAfter ''
          ${lib.optionalString hasShell "source ${shellPkg}/share/mortar-shell/mortar-osk.zsh"}
        '')
      ];

      promptInit = ''
        # Mortar prompt: ~/dir branch ›  (all without forks)
        zmodload zsh/datetime
        typeset -gF _mortar_t0=0
        _mortar_preexec() { _mortar_t0=$EPOCHREALTIME }
        _mortar_precmd() {
          local e=$? d=$PWD h= b= took=
          while true; do
            if [[ -f $d/.git/HEAD ]]; then
              h=$(<$d/.git/HEAD)
            elif [[ -f $d/.git ]]; then # worktree or submodule: "gitdir: path"
              h=$(<$d/.git); h=''${h#gitdir: }
              [[ $h == /* ]] || h=$d/$h
              [[ -r $h/HEAD ]] && h=$(<$h/HEAD) || h=
            fi
            [[ -n $h || $d == / ]] && break
            d=''${d:h}
          done
          if [[ $h == 'ref: refs/heads/'* ]]; then b=''${h#ref: refs/heads/}
          elif [[ -n $h ]]; then b=''${h[1,7]}
          fi
          if (( _mortar_t0 > 0 )); then
            local -F s=$(( EPOCHREALTIME - _mortar_t0 ))
            local -i si=s
            if (( si >= 60 )); then took="$(( si / 60 ))m $(( si % 60 ))s"
            elif (( s >= 3 )); then printf -v took '%.1fs' $s
            fi
            _mortar_t0=0
          fi
          local host= branch=
          [[ -n ''${SSH_CONNECTION-} ]] && host='%F{3}%m%f '
          [[ -n $b ]] && branch=" %F{5}''${b//\%/%%}%f"
          PROMPT="$host%B%(4~|…/%3~|%~)%b$branch %(?..%F{1}%?%f )%(?.%K{#cef058}.%K{#ff9f67})%F{#1d1d1d} %(!.#.›) %f%k "
          RPROMPT=''${took:+%F{8}$took%f}
          return $e
        }
        autoload -Uz add-zsh-hook
        add-zsh-hook preexec _mortar_preexec
        add-zsh-hook precmd _mortar_precmd
        setopt prompt_sp

        # Greeting: a new Terminal window says hello (zsh started by foot
        # itself: not nested shells, tmux or SSH sessions).
        if [[ -n ''${MORTAR_APP-} && -r /proc/$PPID/comm && $(</proc/$PPID/comm) == foot ]]; then
          () {
            local up=$(</proc/uptime) bat= p line
            up=''${up%%.*}
            [[ $up == <-> ]] || up=0
            for p in /sys/class/power_supply/*(N); do
              [[ -r $p/type && $(<$p/type) == Battery && -r $p/capacity ]] && bat=$(<$p/capacity)% && break
            done
            line="%F{#cef058}▆▆%F{#cba6f7}▆▆%f %Bmortar%b%F{8}"
            [[ -n $bat ]] && line+=" · $bat"
            line+=" · up $(( up / 3600 ))h $(( up % 3600 / 60 ))m%f"
            local -a tips=(
              'START: keyboard · ▲ from its top row: suggestions · R1: the first one'
              'R1 on the keyboard types the grey suggestion from history'
              'screenfetch shows the system'
              'll lists with details · lt shows a tree'
              '▲ and ▼ search history for what is typed'
            )
            print -P -- "$line"
            print -P -- "%F{8}''${tips[RANDOM % $#tips + 1]}%f"
          }
        fi
      '';
    };
  };
}
