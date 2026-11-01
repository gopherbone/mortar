# SPDX-License-Identifier: GPL-2.0-or-later
# Mortar shell: a controller-driven Wayland compositor (pkgs/mortar-shell)
# that runs several apps at once. Every app is fullscreen; MENU opens a
# MENU over the app (Running, then the cross: Apps, Games, Music, Settings) that
# switches, closes and launches apps and holds quick settings. Background apps keep
# running (the M8 keeps playing while you use the terminal).
#
# Off by default: importing this module changes nothing until
# `mortar.shell.enable = true`.
#
# With the greeter (modules/mortar-greeter.nix) the shell is one more greeter
# session, "Mortar", sorted first. By default every other greeter session
# also becomes a launcher entry inside the shell (except console-only ones,
# see `excludeGreeterSessions`), so the app list is defined once. With
# `exclusive = true` the greeter offers only the shell.
#
# Without the greeter, start it from any logind session on a VT:
# `mortar-shell-session` (it needs a seat: logind or seatd via libseat).
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib) mkOption mkEnableOption types;
  cfg = config.mortar.shell;
  greeter = config.mortar.greeter;

  icons = [ "wifi" "bat" "usb" "cable" "file" "skip" "term" "folder" "pad" "gear" "link" "check" "lock" "power" "claude" ];

  appType = types.submodule (
    { name, ... }:
    {
      options = {
        name = mkOption {
          type = types.str;
          default = name;
          description = "Name on the launcher tile and in the MENU.";
        };
        description = mkOption {
          type = types.str;
          default = "";
          description = "Second line on the tile.";
        };
        icon = mkOption {
          type = types.enum icons;
          default = "pad";
          description = "Line icon from the Mortar UI set (`claude`: the Claude Spark, for Claude apps only).";
        };
        art = mkOption {
          type = types.nullOr (types.enum arts);
          default = null;
          description = ''
            Isometric illustration on the home tile (and the icon in the
            MENU; `null`: guessed from the id, name and icon).
          '';
        };
        command = mkOption {
          type = types.either types.str (types.listOf types.str);
          description = ''
            What to run: an argv list, or a shell command string (run with
            `sh -c`). Apps get WAYLAND_DISPLAY, SDL_VIDEO_DRIVER=wayland and
            their own virtual gamepad.
          '';
        };
        env = mkOption {
          type = types.attrsOf types.str;
          default = { };
          description = "Extra environment for this app.";
        };
        gamepad = mkOption {
          type = types.enum [ "forward" "keyboard" "none" ];
          default = "forward";
          description = ''
            How the app gets the Brick's buttons while it is on screen:
            `forward` (it reads a gamepad itself, e.g. SDL), `keyboard` (the
            pad types keys and START opens the on-screen keyboard; for
            terminals) or `none`. MENU always belongs to the shell.
          '';
        };
        singleInstance = mkOption {
          type = types.bool;
          default = true;
          description = "Starting it again switches to the running copy.";
        };
        category = mkOption {
          type = types.nullOr (types.enum categories);
          default = null;
          description = ''
            Where it shows in the MENU besides Apps (which lists every
            entry): `games` or `music` (`apps`: only under Apps). `null`:
            `mortar.shell.categories.<id>`, else guessed from the
            illustration and name (M8 and trackers are music; PICO-8,
            PuzzleScript and emulators are games).
          '';
        };
        order = mkOption {
          type = types.int;
          default = 100;
          description = "Sort key (then id) on the launcher.";
        };
        requires = mkOption {
          type = types.nullOr types.str;
          default = null;
          example = "/nix/var/nix/profiles/pico8/bin/pico8";
          description = "Only offer it while this path exists (an app installed outside the system, e.g. PICO-8).";
        };
      };
    }
  );

  arts = [ "m8" "pico8" "games" "claude" "puzzle" "term" "generic" ];
  categories = [ "apps" "games" "music" ];

  argv = c: if builtins.isString c then [ "${pkgs.runtimeShell}" "-c" c ] else c;

  footIni = pkgs.writeText "mortar-foot.ini" ''
    # Mortar terminal: Geist Mono in the Pond palette. [colors-light] is
    # the day ground with ink text (the regular ANSI colours are deep
    # versions of the accents so they read on it; the bright ones are the
    # accents themselves, for blocks such as the zsh prompt's lime key
    # cap); [colors-dark] the night surface. Which one: mortarTerminal.
    font=Geist Mono:size=${toString cfg.terminal.fontSize}
    pad=16x12 center
    initial-color-theme=dark
    [cursor]
    style=block
    blink=no
    [colors-light]
    background=e1ebf7
    foreground=1d1d1d
    cursor=1d1d1d cef058
    selection-background=cba6f7
    selection-foreground=1d1d1d
    regular0=1d1d1d
    regular1=c4452a
    regular2=4d7a12
    regular3=946200
    regular4=2f5fb3
    regular5=7a3fc4
    regular6=1d7a6e
    regular7=bfcbd8
    bright0=7d8799
    bright1=ff9f67
    bright2=cef058
    bright3=ffd330
    bright4=8fb4ff
    bright5=cba6f7
    bright6=7fd6c8
    bright7=ffffff
    [colors-dark]
    cursor=1d1d1d cef058
    background=1d1d1d
    foreground=e1ebf7
    selection-background=cef058
    selection-foreground=1b322d
    regular0=1d1d1d
    regular1=ff9f67
    regular2=cef058
    regular3=ffd330
    regular4=8fb4ff
    regular5=cba6f7
    regular6=7fd6c8
    regular7=e1ebf7
    bright0=4c5164
    bright1=ffb78c
    bright2=dcf58a
    bright3=ffdf66
    bright4=b0caff
    bright5=dcc2fa
    bright6=a3e3d8
    bright7=ffffff
  '';

  # foot with the Mortar config. Entries with MORTAR_FOLLOW_PALETTE in their
  # env (the Terminal) start in the shell's Settings > Palette (MORTAR_PALETTE,
  # set by the shell) and follow it when it changes (the shell sends foot
  # SIGUSR1/SIGUSR2); the others stay on the night surface (Claude Code's
  # TUI is themed for it).
  mortarTerminal = pkgs.writeShellScript "mortar-terminal" ''
    theme=dark
    if [ -n "''${MORTAR_FOLLOW_PALETTE-}" ] && [ "''${MORTAR_PALETTE-}" = day ]; then theme=light; fi
    exec ${lib.getExe cfg.terminal.package} --config ${footIni} -o initial-color-theme="$theme" "$@"
  '';

  terminalApp = {
    name = "Terminal";
    description = "foot · START for keyboard";
    icon = "term";
    gamepad = "keyboard";
    # Opening Terminal switches to the one already running; "New terminal"
    # (below) starts another.
    singleInstance = true;
    order = 5;
    command = cfg.terminal.command;
    env.MORTAR_FOLLOW_PALETTE = "1";
    art = "term";
    category = "apps";
  };

  fromGreeter = lib.mapAttrs (
    id: s: {
      inherit (s) name description icon env order requires;
      art = null;
      category = null;
      command = s.command;
      gamepad = "forward";
      singleInstance = true;
    }
  ) (lib.filterAttrs (id: _: id != "mortar" && !(lib.elem id cfg.excludeGreeterSessions)) greeter.sessions);

  allApps =
    (lib.optionalAttrs (cfg.importGreeterSessions && greeter.enable) fromGreeter)
    // (lib.optionalAttrs cfg.terminal.enable {
      terminal = terminalApp;
      terminal-new = terminalApp // {
        name = "New terminal";
        description = "Another foot window";
        singleInstance = false;
        order = 6;
      };
    })
    // cfg.apps;

  sorted = lib.sort (a: b: if a.order == b.order then a.id < b.id else a.order < b.order) (
    lib.mapAttrsToList (id: a: a // { inherit id; }) allApps
  );

  configFile = pkgs.writeText "mortar-shell.json" (
    builtins.toJSON {
      apps = map (
        a:
        let
          category = if a.category or null != null then a.category else cfg.categories.${a.id} or null;
        in
        {
          inherit (a) id name description icon env gamepad singleInstance;
          command = argv a.command;
        }
        // lib.optionalAttrs (a.art or null != null) { inherit (a) art; }
        // lib.optionalAttrs (category != null) { inherit category; }
        // lib.optionalAttrs (a.requires or null != null) { inherit (a) requires; }
      ) sorted;
      ui = {
        inherit (cfg.ui) renderer motion reduceMotion sounds soundVolume haptics bootAnimation screenshotDir cardFps msaa audioDevice suspend suspendGraceMs idleMinutes;
        hapticMs = cfg.ui.hapticMs;
        palette = if cfg.ui.palette != null then cfg.ui.palette else (if cfg.theme == "dark" then "night" else "day");
        # the stock PowerVR libs for the GPU UI renderer (modules/brick-gpu.nix)
        glLibDir = if (config.mortar.gpu.enable or false) then "${config.mortar.gpu.package}/lib" else null;
      };
      inherit (cfg) autostart backgroundFps theme keyboardLayout;
      # SDL apps started through mortar-sdl-run (modules/brick-gpu.nix)
      # must not take the GPU's display path (brick_dc_drm drives the CRTC,
      # and the shell owns it). With gpuApps they render on the GPU as
      # Wayland clients (mortar-wlegl dma-bufs, scanned out directly),
      # otherwise on software GL.
      env = { MORTAR_SDL = if cfg.gpuApps then "wayland-gpu" else "software"; } // cfg.environment;
      # Volume keys: PipeWire's default sink (codec, USB or Bluetooth), or
      # the codec's DAC without PipeWire (the shell's built-in default).
      actions = lib.optionalAttrs config.services.pipewire.enable {
        volumeSet = [ "${pkgs.wireplumber}/bin/wpctl" "set-volume" "-l" "1.0" "@DEFAULT_AUDIO_SINK@" "{step}" ];
        volumeGet = [ "${pkgs.wireplumber}/bin/wpctl" "get-volume" "@DEFAULT_AUDIO_SINK@" ];
        volumeMute = [ "${pkgs.wireplumber}/bin/wpctl" "set-mute" "@DEFAULT_AUDIO_SINK@" "{mute}" ];
        # MENU > Music / Settings > Mixer: each app's output streams
        pwDump = [ "${config.services.pipewire.package}/bin/pw-dump" ];
        streamVolume = [ "${pkgs.wireplumber}/bin/wpctl" "set-volume" "{id}" "{vol}" ];
        streamMute = [ "${pkgs.wireplumber}/bin/wpctl" "set-mute" "{id}" "{mute}" ];
      };
      # MENU > Settings > Wi-Fi / Bluetooth, More settings > Remote access /
      # Bluetooth devices (modules/mortar-setup.nix)
      setupSocket = if config.mortar ? setup && config.mortar.setup.enable then "/run/mortar-setup/control.sock" else null;
      # Settings > Updates (modules/mortar-update.nix)
      update = if config.mortar ? update && config.mortar.update.enable then config.mortar.update.shell else null;
    }
  );

  session = pkgs.writeShellScriptBin "mortar-shell-session" ''
    # The Mortar shell as a login session (greetd, or any logind session
    # on a VT). Logs: journalctl -t mortar-shell
    export XDG_SESSION_TYPE=wayland XDG_CURRENT_DESKTOP=mortar
    exec ${pkgs.systemd}/bin/systemd-cat -t mortar-shell --level-prefix=false \
      ${lib.getExe cfg.package} run --config ${configFile} "$@"
  '';

  shellSession = {
    name = "Mortar";
    description = "All apps · MENU switches";
    icon = "pad";
    order = 0;
    command = [ "${session}/bin/mortar-shell-session" ];
  };

  backlightRule = ''
    ACTION=="add", SUBSYSTEM=="backlight", RUN+="${pkgs.coreutils}/bin/chgrp video /sys/class/backlight/%k/brightness /sys/class/backlight/%k/bl_power", RUN+="${pkgs.coreutils}/bin/chmod g+w /sys/class/backlight/%k/brightness /sys/class/backlight/%k/bl_power"
  '';

  profileUsers = if greeter.enable then map (p: p.user) (lib.attrValues greeter.profiles) else [ ];
in
{
  options.mortar.shell = {
    enable = mkEnableOption "the Mortar shell (multitasking Wayland compositor)";

    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ../pkgs/mortar-shell { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/mortar-shell { }";
      description = "The mortar-shell package.";
    };

    apps = mkOption {
      type = types.attrsOf appType;
      default = { };
      example = lib.literalExpression ''
        {
          m8 = { name = "M8"; icon = "usb"; command = "exec m8c-brick"; };
          htop = { name = "htop"; icon = "gear"; gamepad = "keyboard";
                   command = [ "''${pkgs.foot}/bin/foot" "htop" ]; };
        }
      '';
      description = "Launcher entries, keyed by id (added to the imported greeter sessions and the terminal).";
    };

    categories = mkOption {
      type = types.attrsOf (types.enum categories);
      default = { };
      example = { m8 = "music"; pico8 = "games"; };
      description = ''
        MENU category by app id, for entries that do not set `category`
        themselves (e.g. the imported greeter sessions).
      '';
    };

    importGreeterSessions = mkOption {
      type = types.bool;
      default = true;
      description = "Offer every other mortar.greeter session as a launcher entry inside the shell.";
    };

    excludeGreeterSessions = mkOption {
      type = types.listOf types.str;
      default = [ "terminal" ];
      description = ''
        Greeter sessions not to import: console programs cannot run inside
        a compositor (the shell's own Terminal entry replaces a console
        session).
      '';
    };

    terminal = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Add a foot terminal to the launcher (keyboard gamepad mode, on-screen keyboard).";
      };
      package = mkOption {
        type = types.package;
        default = pkgs.foot;
        defaultText = lib.literalExpression "pkgs.foot";
        description = "The terminal.";
      };
      fontSize = mkOption {
        type = types.ints.positive;
        default = 13;
        description = "Terminal font size (points; the panel is 1024x768 at about 200 dpi).";
      };
      command = mkOption {
        type = types.listOf types.str;
        readOnly = true;
        default = [ "${mortarTerminal}" ];
        defaultText = lib.literalExpression ''[ "<foot --config Mortar foot.ini>" ]'';
        description = "The terminal with the Mortar config, as an argv; append a command to run it in the terminal (other modules' launcher entries use this).";
      };
    };

    autostart = mkOption {
      type = types.listOf types.str;
      default = [ ];
      example = [ "m8" ];
      description = "App ids to start with the shell; the first one is shown.";
    };

    backgroundFps = mkOption {
      type = types.ints.unsigned;
      default = 2;
      description = "Frame callbacks per second for apps that run in the background (0: none).";
    };

    theme = mkOption {
      type = types.enum [ "light" "dark" ];
      default = "light";
      description = "Theme of the shell's own screens (mortar-ui style v2).";
    };

    ui = {
      renderer = mkOption {
        type = types.enum [ "auto" "gpu" "cpu" ];
        default = "auto";
        description = ''
          What draws the shell's own screens (home, MENU, transitions,
          OSD, toasts): `auto`/`gpu` use GLES on the PowerVR when
          `mortar.gpu.enable` and it initialises (falling back to the CPU on
          any error), `cpu` always draws with tiny-skia (slower animations).
          `MORTAR_SHELL_RENDERER=cpu|gpu` overrides it for one run.
        '';
      };
      motion = mkOption {
        type = types.enum [ "bouncy" "calm" "snappy" ];
        default = "bouncy";
        description = "Default spring preset (MENU > Settings > More settings > Motion changes it per user).";
      };
      reduceMotion = mkOption {
        type = types.bool;
        default = false;
        description = "Default for Settings > Reduce motion: no overshoot, no drifting background.";
      };
      sounds = mkOption {
        type = types.bool;
        default = true;
        description = "Default for Settings > UI sounds.";
      };
      soundVolume = mkOption {
        type = types.ints.between 0 100;
        default = 60;
        description = "UI sound volume (percent of full scale, before the system volume).";
      };
      audioDevice = mkOption {
        type = types.str;
        default = "default";
        description = "ALSA PCM for UI sounds (the codec's dmix by default, see mortar-apps.nix).";
      };
      haptics = mkOption {
        type = types.bool;
        default = true;
        description = "Default for Settings > Haptic ticks (the gpio-vibrator rumble motor).";
      };
      hapticMs = mkOption {
        type = types.submodule {
          options = {
            tick = mkOption { type = types.ints.positive; default = 8; description = "Pulse on moves (ms)."; };
            double = mkOption { type = types.ints.positive; default = 18; description = "Each of the two pulses on launch/close (ms)."; };
            gap = mkOption { type = types.ints.positive; default = 72; description = "Gap between the double pulses (ms)."; };
            long = mkOption { type = types.ints.positive; default = 60; description = "Pulse on screenshots (ms)."; };
          };
        };
        default = { };
        description = "Haptic pulse lengths; the motor is on/off, so length is the feel.";
      };
      palette = mkOption {
        type = types.nullOr (types.enum [ "day" "night" ]);
        default = null;
        description = "Default for Settings > Palette (null: from `theme`).";
      };
      suspend = mkOption {
        type = types.bool;
        default = config.hardware.trimui-brick.suspend.enable or false;
        defaultText = lib.literalExpression "config.hardware.trimui-brick.suspend.enable";
        description = ''
          Default for Settings > Sleep (Suspend / Screen off). Sleep (power
          key tap, MENU > Settings > Sleep, the power menu) blanks the
          screen and backlight and occludes the front app; with Suspend it
          also runs `actions.sleep` (`systemctl suspend`) once the screen
          has been dark for `suspendGraceMs`. Wake sources: see
          hardware.trimui-brick.suspend (s2idle: any button; deep: power
          key). Apps are frozen meanwhile (audio stops).
        '';
      };
      suspendGraceMs = mkOption {
        type = types.ints.unsigned;
        default = 3000;
        description = "How long the screen stays dark before suspending (a power tap within it just wakes the screen).";
      };
      idleMinutes = mkOption {
        type = types.ints.unsigned;
        default = 0;
        description = "Default for Settings > Sleep when idle: minutes without button input before sleeping (0: never).";
      };
      bootAnimation = mkOption {
        type = types.bool;
        default = true;
        description = "Play the boot screen (blocks, wordmark, chime) when the shell starts.";
      };
      screenshotDir = mkOption {
        type = types.str;
        default = "~/Pictures/Screenshots";
        description = "Where VOL+ and VOL- pressed together save screenshots.";
      };
      cardFps = mkOption {
        type = types.ints.unsigned;
        default = 15;
        description = "Frame callbacks per second for the app whose live preview shows in the MENU (Running).";
      };
      msaa = mkOption {
        type = types.enum [ 0 4 ];
        default = 4;
        description = "Multisampling for the GPU UI (EXT_multisampled_render_to_texture, if the driver has it).";
      };
    };

    keyboardLayout = mkOption {
      type = types.str;
      default = "us";
      description = "xkb layout for keyboards and the on-screen keyboard.";
    };

    environment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      description = "Environment for every app.";
    };

    gpuApps = mkOption {
      type = types.bool;
      default = config.mortar.gpu.enable or false;
      defaultText = lib.literalExpression "config.mortar.gpu.enable";
      description = ''
        Run SDL apps started through `mortar-sdl-run` on the PowerVR as
        Wayland clients (pkgs/mortar-wlegl; the shell scans their buffers
        out without a copy), falling back to software GL if that fails.
        Off: software GL (Mesa llvmpipe) only.
      '';
    };

    exclusive = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Make the shell the greeter's only session. The other greeter
        sessions are then gone, so define the apps in `mortar.shell.apps`
        (importGreeterSessions sees only the shell).
      '';
    };

    kernelUinput = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Build the kernel with CONFIG_INPUT_UINPUT=m (per-app virtual
        gamepads need /dev/uinput; the Brick defconfig leaves it off). Turn
        off once the kernel package enables it itself.
      '';
    };

    configFile = mkOption {
      type = types.path;
      readOnly = true;
      default = configFile;
      description = "The generated shell config (also at /etc/mortar-shell/config.json).";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        assertions = map (id: {
          assertion = allApps ? ${id};
          message = "mortar.shell.autostart: unknown app '${id}'.";
        }) cfg.autostart;

        environment.etc."mortar-shell/config.json".source = configFile;
        environment.systemPackages = [ cfg.package session ] ++ lib.optional cfg.terminal.enable cfg.terminal.package;

        # Per-app virtual gamepads (uinput): the uinput group may use
        # /dev/uinput. Session users also need the input and video groups
        # when logind's seat ACLs don't apply (e.g. root over seatd).
        # (Group members, not users.users: the greeter's default profiles are
        # computed from users.users.)
        hardware.uinput.enable = true;
        users.groups = lib.genAttrs [ "uinput" "input" "video" "audio" ] (_: {
          members = lib.filter (u: u != "root") profileUsers;
        });
        boot.kernelPatches = lib.optional cfg.kernelUinput {
          name = "mortar-shell-uinput";
          patch = null;
          structuredExtraConfig.INPUT_UINPUT = lib.kernel.module;
        };

        # foot and other apps find the Mortar fonts (Geist Mono) here.
        fonts.fontconfig.enable = lib.mkDefault true;
        fonts.packages = [ cfg.package.fonts ];

        # The power key belongs to the shell: a tap sleeps/wakes the screen,
        # a hold opens its power menu. logind must not power off on it (its
        # default) or on idle/lid. The PMIC's own long-hold force-off stays
        # as the hardware emergency path.
        services.logind.settings.Login = {
          HandlePowerKey = lib.mkDefault "ignore";
          HandlePowerKeyLongPress = lib.mkDefault "ignore";
          HandleSuspendKey = lib.mkDefault "ignore";
          HandleHibernateKey = lib.mkDefault "ignore";
          HandleLidSwitch = lib.mkDefault "ignore";
          HandleLidSwitchExternalPower = lib.mkDefault "ignore";
          HandleLidSwitchDocked = lib.mkDefault "ignore";
          IdleAction = lib.mkDefault "ignore";
        };

        # Brightness from MENU > Settings, and the backlight off while asleep
        # (bl_power), for the video group (same rule as the greeter).
        services.udev.extraRules = backlightRule;
      }
      (lib.mkIf (greeter.enable && !cfg.exclusive) {
        mortar.greeter.sessions.mortar = shellSession;
      })
      (lib.mkIf (greeter.enable && cfg.exclusive) {
        mortar.greeter.sessions = lib.mkForce { mortar = shellSession; };
      })
    ]
  );
}
