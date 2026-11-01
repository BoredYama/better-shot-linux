#!/usr/bin/env bash
# Checks the BetterShot Helper GNOME Shell extension against a real, headless
# GNOME Shell with a private session bus, a private system bus (the shell's
# login manager needs one), and the extension enabled from this checkout. The
# first run uses one virtual 1280x800 monitor; a second run adds a 1024x768
# monitor to check Capture Screen's screen picker. It drives every helper method with gdbus, confirms that
# an unlisted program is refused, opens a real BetterShot window to list and
# capture, and runs the Rust client's integration test, which injects
# pointer input through Mutter's RemoteDesktop interface to check the pointer
# trace and to drive real fullscreen, region, and window captures in the app.
# The container has no screenshot portal backend, so captures that arrive
# prove the helper served them.
#
# Needs gnome-shell (46 or later), gdbus, dbus-send, dbus-daemon,
# dbus-run-session, and python3 with Pillow. Run from linux/ after
# `cargo build` (or through `make check-gnome-helper`).
set -euo pipefail

UUID=bettershot-helper@bettershot.site
HERE="$(cd "$(dirname "$0")/.." && pwd)"

if [[ "${1:-}" != --inside ]]; then
  command -v gnome-shell >/dev/null || { echo "gnome-shell is not installed; skipping the helper check" >&2; exit 0; }
  [[ -x "$HERE/target/debug/bettershot" ]] || { echo "Build first: cargo build" >&2; exit 1; }
  # The Rust client's integration test, built so its path can be allowed.
  CLIENT_TEST="$(cd "$HERE" && cargo test -p bettershot-platform --test gnome_helper --no-run --message-format=json 2>/dev/null |
    python3 -c 'import json,sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "gnome_helper":
        print(m["executable"])' || true)"
  WORK="$(mktemp -d)"
  # Wayland and bus sockets need short paths; the runtime folder holds them.
  RUN_BASE="${XDG_RUNTIME_DIR:-/run}"
  [[ -w "$RUN_BASE" ]] || RUN_BASE=/tmp
  RUN="$(mktemp -d "$RUN_BASE/bsgs.XXXXXX")"
  chmod 700 "$RUN"
  trap 'rm -rf "$WORK" "$RUN"' EXIT
  mkdir -p "$WORK/home/.local/share/gnome-shell/extensions" "$WORK/home/.config/glib-2.0/settings"
  cp -r "$HERE/gnome-extension/$UUID" "$WORK/home/.local/share/gnome-shell/extensions/"
  cat > "$WORK/home/.config/glib-2.0/settings/keyfile" <<EOF
[org/gnome/shell]
enabled-extensions=['$UUID']
disable-user-extensions=false
EOF
  export WORK RUN CLIENT_TEST HERE
  # One monitor for the helper's API and the app's captures, then two for
  # Capture Screen's screen picker, each in a fresh shell.
  MODE=single dbus-run-session -- bash "$0" --inside
  MODE=screens dbus-run-session -- bash "$0" --inside
  echo "All helper checks passed."
  exit 0
fi

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "ok - $*"; }

export HOME="$WORK/home" XDG_CONFIG_HOME="$WORK/home/.config" XDG_DATA_HOME="$WORK/home/.local/share"
export XDG_RUNTIME_DIR="$RUN" GSETTINGS_BACKEND=keyfile
SYSTEM_BUS="bettershot-gs-$$"
cat > "$WORK/system-bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>system</type>
  <listen>unix:abstract=$SYSTEM_BUS</listen>
  <auth>EXTERNAL</auth>
  <policy context="default"><allow user="*"/><allow own="*"/><allow send_type="method_call"/><allow send_type="signal"/>
    <allow send_type="method_return"/><allow send_type="error"/><allow receive_type="method_call"/>
    <allow receive_type="method_return"/><allow receive_type="error"/><allow receive_type="signal"/></policy>
</busconfig>
EOF
dbus-daemon --config-file="$WORK/system-bus.conf" --fork --print-pid > "$WORK/system-bus.pid"
export DBUS_SYSTEM_BUS_ADDRESS="unix:abstract=$SYSTEM_BUS"

# gdbus and the client test reach the helper directly; the app is the
# development build under test.
ALLOWED="/usr/bin/gdbus:$HERE/target/debug/bettershot"
[[ -n "$CLIENT_TEST" ]] && ALLOWED="$ALLOWED:$CLIENT_TEST"
MONITORS=(--virtual-monitor 1280x800)
[[ "$MODE" == screens ]] && MONITORS+=(--virtual-monitor 1024x768)
BETTERSHOT_HELPER_ALLOWED_PATHS="$ALLOWED" gnome-shell --headless "${MONITORS[@]}" --wayland --no-x11 \
  > "$WORK/shell.log" 2>&1 &
SHELL_PID=$!
APP_PID=""
cleanup() {
  [[ -n "$APP_PID" ]] && kill "$APP_PID" 2>/dev/null
  kill "$SHELL_PID" 2>/dev/null; wait "$SHELL_PID" 2>/dev/null
  kill "$(cat "$WORK/system-bus.pid")" 2>/dev/null || true
}
trap cleanup EXIT

