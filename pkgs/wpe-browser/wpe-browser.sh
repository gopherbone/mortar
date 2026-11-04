#!@bash@
# Start WPE Browser on Mortar. Upstream's launch.sh, without its /mnt paths and its
# firmware, GStreamer-bundle and Cedar parts. Settings in ~/.config/wpe-browser;
# history, favorites and the compiled ad-block rules in ~/.local/share/wpe-browser;
# cookies and site data where WebKit keeps them under ~/.local/share; caches in ~/.cache.
set -u
export PATH=@coreutils@/bin:@gnugrep@/bin:@gawk@/bin

share=@out@/share/wpe-browser
config=${XDG_CONFIG_HOME:-$HOME/.config}/wpe-browser
cache=${XDG_CACHE_HOME:-$HOME/.cache}/wpe-browser
settings=$config/settings.conf
mkdir -p "$config" "$cache"

# Only ever adds: new settings with their defaults, never changing a value.
if [ ! -f "$settings" ]; then
    cp "$share/settings.default.conf" "$settings"
    chmod u+w "$settings"
else
    have=" $(grep -o '^[A-Z_]*=' "$settings" | tr -d '=' | tr '\n' ' ') "
    new=$(awk -v have="$have" '
        /^#/ || /^$/ { block = block $0 "\n"; next }
        /^[A-Z_]+=/ { key = $0; sub(/=.*/, "", key)
                      if (index(have, " " key " ") == 0) printf "%s%s\n", block, $0
                      block = ""; next }
    ' "$share/settings.default.conf")
    if [ -n "$new" ]; then
        [ -n "$(tail -c 1 "$settings")" ] && echo >> "$settings"
        printf '%s\n' "$new" >> "$settings"
    fi
fi
export WPE_TSP_SETTINGS_FILE=$settings

keys="HOME_URL SEARCH_URL SCALE PAGE_MEMORY_LIMIT_MB POINTER_HIDE_SECONDS HISTORY_SIZE DOWNLOAD_DIR
      USER_AGENT AD_BLOCK AD_BLOCK_EXCEPTIONS WEBGL START_PAGE KEYBOARD_LAYOUTS"
for key in $keys; do
    eval "caller_$key=\${WPE_TSP_$key-}"
done
load_settings() {
    for key in $keys; do
        unset "$key"
    done
    [ -f "$settings" ] && . "$settings"
    for key in $keys; do
        eval "value=\${$key-} caller=\${caller_$key}"
        if [ -n "$caller" ]; then
            export "WPE_TSP_$key=$caller"
        elif [ -n "$value" ]; then
            export "WPE_TSP_$key=$value"
        else
            unset "WPE_TSP_$key"
        fi
    done
}
load_settings

export WPE_TSP_DEVICE=${WPE_TSP_DEVICE:-brick}
export FONTCONFIG_FILE=$share/fonts.conf
export GIO_MODULE_DIR=@glibNetworking@/lib/gio/modules
export XKB_CONFIG_ROOT=@xkeyboardConfig@/share/X11/xkb
export XDG_DATA_DIRS=$share/xdg:${XDG_DATA_DIRS:-}

# <video>/<audio>: only the plugins in the package, in software. The registry is a cache.
# AV1 is hidden so sites pick H.264 or VP9: it is too slow in software on four A53s.
export GST_PLUGIN_SYSTEM_PATH_1_0=@gstPlugins@/lib/gstreamer-1.0
export GST_PLUGIN_PATH_1_0=
export GST_REGISTRY_1_0=$cache/gstreamer-registry.bin
export GST_REGISTRY_FORK=no
export GST_PLUGIN_FEATURE_RANK=avdec_av1:NONE${GST_PLUGIN_FEATURE_RANK:+,$GST_PLUGIN_FEATURE_RANK}

# Tuning from upstream, measured there on the same GPU: no MSAA, TextureMapper for layers.
export WEBKIT_SKIA_MSAA_SAMPLE_COUNT=${WEBKIT_SKIA_MSAA_SAMPLE_COUNT:-0}
export WEBKIT_USE_SKIA_FOR_COMPOSITION=${WEBKIT_USE_SKIA_FOR_COMPOSITION:-0}
# One web process: each costs 100-200 MB.
export WEBKIT_DISABLE_PSON=1
export WEBKIT_DISABLE_WEB_PROCESS_CACHE=1
export MALLOC_ARENA_MAX=${MALLOC_ARENA_MAX:-2}

# The GPU, when mortar-sdl-run gave us mortar-wlegl. WebKit's processes use the stock EGL
# directly (surfaceless), the same copy that mortar-wlegl wraps, which it names itself.
# Anything else (mortar-sdl-run's software fallback, a test VM): Mesa, with copied frames.
case ${SDL_EGL_LIBRARY:-} in
*mortar-wlegl*)
    pvr=$(grep -a -o -m 1 '/nix/store/[a-z0-9]*-pvr-ddk-userspace-[^/:]*/lib' "$SDL_EGL_LIBRARY" | head -n 1)
    if [ -n "$pvr" ]; then
        export LD_LIBRARY_PATH=${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}$pvr
        # Frames without a CPU copy: WebKit renders into KMS dumb buffers, as mortar-wlegl
        # does. WPE_TSP_ZERO_COPY=0 copies them instead (diagnostics).
        if [ "${WPE_TSP_ZERO_COPY:-1}" = 1 ]; then
            for card in /sys/class/drm/card[0-9]; do
                case $(readlink "$card/device/driver" 2>/dev/null) in
                */sun4i-drm) export WEBKIT_DMABUF_DUMB=/dev/dri/${card##*/} ;;
                esac
            done
        fi
    else
        echo "wpe-browser: no stock EGL named in $SDL_EGL_LIBRARY" >&2
    fi
    ;;
esac

# 75: the Settings page restarts the browser for settings read at start, back on its page.
restart_uri=${XDG_RUNTIME_DIR:-/tmp}/wpe-tsp-restart-uri
while :; do
    @out@/bin/wpe-tsp "$@"
    status=$?
    [ "$status" = 75 ] || exit "$status"
    uri=$(cat "$restart_uri" 2>/dev/null)
    rm -f "$restart_uri"
    if [ -n "$uri" ]; then set -- "$uri"; else set --; fi
    load_settings
done
