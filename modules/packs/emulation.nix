# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-play: emulators through libretro, without RetroArch's UI
# (pkgs/mortar-play). A curated core per system, preconfigured here: core
# option presets, controls, scaling. Nothing to set up on the device: copy
# games in and play.
#
#   ROMs   /var/lib/mortar-play/roms/<folder>/  and  ~/Games/<folder>/
#          (.zip / .7z work too; arcade sets stay zipped)
#   BIOS   /var/lib/mortar-play/bios/   (never shipped; the UI says what is
#          missing, e.g. scph5501.bin for the PlayStation)
#   Saves  /var/lib/mortar-play/saves/<system>/, states in .../states/
#
# Opt-in: importing this module turns it on. Every game runs as its own mortar-shell app
# (MENU > Games lists the library; `mortar.shell.games`), it adds a "Games"
# launcher entry (mortar-play's own library, "All systems") to
# mortar-shell (and a greeter session when the greeter is on; it then runs
# on the PowerVR GPU through mortar-sdl-run).
#
# Several of the best cores for a 2 GHz A53 are "unfree redistributable"
# (non-commercial licences): snes9x2005-plus, genesis-plus-gx, fbneo. They are
# in `mortar.unfree` below (`mortar.play.unfreeCores` lists what is enabled);
# changing a system's core may need another entry there.
{
  config,
  lib,
  pkgs,
  options,
  ...
}:

let
  inherit (lib) mkOption mkEnableOption types;
  cfg = config.mortar.play;
  owner = "${cfg.user} ${config.users.users.${cfg.user}.group}";
  lr = pkgs.libretro;

  coreFile = pkg: "${pkg}/lib/retroarch/cores/${lib.replaceStrings [ "-" ] [ "_" ] pkg.core}_libretro${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}";

  biosType = types.submodule {
    options = {
      files = mkOption { type = types.listOf types.str; description = "Any one of these files (case-insensitive) satisfies the entry."; };
      required = mkOption { type = types.bool; default = false; description = "Games can't start without it (else the UI warns once)."; };
      description = mkOption { type = types.str; default = ""; };
    };
  };

  systemType = types.submodule (
    { name, ... }:
    {
      options = {
        enable = mkOption { type = types.bool; default = true; };
        name = mkOption { type = types.str; default = name; description = "Name in the library."; };
        tag = mkOption { type = types.str; default = lib.toUpper name; description = "Short tag (pill), e.g. GBA."; };
        folders = mkOption { type = types.listOf types.str; default = [ name ]; description = "Folder names under each ROM root."; };
        extensions = mkOption { type = types.listOf types.str; default = [ ]; description = "ROM file extensions (lowercase, no dot); zip/7z are always listed."; };
        core = mkOption { type = types.package; description = "The libretro core (from pkgs.libretro)."; };
        options = mkOption { type = types.attrsOf types.str; default = { }; description = "Core option presets (key = value)."; };
        scaling = mkOption {
          type = types.enum [ "smooth" "sharp" "sharp-bilinear" "integer" ];
          default = "sharp";
          description = ''
            The default picture; Display in the in-game menu and the shell's
            game panel overrides it per game or per system (kept in the
            state directory). smooth: bilinear (inside the shell the display
            engine scales the game's own frame: no CPU); sharp: nearest
            neighbour to the full size; sharp-bilinear: nearest to the largest
            whole multiple, then a smooth finish (crisp and even); integer:
            whole multiples only, black borders (pixel perfect).
          '';
        };
        aspect = mkOption {
          type = types.enum [ "fit" "stretch" ];
          default = "fit";
          description = "fit: the core's display aspect ratio; stretch: the whole screen (integer: whole multiples on each axis).";
        };
        controls = mkOption {
          type = types.attrsOf types.str;
          default = { };
          example = { x = "a"; y = "b"; };
          description = "Brick button -> RetroPad button (or \"none\"). Unlisted buttons keep their RetroPad position.";
        };
        fastForward = mkOption { type = types.str; default = ""; description = "Hold-to-fast-forward button for this system (\"\" = the global one, \"none\" = off)."; };
        bios = mkOption { type = types.listOf biosType; default = [ ]; };
        keepArchives = mkOption { type = types.bool; default = false; description = "Hand .zip/.7z to the core unextracted (arcade sets)."; };
        noContent = mkOption { type = types.bool; default = false; description = "The core runs without a game file."; };
        order = mkOption { type = types.int; default = 100; };
      };
    }
  );

  # The curated set. Option keys/values checked against the cores' sources
  # (libretro_core_options.h) in the pinned nixpkgs and by `mortar.play.check`
  # (which loads every core); presets the core does not know are logged and
  # ignored at runtime.
  curated = {
    gb = {
      name = "Game Boy";
      tag = "GB";
      folders = [ "gb" ];
      extensions = [ "gb" "dmg" ];
      core = lr.gambatte;
      scaling = "integer";
      options = { gambatte_gb_colorization = "auto"; gambatte_mix_frames = "disabled"; gambatte_gb_bootloader = "disabled"; };
      order = 10;
    };
    gbc = {
      name = "Game Boy Color";
      tag = "GBC";
      folders = [ "gbc" ];
      extensions = [ "gbc" ];
      core = lr.gambatte;
      scaling = "integer";
      options = { gambatte_mix_frames = "disabled"; gambatte_gb_bootloader = "disabled"; gambatte_gbc_color_correction = "GBC only"; };
      order = 11;
    };
    gba = {
      name = "Game Boy Advance";
      tag = "GBA";
      folders = [ "gba" ];
      extensions = [ "gba" ];
      core = lr.mgba;
      scaling = "integer";
      options = {
        mgba_skip_bios = "ON";
        mgba_idle_optimization = "Remove Known";
        mgba_frameskip = "disabled";
        mgba_color_correction = "OFF";
        mgba_interframe_blending = "OFF";
        mgba_audio_low_pass_filter = "disabled";
      };
      bios = [ { files = [ "gba_bios.bin" ]; description = "GBA BIOS (optional, mGBA has its own)"; } ];
      order = 12;
    };
    nes = {
      name = "NES";
      tag = "NES";
      folders = [ "nes" "fc" ];
      extensions = [ "nes" "fds" "unf" "unif" ];
      core = lr.fceumm;
      options = { fceumm_sndquality = "Low"; fceumm_aspect = "4:3"; fceumm_overscan_v_top = "8"; fceumm_overscan_v_bottom = "8"; };
      bios = [ { files = [ "disksys.rom" ]; description = "Famicom Disk System BIOS (only for .fds)"; } ];
      order = 20;
    };
    snes = {
      name = "Super Nintendo";
      tag = "SNES";
      folders = [ "snes" "sfc" ];
      extensions = [ "sfc" "smc" "fig" "swc" ];
      core = lr.snes9x2005-plus;
      options = { snes9x_2005_frameskip = "disabled"; snes9x_2005_reduce_sprite_flicker = "disabled"; };
      order = 21;
    };
    md = {
      name = "Mega Drive";
      tag = "MD";
      folders = [ "md" "genesis" "megadrive" ];
      extensions = [ "md" "gen" "smd" "bin" "68k" "sgd" ];
      core = lr.genesis-plus-gx;
      options = {
        genesis_plus_gx_render = "single field";
        genesis_plus_gx_blargg_ntsc_filter = "disabled";
        genesis_plus_gx_audio_filter = "disabled";
        genesis_plus_gx_ym2612 = "mame (ym2612)";
        genesis_plus_gx_overscan = "disabled";
        genesis_plus_gx_left_border = "disabled";
      };
      order = 30;
    };
    sms = {
      name = "Master System";
      tag = "SMS";
      folders = [ "sms" "mastersystem" ];
      extensions = [ "sms" ];
      core = lr.genesis-plus-gx;
      options = { genesis_plus_gx_audio_filter = "disabled"; genesis_plus_gx_overscan = "disabled"; };
      order = 31;
    };
    gg = {
      name = "Game Gear";
      tag = "GG";
      folders = [ "gg" "gamegear" ];
      extensions = [ "gg" ];
      core = lr.genesis-plus-gx;
      scaling = "integer";
      options = { genesis_plus_gx_audio_filter = "disabled"; genesis_plus_gx_gg_extra = "disabled"; genesis_plus_gx_lcd_filter = "disabled"; };
      order = 32;
    };
    pce = {
      name = "PC Engine";
      tag = "PCE";
      folders = [ "pce" "tg16" "pcecd" ];
      extensions = [ "pce" "sgx" "cue" "ccd" "chd" ];
      core = lr.beetle-pce-fast;
      options = { pce_fast_cdimagecache = "disabled"; pce_fast_frameskip = "disabled"; };
      bios = [ { files = [ "syscard3.pce" ]; description = "PC Engine CD System Card (only for CD games)"; } ];
      order = 40;
    };
    psx = {
      name = "PlayStation";
      tag = "PS1";
      folders = [ "psx" "ps1" ];
      extensions = [ "cue" "chd" "pbp" "m3u" "ccd" "iso" ];
      # nixpkgs' aarch64 build already has the ari64 dynarec (+ its compile
      # thread), the NEON GPU and -Ofast ("arm64v8 neon ari64 gpu=neon" in
      # the log); rebuilding it with -mcpu=cortex-a53 measured the same
      # (2.09 vs 2.10 ms/frame), so the cached build stays.
      core = lr.pcsx-rearmed;
      # crisp but even: nearest x3 for 320x240 (x1 for 640x480), the display
      # engine scales the rest (plain smooth looked a little soft)
      scaling = "sharp-bilinear";
      fastForward = "f2";
      # Option names and values as this pcsx-rearmed (r26) declares them:
      # GPU thread is enabled/disabled/auto (no sync/async here), the SPU
      # IRQ option is gone. Frameskip "auto" only skips drawing when
      # mortar-play's audio queue reports an underrun is likely (below a
      # quarter full), i.e. only in scenes that can't keep up.
      options = {
        pcsx_rearmed_drc = "enabled";
        pcsx_rearmed_drc_thread = "auto";
        pcsx_rearmed_gpu_thread_rendering = "enabled";
        pcsx_rearmed_spu_thread = "enabled";
        pcsx_rearmed_frameskip_type = "auto";
        pcsx_rearmed_neon_interlace_enable_v2 = "disabled";
        pcsx_rearmed_neon_enhancement_enable = "disabled";
        pcsx_rearmed_show_bios_bootlogo = "disabled";
        pcsx_rearmed_dithering = "enabled";
        pcsx_rearmed_memcard2 = "none";
      };
      bios = [
        {
          files = [ "scph5501.bin" "scph7001.bin" "scph1001.bin" "scph101.bin" "scph5500.bin" "scph5502.bin" "psxonpsp660.bin" "ps1_rom.bin" ];
          description = "PlayStation BIOS (the built-in HLE BIOS plays most games)";
        }
      ];
      order = 50;
    };
    arcade = {
      name = "Arcade";
      tag = "ARC";
      folders = [ "arcade" "fbneo" "neogeo" ];
      extensions = [ "zip" "7z" ];
      core = lr.fbneo;
      keepArchives = true;
      fastForward = "f2";
      options = {
        fbneo-frameskip-type = "disabled";
        fbneo-samplerate = "44100";
        fbneo-allow-depth-32 = "disabled";
        fbneo-hiscores = "enabled";
      };
      bios = [ { files = [ "neogeo.zip" ]; description = "Neo Geo BIOS set (only for Neo Geo games; next to the games or in the BIOS folder)"; } ];
      order = 60;
    };
    atari2600 = {
      name = "Atari 2600";
      tag = "2600";
      folders = [ "atari2600" "a26" ];
      extensions = [ "a26" "bin" ];
      core = lr.stella2014;
      order = 70;
    };
    lynx = {
      name = "Atari Lynx";
      tag = "LYNX";
      folders = [ "lynx" ];
      extensions = [ "lnx" "o" ];
      core = lr.handy;
      scaling = "integer";
      bios = [ { files = [ "lynxboot.img" ]; required = true; description = "Lynx boot ROM"; } ];
      order = 71;
    };
    ngp = {
      name = "Neo Geo Pocket";
      tag = "NGP";
      folders = [ "ngp" "ngpc" ];
      extensions = [ "ngp" "ngc" ];
      core = lr.beetle-ngp;
      scaling = "integer";
      order = 72;
    };
    wswan = {
      name = "WonderSwan";
      tag = "WS";
      folders = [ "wswan" "wsc" ];
      extensions = [ "ws" "wsc" ];
      core = lr.beetle-wswan;
      scaling = "integer";
      order = 73;
    };
    "2048" = {
      name = "2048";
      tag = "2048";
      folders = [ "2048" ];
      core = lr.twenty-fortyeight;
      noContent = true;
      order = 90;
    };
  };

  enabled = lib.filterAttrs (_: s: s.enable) cfg.systems;

  configJson = pkgs.writeText "mortar-play.json" (
    builtins.toJSON {
      romDirs = [ "${cfg.dataDir}/roms" ] ++ cfg.extraRomDirs;
      biosDir = "${cfg.dataDir}/bios";
      saveDir = "${cfg.dataDir}/saves";
      stateDir = "${cfg.dataDir}/states";
      dataDir = cfg.dataDir;
      inherit (cfg) audioLatencyMs autosaveSeconds autoResume theme hotkeys;
      systems = lib.mapAttrsToList (id: s: {
        inherit id;
        inherit (s) name tag folders extensions options scaling aspect controls fastForward bios keepArchives noContent order;
        core = coreFile s.core;
      }) enabled;
    }
  );

  # The config, after checking every core file exists (catches a core
  # whose library name doesn't follow <core>_libretro.so).
  configFile = pkgs.runCommand "mortar-play-config.json" { } ''
    ${lib.concatMapStrings (s: ''
      test -e ${coreFile s.core} || { echo "mortar-play: missing ${coreFile s.core}"; exit 1; }
    '') (lib.attrValues enabled)}
    cp ${configJson} $out
  '';

  launcher = pkgs.writeShellScriptBin "mortar-play-launch" ''
    # Logs: journalctl -t mortar-play
    export MORTAR_PLAY_CONFIG=/etc/mortar-play/config.json
    exec ${pkgs.systemd}/bin/systemd-cat -t mortar-play --level-prefix=false ${lib.getExe cfg.package} "$@"
  '';

  hasShell = options ? mortar && options.mortar ? shell;
  hasGreeter = options ? mortar && options.mortar ? greeter;
  hasGpu = options ? mortar && options.mortar ? gpu;
in
{
  options.mortar.play = {
    enable = mkEnableOption "mortar-play, emulators through libretro with a preconfigured minimal frontend" // { default = true; };

    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ../../pkgs/mortar-play { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../pkgs/mortar-play { }";
    };

    dataDir = mkOption {
      type = types.str;
      default = "/var/lib/mortar-play";
      description = "ROMs (roms/<folder>), BIOS (bios/), saves, states and recents.";
    };

    user = mkOption {
      type = types.str;
      default = config.mortar.user.name or "root";
      defaultText = lib.literalExpression "config.mortar.user.name";
      description = ''
        Owner of dataDir and its folders: the session user, who plays (and
        writes saves, states, recents). ROMs and BIOS copied in as root over
        SSH stay readable to it.
      '';
    };

    extraRomDirs = mkOption {
      type = types.listOf types.str;
      default = [ "~/Games" ];
      description = "More ROM roots, each with one folder per system.";
    };

    systems = mkOption {
      type = types.attrsOf systemType;
      default = { };
      example = lib.literalExpression ''
        {
          snes.core = pkgs.libretro.snes9x2010;
          gb.options.gambatte_gb_colorization = "disabled";
          lynx.enable = false;
        }
      '';
      description = "Systems, keyed by id. The curated set is the default; set attributes here to change one.";
    };

    hotkeys = {
      menu = mkOption {
        type = types.listOf types.str;
        default = [ "menu" "f1" "select+menu" ];
        description = ''
          Chords that open the in-game menu. Inside mortar-shell a lone MENU
          opens the shell's overlay (the game pauses meanwhile); hold SELECT
          and press MENU, or press F1, for the game's menu.
        '';
      };
      fastForward = mkOption { type = types.str; default = "r2"; description = "Hold to fast-forward (systems without an R2 of their own)."; };
      fastForwardSpeed = mkOption { type = types.ints.between 2 8; default = 3; };
    };

    audioLatencyMs = mkOption { type = types.ints.between 16 250; default = 64; description = "Target audio queue (dynamic rate control keeps it there)."; };
    autosaveSeconds = mkOption { type = types.ints.positive; default = 10; description = "How often changed save RAM is written."; };
    autoResume = mkOption { type = types.bool; default = true; description = "Save a state on quit and load it when the game starts again."; };
    theme = mkOption { type = types.enum [ "light" "dark" ]; default = "light"; };

    shellApp = mkOption { type = types.bool; default = true; description = "Add a \"Games\" entry to mortar-shell's launcher."; };
    greeterSession = mkOption { type = types.bool; default = true; description = "Add a \"Games\" greeter session (GPU path) when the greeter is enabled."; };

    unfreeCores = mkOption {
      type = types.listOf types.str;
      readOnly = true;
      default = lib.unique (map (s: lib.getName s.core) (lib.filter (s: !(s.core.meta.license.free or true)) (lib.attrValues enabled)));
      description = "Package names of enabled cores with unfree licences, for nixpkgs.config.allowUnfreePredicate.";
    };

    configFile = mkOption { type = types.path; readOnly = true; default = configFile; };

    check = mkOption {
      type = types.package;
      readOnly = true;
      description = ''
        Build to load every enabled core and validate its option presets
        against what the core declares (`mortar-play check`); the report is
        in $out. Fails on an unknown option or value.
      '';
      default = pkgs.runCommand "mortar-play-check" { nativeBuildInputs = [ cfg.package ]; } ''
        mortar-play check --config ${configFile} | tee $out
      '';
    };
  };

  config = lib.mkMerge [
    { mortar.unfree = [ "libretro-snes9x2005-plus" "libretro-genesis-plus-gx" "libretro-fbneo" ]; }
    { mortar.play.systems = lib.mapAttrs (_: s: lib.mapAttrs (_: lib.mkDefault) s) curated; }
    (lib.mkIf cfg.enable (
      lib.mkMerge [
        {
          environment.etc."mortar-play/config.json".source = configFile;
          environment.systemPackages = [ cfg.package launcher ];

          systemd.tmpfiles.rules =
            map (d: "d ${cfg.dataDir}/${d} 0755 ${owner} - -") [ "" "roms" "bios" "saves" "states" ]
            ++ map (f: "d ${cfg.dataDir}/roms/${f} 0755 ${owner} - -") (lib.unique (lib.concatMap (s: lib.take 1 s.folders) (lib.attrValues (lib.filterAttrs (_: s: !s.noContent) enabled))));
        }
        (lib.optionalAttrs hasShell (
          lib.mkIf cfg.shellApp {
            mortar.shell.apps.play = {
              name = "Games";
              description = "Emulators · libretro";
              icon = "pad";
              order = 8;
              category = "games";
              gamepad = "forward";
              command = [ "${launcher}/bin/mortar-play-launch" ];
            };
            # every game its own app: MENU > Games lists the library (read
            # with `list --json` off the shell's event loop), A starts
            # `run SYSTEM ROM`; the play entry above is "All systems"
            mortar.shell.games = {
              list = [ (lib.getExe cfg.package) "--config" "/etc/mortar-play/config.json" "list" "--json" ];
              run = [ "${launcher}/bin/mortar-play-launch" "run" ];
              library = "play";
            };
          }
        ))
        (lib.optionalAttrs hasGreeter (
          lib.mkIf (cfg.greeterSession && config.mortar.greeter.enable) {
            mortar.greeter.sessions.play = {
              name = "Games";
              description = "Emulators · libretro";
              icon = "pad";
              order = 15;
              command =
                if hasGpu then
                  "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${launcher}/bin/mortar-play-launch"
                else
                  "exec ${launcher}/bin/mortar-play-launch";
            };
          }
        ))
      ]
    ))
  ];
}