helper() { gdbus call --session --dest site.bettershot.Helper --object-path /site/bettershot/Helper --method "site.bettershot.Helper.$1" "${@:2}"; }
has_owner() { gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner "$1" 2>/dev/null | grep -q true; }
png_size() { python3 -c "from PIL import Image; import sys; print('%dx%d' % Image.open(sys.argv[1]).size)" "$1"; }

for _ in $(seq 1 60); do has_owner site.bettershot.Helper && break; sleep 0.5; done
has_owner site.bettershot.Helper || { grep -iE "error|bettershot" "$WORK/shell.log" | head -20 >&2; fail "the extension did not start"; }
[[ "$MODE" == single ]] && pass "the extension owns site.bettershot.Helper"

# The app's test profile lives in the runtime folder, because the helper
# only writes there; it saves captures automatically and captures a region
# as soon as the drag ends, so the client test can drive it with the pointer.
APP_ROOT="$RUN/app-$MODE"
mkdir -p "$APP_ROOT/config"
cat > "$APP_ROOT/config/settings.json" <<EOF
{"afterCapture.screenshot.save": true, "bs_captureRegionOnRelease": true, "bs_playSound": false,
 "bs_onboardingSeenVersion": 2, "bs_releaseNotesSeenVersion": "99.0.0"}
EOF
export WAYLAND_DISPLAY=wayland-0 GDK_BACKEND=wayland
[[ -n "$CLIENT_TEST" ]] || fail "the Rust client test (crates/bettershot-platform/tests/gnome_helper.rs) did not build"
client_test() {
  if ! RUST_BACKTRACE=0 BETTERSHOT_HELPER_TEST="$1" BETTERSHOT_HELPER_APP="$HERE/target/debug/bettershot" \
    BETTERSHOT_HELPER_APP_ROOT="$APP_ROOT" "$CLIENT_TEST" --nocapture --test-threads=1 > "$WORK/client-$1.log" 2>&1; then
    tail -40 "$WORK/client-$1.log" >&2
    echo "--- app log" >&2; tail -20 "$WORK/app.log" >&2
    echo "--- app files" >&2; find "$APP_ROOT" -type f -not -path "*/runtime/*" 2>/dev/null | head -20 >&2
    fail "the Rust client test ($1) failed"
  fi
}

if [[ "$MODE" == screens ]]; then
  BETTERSHOT_TESTING=1 BETTERSHOT_TEST_ROOT="$APP_ROOT" "$HERE/target/debug/bettershot" > "$WORK/app.log" 2>&1 &
  APP_PID=$!
  client_test screens
  pass "Capture Screen on two monitors captures the one picked"
  if grep -E "JS ERROR|bettershot-helper" "$WORK/shell.log" | grep -v "Gio.IOErrorEnum: Could not connect" | grep -q .; then
    grep -E "JS ERROR|bettershot-helper" "$WORK/shell.log" | head -10 >&2
    fail "GNOME Shell logged errors from the extension"
  fi
  exit 0
fi

INFO="$(helper GetInfo)"
[[ "$INFO" == *"'api': <uint32 1>"* ]] || fail "GetInfo: $INFO"
[[ "$INFO" == *"(0, 0, 1280, 800, 1.0)"* ]] || fail "GetInfo monitors: $INFO"
pass "GetInfo reports the API and the virtual monitor"

helper Screenshot false "$RUN/desktop.png" >/dev/null
[[ "$(png_size "$RUN/desktop.png")" == 1280x800 ]] || fail "the desktop screenshot is $(png_size "$RUN/desktop.png")"
pass "Screenshot writes the whole desktop"

# Only allowed programs: dbus-send is not one.
DENIED="$(dbus-send --session --print-reply --dest=site.bettershot.Helper /site/bettershot/Helper site.bettershot.Helper.GetInfo 2>&1 || true)"
[[ "$DENIED" == *"site.bettershot.Helper.Error.AccessDenied"* ]] || fail "an unlisted program was not refused: $DENIED"
pass "an unlisted program is refused"

OUTSIDE="$(helper Screenshot false "$WORK/outside.png" 2>&1 || true)"
[[ "$OUTSIDE" == *"site.bettershot.Helper.Error.Failed"* && ! -e "$WORK/outside.png" ]] || fail "a path outside the runtime folder was accepted: $OUTSIDE"
pass "files outside the runtime folder are refused"

# A real window for the client test to list and capture: BetterShot's own
# Settings window. The client test closes the Activities overview a fresh
# shell opens, then checks ListWindows, ScreenshotWindow (with and without
# the shadow), and a pointer trace with motion and a click it injects through
# Mutter's RemoteDesktop interface, which lives only as long as one
# connection, so gdbus calls cannot drive it.
BETTERSHOT_TESTING=1 BETTERSHOT_TEST_ROOT="$APP_ROOT" "$HERE/target/debug/bettershot" > "$WORK/app.log" 2>&1 &
APP_PID=$!
client_test 1
pass "the Rust client lists and captures windows and traces the pointer"
pass "BetterShot captures the screen, a dragged region, and a picked window through the helper"

if grep -E "JS ERROR|bettershot-helper" "$WORK/shell.log" | grep -v "Gio.IOErrorEnum: Could not connect" | grep -q .; then
  grep -E "JS ERROR|bettershot-helper" "$WORK/shell.log" | head -10 >&2
  fail "GNOME Shell logged errors from the extension"
fi
pass "GNOME Shell logged no extension errors"
