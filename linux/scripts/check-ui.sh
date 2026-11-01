#!/usr/bin/env bash
# Drives the real app under Xvfb with a fixture "screen" instead of the portal,
# and checks the files each flow writes. Screenshots of every step are written
# to target/ui-snapshots/ for review in light and dark appearances.
#
# Needs: Xvfb, Openbox, xdotool, xclip, ImageMagick (import), dbus-run-session,
# python3 + Pillow, Tesseract (tesseract-ocr) for the text scenarios, and
# gst-discoverer-1.0 (gstreamer1.0-plugins-base-apps) for the recording scenario. Openbox provides real window-manager focus and stacking.
# Run from linux/ after `cargo build`: bash scripts/check-ui.sh
# (BETTERSHOT_UI_ONLY=text,color runs only the named scenarios.)
#
# What this cannot check: the real portals, Wayland compositors, global
# shortcuts, the tray (no StatusNotifierWatcher here), or window placement.
set -euo pipefail

cd "$(dirname "$0")/.."
BIN="$PWD/target/debug/bettershot"
OUT="$PWD/target/ui-snapshots"
export DISPLAY=":${BETTERSHOT_XVFB_DISPLAY:-97}" GDK_BACKEND=x11 NO_AT_BRIDGE=1 GTK_A11Y=none
# Toolbar positions in the 1180-wide editor window.
SMART_X=588
# The card's bottom-right action, the editor header's Share button, and the
# Settings Sharing tab, in window coordinates.
CARD_SHARE_X=309
CARD_SHARE_Y=209
EDITOR_SHARE_X=939
SETTINGS_SHARING_X=554
# The gallery's Cloud Shares > All Media row and the first card's actions button.
GALLERY_CLOUD_X=80
GALLERY_CLOUD_Y=262
GALLERY_LOCAL_Y=110
GALLERY_MENU_X=462
GALLERY_MENU_Y=88
GALLERY_MENU2_X=723
GALLERY_DELETE_Y=312
GALLERY_CONFIRM_X=599
# Settings tabs sit along the bottom of the 720x640 window: General 68,
# Capture 166, Recording 263, Capture Deck 360, Shortcuts 457, Sharing 554, About 651.
# Settings › About › What's New, in window coordinates.
ABOUT_WHATS_NEW_Y=400
GALLERY_CONFIRM_Y=395


fail() {
  echo "FAIL: $*" >&2
  [[ -f "${BETTERSHOT_TEST_ROOT:-}/app.log" ]] && grep -v "^ *[0-9]*: \|^ *at " "$BETTERSHOT_TEST_ROOT/app.log" | tail -20 >&2
  exit 1
}
shot() { import -window root "$OUT/$1.png"; }
saved_files() { find "$BETTERSHOT_TEST_ROOT/Pictures/Screenshots" -type f 2>/dev/null | sort; }
size_of() { python3 -c "from PIL import Image; import sys; print('%dx%d' % Image.open(sys.argv[1]).size)" "$1"; }
click_at() { xdotool mousemove "$1" "$2"; sleep 0.4; xdotool mousedown 1; sleep 0.15; xdotool mouseup 1; sleep 0.8; }
settings_tab() {
  # Tabs sit along the bottom of the 720x640 Settings window.
  eval "$(xdotool getwindowgeometry --shell "$(xdotool search --onlyvisible --name 'BetterShot Settings')")"
  click_at $((X + $1)) $((Y + 615))
}
drag_to() { xdotool mousemove "$1" "$2"; sleep 0.3; xdotool mousedown 1; sleep 0.1; xdotool mousemove $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 )); sleep 0.1; xdotool mousemove "$3" "$4"; sleep 0.2; xdotool mouseup 1; sleep 0.4; }
editor_window() { xdotool search --onlyvisible --name 'BetterShot$' | head -1; }
shape_count() { python3 -c "import json,sys; print(len(json.load(open(sys.argv[1]))['shapes']))" "$1"; }
# xdotool's --name can't match titles with characters outside Latin-1 (like
# ’ or —) here, so compare the names in bash.
window_titled() {
  local id
  for id in $(xdotool search --onlyvisible --name "." 2>/dev/null); do
    [[ "$(xdotool getwindowname "$id" 2>/dev/null)" == $1 ]] && { echo "$id"; return 0; }
  done
  return 0
}
clipboard_text() { xclip -o -selection clipboard 2>/dev/null || true; }
select_area() { xdotool mousemove 300 200 mousedown 1 mousemove 450 300 mousemove 620 420 mouseup 1; sleep 0.7; }

scenario() {
  local name="$1" appearance="$2" settings="$3"
  # BETTERSHOT_UI_ONLY=text,color runs only those scenarios.
  if [[ -n "${BETTERSHOT_UI_ONLY:-}" && ",$BETTERSHOT_UI_ONLY," != *",$name,"* ]]; then return 0; fi
  export BETTERSHOT_TEST_ROOT="$WORK/$name-$appearance"
  mkdir -p "$BETTERSHOT_TEST_ROOT/config"
  if [[ -n "$settings" ]]; then
    printf '%s' "$settings" > "$BETTERSHOT_TEST_ROOT/config/settings.json"
    # Release notes stay closed unless the scenario is about them.
    if [[ "$name" != whatsnew ]]; then
      python3 - "$BETTERSHOT_TEST_ROOT/config/settings.json" <<'PY'
import json, sys
settings = json.load(open(sys.argv[1]))
settings.setdefault("bs_releaseNotesSeenVersion", "99.0.0")
json.dump(settings, open(sys.argv[1], "w"))
PY
    fi
  fi
  export SCENARIO="$name" APPEARANCE="$appearance"
  dbus-run-session -- bash "$0" --inside 2>"$WORK/$name-$appearance.log" || { cat "$WORK/$name-$appearance.log" >&2; fail "$name"; }
  echo "PASS $name ($appearance)"
}

inside() {
  openbox >/dev/null 2>&1 &
  sleep 0.5
  [[ "$SCENARIO" != scroll ]] || export BETTERSHOT_FAKE_SCREEN="$SCROLL_FRAMES"
  # A new install has no appearance setting; follow a dark "system" instead.
  [[ "$APPEARANCE" != dark ]] || export ADW_DEBUG_COLOR_SCHEME=prefer-dark
  if [[ "$SCENARIO" == share ]]; then
    # Fixture keys for the fake bucket; tests never open the real keyring.
    printf '{"accessKeyID": "AKIDFIXTURE", "secretAccessKey": "fixture-secret"}' > "$BETTERSHOT_TEST_ROOT/config/test-r2-keys.json"
    export FAKE_R2="$BETTERSHOT_TEST_ROOT/fake-r2"
    pkill -f "^python3 .*fake-r2[.]py" || true
    python3 "$FAKE_R2_SCRIPT" "$FAKE_R2" 8765 >"$BETTERSHOT_TEST_ROOT/fake-r2.log" 2>&1 &
    export BETTERSHOT_R2_ENDPOINT="http://127.0.0.1:8765"
    sleep 0.5
  fi
  "$BIN" >"$BETTERSHOT_TEST_ROOT/app.log" 2>&1 &
  sleep 2.5
  shot "$SCENARIO-$APPEARANCE-1-settings"
  case "$SCENARIO" in
    autosave)
      "$BIN" capture region; sleep 1.5
      shot "$SCENARIO-$APPEARANCE-2-selector"
      select_area; shot "$SCENARIO-$APPEARANCE-3-selected"
      xdotool key Return; sleep 1.5
      shot "$SCENARIO-$APPEARANCE-4-deck"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "automatic saving wrote $(saved_files | wc -l) files"
      [[ $(size_of "$(saved_files)") == 320x220 ]] || fail "saved size $(size_of "$(saved_files)")"
      find "$BETTERSHOT_TEST_ROOT/runtime/Clipboard" -name '*.png' | grep -q . || fail "nothing was copied"

      "$BIN" capture previous-region; sleep 1.5
      [[ $(saved_files | wc -l) -eq 2 ]] || fail "previous region did not save"
      [[ $(size_of "$(saved_files | tail -1)") == 320x220 ]] || fail "previous region size differs"
      ;;
    manual)
      settings_tab 166; shot "$SCENARIO-$APPEARANCE-1b-settings-capture"
      settings_tab 263; shot "$SCENARIO-$APPEARANCE-1c-settings-recording"
      settings_tab 360; shot "$SCENARIO-$APPEARANCE-1d-settings-deck"
      settings_tab 457; shot "$SCENARIO-$APPEARANCE-1e-settings-shortcuts"
      settings_tab 68
      "$BIN" capture region; sleep 1.5
      select_area; xdotool key Escape; sleep 1
      [[ -z $(find "$BETTERSHOT_TEST_ROOT/data" -name '*.png' 2>/dev/null) ]] || fail "a cancelled selection left files"
      "$BIN" capture region; sleep 1.5
      select_area; xdotool key Return; sleep 1.5
      shot "$SCENARIO-$APPEARANCE-2-deck-unsaved"
      [[ $(saved_files | wc -l) -eq 0 ]] || fail "a capture reached the save folder without Save"
      # The Save button sits at the card's center right, with Settings still open.
      eval "$(xdotool getwindowgeometry --shell "$(xdotool search --onlyvisible --name '^Captures$')")"
      xdotool mousemove $((X + 309)) $((Y + 120)); sleep 0.5; xdotool mousedown 1; sleep 0.15; xdotool mouseup 1; sleep 1
      shot "$SCENARIO-$APPEARANCE-3-deck-saved"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "Save did not write the capture"
      ;;
    editor)
      "$BIN" edit "$BETTERSHOT_FAKE_SCREEN"; sleep 2.5
      eval "$(xdotool getwindowgeometry --shell "$(editor_window)")"
      local ex=$X ey=$Y
      xdotool mousemove $((ex + 700)) $((ey + 650)); sleep 0.3
      xdotool key r; sleep 0.3; drag_to $((ex + 380)) $((ey + 260)) $((ex + 600)) $((ey + 360))
      xdotool key a; sleep 0.3; drag_to $((ex + 860)) $((ey + 250)) $((ex + 700)) $((ey + 320))
      xdotool key 1; sleep 0.3; drag_to $((ex + 980)) $((ey + 430)) $((ex + 980)) $((ey + 430))
      xdotool key t; sleep 0.3; drag_to $((ex + 400)) $((ey + 520)) $((ex + 400)) $((ey + 520))
      xdotool type --delay 30 "Check this"; sleep 0.3; xdotool key Escape; sleep 0.3
      xdotool key Escape; sleep 0.2; xdotool key Escape; sleep 0.4
      shot "$SCENARIO-$APPEARANCE-2-annotated"
      # The Blush gradient tile, with no tool or selection showing style controls.
      xdotool mousemove $((ex + 99)) $((ey + 249)); sleep 0.3; xdotool mousedown 1; sleep 0.1; xdotool mouseup 1; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-3-background"
      xdotool key ctrl+s; sleep 3
      shot "$SCENARIO-$APPEARANCE-4-saved"
      local sidecar="$BETTERSHOT_TEST_ROOT/data/Library/screen.edited.png.bettershot"
      [[ -f "$sidecar" ]] || fail "Save did not write the sidecar"
      [[ $(shape_count "$sidecar") -eq 4 ]] || fail "the sidecar holds $(shape_count "$sidecar") shapes, not 4"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "Save did not write one export"
      local exported; exported="$(saved_files)"
      [[ $(size_of "$exported") != 1280x800 ]] || fail "the background did not change the export's size"

      # Rotate, then close: the unsaved-changes prompt appears; Cancel keeps the window.
      xdotool mousemove $((ex + 688)) $((ey + 72)); sleep 0.3; xdotool mousedown 1; sleep 0.1; xdotool mouseup 1; sleep 0.8
      # The header bar's close button, at the window's top right.
      xdotool mousemove $((ex + 1152)) $((ey + 27)); sleep 0.3; xdotool mousedown 1; sleep 0.1; xdotool mouseup 1; sleep 1
      shot "$SCENARIO-$APPEARANCE-5-close-prompt"
      python3 - "$OUT/$SCENARIO-$APPEARANCE-5-close-prompt.png" "$OUT/$SCENARIO-$APPEARANCE-4-saved.png" <<'PY' || fail "closing with unsaved changes showed no prompt"
import sys
from PIL import Image, ImageChops
# The prompt dims the window, so the frame differs broadly from the saved one.
a = Image.open(sys.argv[1]).convert("L").resize((160, 100))
b = Image.open(sys.argv[2]).convert("L").resize((160, 100))
diff = sum(1 for p in ImageChops.difference(a, b).getdata() if p > 12)
sys.exit(0 if diff > 4000 else 1)
PY
      xdotool key Escape; sleep 0.6
      [[ -n "$(editor_window)" ]] || fail "Cancel closed the editor"
      xdotool key ctrl+s; sleep 3
      local width height
      IFS=x read -r width height <<<"$(size_of "$exported")"
      (( height > width )) || fail "rotating did not update the saved export ($width x $height)"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "a second Save added a file instead of replacing the export"

      # Reopening loads the sidecar's shapes.
      xdotool mousemove $((ex + 1152)) $((ey + 27)); sleep 0.3; xdotool mousedown 1; sleep 0.1; xdotool mouseup 1; sleep 1
      [[ -z "$(editor_window)" ]] || fail "a saved editor did not close"
      "$BIN" edit "$exported"; sleep 2.5
      shot "$SCENARIO-$APPEARANCE-6-reopened"
      [[ -n "$(editor_window)" ]] || fail "the edited capture did not reopen"
      ;;
    crop)
      "$BIN" edit "$BETTERSHOT_FAKE_SCREEN"; sleep 2.5
      eval "$(xdotool getwindowgeometry --shell "$(editor_window)")"
      local cx=$X cy=$Y
      xdotool mousemove $((cx + 700)) $((cy + 650)); sleep 0.3
      xdotool key c; sleep 0.6
      # Drag the crop's top-left handle inward, from where the canvas shows the image's corner.
      local corner; corner=$(python3 - "$cx" "$cy" <<'PY'
import sys
cx, cy = int(sys.argv[1]), int(sys.argv[2])
# The canvas spans x 306...1176, y 95...796 of the window; a 1280x800 image fits at zoom min(1, ...).
w, h = 1176 - 306, 796 - 95
zoom = min((w - 64) / 1280, (h - 64) / 800, 1)
ox = 306 + (w - 1280 * zoom) / 2
oy = 95 + (h - 800 * zoom) / 2
print(int(cx + ox), int(cy + oy))
PY
)
      read -r hx hy <<<"$corner"
      drag_to "$hx" "$hy" $((hx + 120)) $((hy + 80))
      shot "$SCENARIO-$APPEARANCE-2-cropping"
      xdotool key Return; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-3-cropped"
      xdotool key ctrl+s; sleep 3
      local width height
      IFS=x read -r width height <<<"$(size_of "$(saved_files)")"
      (( width < 1280 && height < 800 && width > 900 )) || fail "the crop was not applied ($width x $height)"
      ;;
    text)
      # Capture Text copies the selection's text on release.
      "$BIN" capture text; sleep 1.5
      shot "$SCENARIO-$APPEARANCE-2-selector"
      drag_to 210 160 690 300; sleep 4
      local text; text="$(clipboard_text)"
      [[ "$text" == *"BetterShot capture notes"* ]] || fail "OCR copied \"$text\""
      [[ "$text" == *$'\n'* ]] || fail "OCR lost its line breaks"
      "$BIN" capture text-single-line; sleep 1.5
      drag_to 210 160 690 300; sleep 4
      text="$(clipboard_text)"
      [[ "$text" == *"notes Contact"* && "$text" != *$'\n'* ]] || fail "single-line OCR copied \"$text\""
      # An area without text must leave the clipboard alone.
      "$BIN" capture text; sleep 1.5
      drag_to 860 60 1200 180; sleep 4
      [[ "$(clipboard_text)" == "$text" ]] || fail "an empty OCR result changed the clipboard"
      ;;
    color)
      "$BIN" capture color; sleep 1.5
      xdotool mousemove 900 600; sleep 0.5; xdotool mousemove 905 602; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-2-loupe"
      xdotool mousedown 1; sleep 0.1; xdotool mouseup 1; sleep 1
      [[ "$(clipboard_text)" == "#F05A50" ]] || fail "the color picker copied \"$(clipboard_text)\""
      [[ -z $(xdotool search --onlyvisible --name '^Pick a color$' 2>/dev/null) ]] || fail "the picker stayed open"
      "$BIN" capture color; sleep 1.5
      xdotool key Escape; sleep 0.8
      [[ "$(clipboard_text)" == "#F05A50" ]] || fail "cancelling changed the clipboard"
      ;;
    redact)
      "$BIN" edit "$BETTERSHOT_FAKE_SCREEN"; sleep 2.5
      eval "$(xdotool getwindowgeometry --shell "$(editor_window)")"
      local rx=$X ry=$Y
      # Smart Redaction follows the twelve tool buttons.
      click_at $((rx + SMART_X)) $((ry + 72))
      shot "$SCENARIO-$APPEARANCE-2-popover"
      click_at $((rx + SMART_X - 40)) $((ry + 150)); sleep 5
      shot "$SCENARIO-$APPEARANCE-3-redacted"
      xdotool key Escape; sleep 0.3
      xdotool mousemove $((rx + 700)) $((ry + 650)); sleep 0.3
      xdotool key ctrl+s; sleep 3
      local sidecar="$BETTERSHOT_TEST_ROOT/data/Library/screen.edited.png.bettershot"
      [[ -f "$sidecar" ]] || fail "Save did not write the sidecar"
      python3 - "$sidecar" <<'PY' || fail "smart redaction did not cover the email address"
import json, sys
shapes = json.load(open(sys.argv[1]))["shapes"]
redactions = [s for s in shapes if "redaction" in s["kind"]]
# The email address sits at about x 333...550, y 262...285 in the 1280x800 fixture.
ok = any(315 < s["x"] < 340 and 245 < s["y"] < 270 and s["kind"]["redaction"]["_0"]["w"] > 200
         for s in redactions)
print(len(redactions), [(round(s["x"]), round(s["y"])) for s in redactions], file=sys.stderr)
sys.exit(0 if ok else 1)
PY
      ;;
    pin)
      "$BIN" capture region-pin; sleep 1.5
      select_area; xdotool key Return; sleep 1.5
      local pin; pin="$(xdotool search --onlyvisible --name '^Pinned Screenshot$' | head -1)"
      [[ -n "$pin" ]] || fail "Capture Region & Pin opened no pin"
      [[ -z $(xdotool search --onlyvisible --name '^Captures$' 2>/dev/null) ]] || fail "the card stayed after pinning"
      find "$BETTERSHOT_TEST_ROOT/data/Library" -name '*.png' | grep -q . || fail "the pinned capture is not in the library"
      [[ $(saved_files | wc -l) -eq 0 ]] || fail "pinning wrote to the save folder"
      eval "$(xdotool getwindowgeometry --shell "$pin")"
      [[ "${WIDTH}x$HEIGHT" == 320x220 ]] || fail "the pin opened at ${WIDTH}x$HEIGHT"
      local px=$X py=$Y
      xdotool mousemove $((px + 100)) $((py + 100)); sleep 0.6
      shot "$SCENARIO-$APPEARANCE-2-pin-hover"
      for _ in 1 2 3 4; do xdotool click 4; sleep 0.15; done; sleep 0.8
      eval "$(xdotool getwindowgeometry --shell "$pin")"
      [[ "${WIDTH}x$HEIGHT" == 384x264 ]] || fail "scrolling up scaled the pin to ${WIDTH}x$HEIGHT"
      xdotool mousemove $((px + 60)) $((py + 60)); sleep 0.3
      xdotool mousedown 3; sleep 0.1; xdotool mouseup 3; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-3-pin-menu"
      xdotool key Escape; sleep 0.4
      xdotool windowactivate --sync "$pin" 2>/dev/null || true; sleep 0.3
      xdotool key ctrl+w; sleep 0.8
      [[ -z $(xdotool search --onlyvisible --name '^Pinned Screenshot$' 2>/dev/null) ]] || fail "Ctrl+W did not close the pin"
      ;;
    scroll)
      # The fixture folder plays one frame per screenshot: the selector's, two
      # Add Frames scrolled 250 px each, then Done's final frame.
      "$BIN" capture scroll; sleep 1.5
      drag_to 320 90 880 700; sleep 0.3
      shot "$SCENARIO-$APPEARANCE-2-selected"
      xdotool key Return; sleep 1.5
      [[ -n $(xdotool search --onlyvisible --name '^Scrolling Capture$' 2>/dev/null) ]] || fail "no session window"
      shot "$SCENARIO-$APPEARANCE-3-session"
      xdotool key Return; sleep 2
      xdotool key Return; sleep 2
      shot "$SCENARIO-$APPEARANCE-4-two-frames"
      "$BIN" capture scroll; sleep 3
      [[ -z $(xdotool search --onlyvisible --name '^Scrolling Capture$' 2>/dev/null) ]] || fail "running the action again did not finish"
      shot "$SCENARIO-$APPEARANCE-5-deck"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "the scrolling capture was not saved"
      [[ $(size_of "$(saved_files)") == 560x1110 ]] || fail "the stitched size is $(size_of "$(saved_files)"), not 560x1110"
      python3 - "$(saved_files)" "$SCROLL_FRAMES-page.png" <<'PY' || fail "the stitched image does not match the page"
import sys
from PIL import Image
shot = Image.open(sys.argv[1]).convert("RGB")
page = Image.open(sys.argv[2]).convert("RGB")
# The area starts 20 px into the window and 10 px into its 60 px header, so
# the header fills rows 0...49 once, and below it the body is the page itself.
bad = sum(1 for y in range(50, shot.height, 7) for x in range(0, 540, 13)
          if shot.getpixel((x, y)) != page.getpixel((x + 20, y - 50)))
header = sum(1 for y in range(0, 50, 5) if shot.getpixel((100, y)) != (52, 101, 164))
print(f"{bad} body and {header} header mismatches", file=sys.stderr)
sys.exit(0 if bad == 0 and header == 0 else 1)
PY
      # Escape cancels a session without staging anything.
      "$BIN" capture scroll; sleep 1.5
      drag_to 320 90 880 700; xdotool key Return; sleep 1.5
      xdotool key Escape; sleep 1
      [[ -z $(xdotool search --onlyvisible --name '^Scrolling Capture$' 2>/dev/null) ]] || fail "Escape did not cancel"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "a cancelled session saved a capture"
      ;;
    share)
      # Share from a card: the fake bucket gets the objects, the clipboard the link.
      "$BIN" capture region; sleep 1.5
      select_area; xdotool key Return; sleep 1.5
      eval "$(xdotool getwindowgeometry --shell "$(xdotool search --onlyvisible --name '^Captures$')")"
      shot "$SCENARIO-$APPEARANCE-2-card"
      click_at $((X + CARD_SHARE_X)) $((Y + CARD_SHARE_Y)); sleep 3
      shot "$SCENARIO-$APPEARANCE-3-card-shared"
      local link; link="$(clipboard_text)"
      [[ "$link" == https://bettershot.site/s/* ]] || fail "sharing copied \"$link\""
      find "$FAKE_R2/shots/s" -name meta.json | grep -q . || fail "no manifest reached the bucket"
      grep -q "$link" "$BETTERSHOT_TEST_ROOT/data/Library/history.json" || fail "the link was not saved with the capture"
      [[ $(saved_files | wc -l) -eq 0 ]] || fail "sharing wrote to the save folder"
      # Share from the editor.
      "$BIN" edit "$BETTERSHOT_FAKE_SCREEN"; sleep 2.5
      eval "$(xdotool getwindowgeometry --shell "$(editor_window)")"
      click_at $((X + EDITOR_SHARE_X)) $((Y + 27)); sleep 4
      shot "$SCENARIO-$APPEARANCE-4-editor-shared"
      [[ "$(clipboard_text)" == https://bettershot.site/s/* && "$(clipboard_text)" != "$link" ]] || fail "editor sharing copied \"$(clipboard_text)\""
      [[ $(find "$FAKE_R2/shots/s" -name meta.json | wc -l) -eq 2 ]] || fail "the editor share did not upload"
      "$BIN" settings; sleep 1.5
      settings_tab "$SETTINGS_SHARING_X"; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-5-settings-sharing"
      xdotool key Escape; sleep 0.5
      # The Media Gallery lists both shared captures, locally and in the cloud.
      "$BIN" gallery; sleep 2.5
      local gallery; gallery="$(window_titled 'Media Gallery')"
      [[ -n "$gallery" ]] || fail "the Media Gallery did not open"
      xdotool windowactivate --sync "$gallery" 2>/dev/null || true; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-6-gallery-local"
      eval "$(xdotool getwindowgeometry --shell "$gallery")"
      local gx=$X gy=$Y
      click_at $((gx + GALLERY_CLOUD_X)) $((gy + GALLERY_CLOUD_Y))
      shot "$SCENARIO-$APPEARANCE-7-gallery-cloud"
      click_at $((gx + GALLERY_MENU_X)) $((gy + GALLERY_MENU_Y))
      shot "$SCENARIO-$APPEARANCE-8-gallery-menu"
      click_at $((gx + GALLERY_MENU_X)) $((gy + GALLERY_DELETE_Y))
      shot "$SCENARIO-$APPEARANCE-9-gallery-confirm"
      click_at $((gx + GALLERY_CONFIRM_X)) $((gy + GALLERY_CONFIRM_Y)); sleep 2
      shot "$SCENARIO-$APPEARANCE-10-gallery-deleted"
      [[ $(find "$FAKE_R2/shots/s" -name meta.json | wc -l) -eq 1 ]] || fail "Delete Cloud Share left the objects"
      [[ $(find "$FAKE_R2/shots/s" -type f | wc -l) -eq 3 ]] || fail "the other share lost objects"
      python3 - "$BETTERSHOT_TEST_ROOT/data/Library/history.json" <<'PY' || fail "the library still lists two share links"
import json, sys
records = json.load(open(sys.argv[1]))
print([(r.get("filename"), r.get("shareURL")) for r in records], file=sys.stderr)
sys.exit(0 if sum(1 for r in records if r.get("shareURL")) == 1 else 1)
PY
      # Move to Trash from On this Computer: files go to the (test) trash,
      # and the shared capture stays listed under Cloud Shares.
      click_at $((gx + GALLERY_CLOUD_X)) $((gy + GALLERY_LOCAL_Y))
      click_at $((gx + GALLERY_MENU2_X)) $((gy + GALLERY_MENU_Y))
      click_at $((gx + GALLERY_MENU2_X)) $((gy + GALLERY_DELETE_Y))
      shot "$SCENARIO-$APPEARANCE-11-trash-confirm"
      click_at $((gx + GALLERY_CONFIRM_X)) $((gy + GALLERY_CONFIRM_Y)); sleep 1
      shot "$SCENARIO-$APPEARANCE-12-trashed"
      [[ $(find "$BETTERSHOT_TEST_ROOT/data/Trash" -type f 2>/dev/null | wc -l) -ge 1 ]] || fail "Move to Trash moved nothing"
      python3 - "$BETTERSHOT_TEST_ROOT/data/Library/history.json" <<'PY' || fail "the shared capture left the library"
import json, sys
sys.exit(0 if any(r.get("shareURL") for r in json.load(open(sys.argv[1]))) else 1)
PY
      ;;
    onboarding)
      # A new install opens setup; Continue walks the four steps.
      local setup; setup="$(xdotool search --onlyvisible --name '^Welcome to BetterShot$' 2>/dev/null | head -1 || true)"
      [[ -n "$setup" ]] || fail "a new install did not open setup"
      xdotool windowactivate --sync "$setup" 2>/dev/null || true; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-2-welcome"
      xdotool key Return; sleep 0.6; shot "$SCENARIO-$APPEARANCE-3-permissions"
      xdotool key Return; sleep 0.6; shot "$SCENARIO-$APPEARANCE-4-saving"
      grep -q '"bs_onboardingStep": *2' "$BETTERSHOT_TEST_ROOT/config/settings.json" || fail "the step was not saved"
      xdotool key Return; sleep 0.6; shot "$SCENARIO-$APPEARANCE-5-practice"
      "$BIN" capture region; sleep 1.5
      select_area; xdotool key Return; sleep 2
      xdotool windowactivate --sync "$setup" 2>/dev/null || true; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-6-practice-done"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "a new install did not save automatically"
      xdotool key Return; sleep 0.8
      [[ -z $(xdotool search --onlyvisible --name '^Welcome to BetterShot$' 2>/dev/null) ]] || fail "Done did not close setup"
      python3 - "$BETTERSHOT_TEST_ROOT/config/settings.json" <<'PY' || fail "setup was not marked finished"
import json, sys
s = json.load(open(sys.argv[1]))
sys.exit(0 if s.get("bs_onboardingSeenVersion") == 2 and "bs_onboardingStep" not in s else 1)
PY
      ;;
    whatsnew)
      # No Linux release lists this version yet, so an upgrade shows nothing at
      # launch; Settings › About › What's New shows the Unreleased changes.
      [[ -z "$(window_titled '*New in BetterShot')" ]] || fail "release notes opened without a release entry"
      settings_tab 651; sleep 0.6
      shot "$SCENARIO-$APPEARANCE-2-about"
      eval "$(xdotool getwindowgeometry --shell "$(xdotool search --onlyvisible --name 'BetterShot Settings' | head -1)")"
      click_at $((X + 360)) $((Y + ABOUT_WHATS_NEW_Y)); sleep 1
      local notes; notes="$(window_titled '*New in BetterShot')"
      [[ -n "$notes" ]] || fail "What's New did not open from About"
      xdotool windowactivate --sync "$notes" 2>/dev/null || true; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-3-notes"
      xdotool key Escape; sleep 0.6
      [[ -z "$(window_titled '*New in BetterShot')" ]] || fail "Escape did not close What's New"
      ;;
    record)
      # The fixture image plays as the screen stream; the portal is not used.
      "$BIN" record; sleep 1.5
      [[ -n "$(window_titled 'Capture Bar')" ]] || fail "the capture bar did not open"
      shot "$SCENARIO-$APPEARANCE-2-bar"
      "$BIN" record area; sleep 1.5
      drag_to 700 120 1180 760
      shot "$SCENARIO-$APPEARANCE-3-area"
      xdotool key Return; sleep 1
      # Once the recording row shows, pointer travel and a click inside the
      # area land in input.json.
      for _ in $(seq 1 20); do [[ -n "$(window_titled 'Capture Bar')" ]] && break; sleep 0.25; done; sleep 1.5
      xdotool mousemove 800 300; sleep 0.3; xdotool mousemove 1000 500; sleep 0.3
      xdotool mousedown 1; sleep 0.15; xdotool mouseup 1; sleep 0.5
      [[ -n "$(window_titled 'Capture Bar')" ]] || fail "the recording bar is not showing"
      shot "$SCENARIO-$APPEARANCE-4-recording"
      "$BIN" record pause; sleep 1.5
      shot "$SCENARIO-$APPEARANCE-5-paused"
      "$BIN" record pause; sleep 1.5
      "$BIN" record stop; sleep 4
      shot "$SCENARIO-$APPEARANCE-6-card"
      [[ -z "$(window_titled 'Capture Bar')" ]] || fail "the bar stayed after Stop"
      [[ -n "$(window_titled 'Recordings')" ]] || fail "no recording card appeared"
      local package; package="$(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | head -1)"
      [[ -n "$package" && -f "$package/screen.mov" && -f "$package/capture.json" && -f "$package/poster.jpg" && -f "$package/input.json" ]] || fail "the package is incomplete: $(ls "$package" 2>/dev/null)"
      [[ $(saved_files | wc -l) -eq 1 && "$(saved_files)" == *.mp4 ]] || fail "automatic saving wrote $(saved_files | wc -l) files"
      python3 - "$package/capture.json" "$BETTERSHOT_TEST_ROOT/data/Library/history.json" "$package/input.json" <<'PY' || fail "the manifest or library entry is wrong"
import json, sys
m = json.load(open(sys.argv[1]))
print(m, file=sys.stderr)
# Under Xvfb the session is X11, so the cursor is hidden and sampled into input.json.
ok = (m["pixelWidth"], m["pixelHeight"]) == (480, 640) and 2.0 < m["duration"] < 9.0 and m["pointerSynthesized"] is True
records = json.load(open(sys.argv[2]))
ok = ok and any(r["kind"] == "recording" and r["pixelWidth"] == 480 for r in records)
pointer = json.load(open(sys.argv[3]))
print(len(pointer["travel"]), "travel samples,", [(e["phase"], round(e["x"], 3), round(e["y"], 3), round(e["time"], 2)) for e in pointer["presses"]], file=sys.stderr)
ok = ok and len(pointer["travel"]) >= 2 and all(0 <= s["x"] <= 1 and 0 <= s["y"] <= 1 for s in pointer["travel"])
ok = ok and [p["phase"] for p in pointer["presses"]] == ["down", "up"] and abs(pointer["presses"][0]["x"] - 300 / 480) < 0.02
sys.exit(0 if ok else 1)
PY
      local probe; probe="$(gst-discoverer-1.0 "$(saved_files)" 2>/dev/null || true)"
      [[ "$probe" == *"Width: 480"* && "$probe" == *"Height: 640"* && "$probe" == *"H.264 (High Profile)"* ]] || fail "the saved MP4 is not 480x640 H.264 High: $probe"
      # Discard asks first; Cancel keeps recording, Discard removes the package.
      "$BIN" record display; sleep 3
      "$BIN" record discard; sleep 1.2
      shot "$SCENARIO-$APPEARANCE-7-discard-confirm"
      xdotool key Escape; sleep 1
      [[ -n "$(window_titled 'Capture Bar')" ]] || fail "Cancel ended the recording"
      [[ $(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | wc -l) -eq 2 ]] || fail "the second recording has no package"
      "$BIN" record discard; sleep 1.2
      xdotool key shift+Tab; sleep 0.2; xdotool key Return; sleep 3
      [[ -z "$(window_titled 'Capture Bar')" ]] || fail "Discard did not finish"
      [[ $(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | wc -l) -eq 1 ]] || fail "Discard kept the package"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "a discarded recording was saved"
      # A package without a manifest is an interrupted recording: the next
      # launch recovers its playable footage into the library.
      "$BIN" quit; sleep 1
      local interrupted="$BETTERSHOT_TEST_ROOT/data/Recordings/BetterShot_interrupted_ABCDEF.bettershotrec"
      mkdir -p "$interrupted" && cp "$package/screen.mov" "$interrupted/screen.mov"
      "$BIN" >>"$BETTERSHOT_TEST_ROOT/app.log" 2>&1 &
      sleep 4
      [[ -f "$interrupted/capture.json" && -f "$interrupted/poster.jpg" ]] || fail "the interrupted recording was not recovered"
      [[ $(grep -o '"kind":"recording"' "$BETTERSHOT_TEST_ROOT/data/Library/history.json" | wc -l) -eq 2 ]] || fail "the recovered recording is not in the library"
      # The gallery shows both recordings; Open shows a card.
      "$BIN" gallery; sleep 2.5
      local gallery; gallery="$(window_titled 'Media Gallery')"
      [[ -n "$gallery" ]] || fail "the Media Gallery did not open"
      xdotool windowactivate --sync "$gallery" 2>/dev/null || true; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-8-gallery"
      ;;
    studio)
      # Record the fixture, open the package in the studio, cut it, frame it,
      # and save: the project, its flattened render, and the saved MP4 follow.
      "$BIN" record display; sleep 4
      "$BIN" record stop; sleep 4
      local package; package="$(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | head -1)"
      [[ -n "$package" ]] || fail "no recording package"
      "$BIN" edit "$package"; sleep 4
      local studio; studio="$(window_titled '*BetterShot Studio')"
      [[ -n "$studio" ]] || fail "the studio did not open"
      xdotool windowactivate --sync "$studio" 2>/dev/null || true; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-2-studio"
      eval "$(xdotool getwindowgeometry --shell "$studio")"
      local sx=$X sy=$Y sw=$WIDTH sh=$HEIGHT
      # Scissors, then a cut in the middle of the timeline.
      xdotool key s; sleep 0.3
      click_at $((sx + sw - 300)) $((sy + sh - 50))
      xdotool key s; sleep 0.3
      shot "$SCENARIO-$APPEARANCE-3-cut"
      # The Blush gradient tile.
      click_at $((sx + 95)) $((sy + 112))
      # Delete the selected (trailing) clip, then undo that.
      xdotool key Delete; sleep 0.5
      xdotool key ctrl+z; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-4-framed"
      xdotool key space; sleep 1.2; xdotool key space; sleep 0.5
      xdotool key ctrl+s; sleep 14
      shot "$SCENARIO-$APPEARANCE-5-saved"
      [[ -f "$package/edit.json" && -f "$package/render.json" ]] || fail "Save did not write the project"
      python3 - "$package/edit.json" <<'PY' || fail "the saved project lacks the cut or the gradient"
import json, sys
d = json.load(open(sys.argv[1]))
print(len(d["clips"]), list(d["style"]["background"].keys()), file=sys.stderr)
sys.exit(0 if len(d["clips"]) == 2 and "gradient" in d["style"]["background"] and d["formatVersion"] == 8 else 1)
PY
      local rendered; rendered="$(find "$package" -maxdepth 1 -name '*.mp4' | head -1)"
      [[ -n "$rendered" ]] || fail "Save did not render the deliverable into the package"
      [[ $(saved_files | wc -l) -eq 1 && "$(saved_files)" == *.mp4 ]] || fail "Save wrote $(saved_files | wc -l) files"
      local probe; probe="$(gst-discoverer-1.0 "$(saved_files)" 2>/dev/null || true)"
      [[ "$probe" == *"Width: 1280"* && "$probe" == *"Height: 800"* ]] || fail "the saved video is not 1280x800: $probe"
      # A second Save with the padding changed replaces the saved file.
      xdotool mousemove $((sx + 120)) $((sy + 349)); sleep 0.3; xdotool mousedown 1; sleep 0.1; xdotool mouseup 1; sleep 0.8
      xdotool key ctrl+s; sleep 14
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "a second Save added a file instead of replacing the export"
      # Closing with no unsaved edits closes at once.
      xdotool key ctrl+w; sleep 1
      [[ -z "$(window_titled '*BetterShot Studio')" ]] || fail "the studio did not close after saving"
      # Reopening loads the saved cut and background.
      "$BIN" edit "$package"; sleep 4
      studio="$(window_titled '*BetterShot Studio')"
      [[ -n "$studio" ]] || fail "the studio did not reopen"
      xdotool windowactivate --sync "$studio" 2>/dev/null || true; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-6-reopened"
      ;;
    cursor)
      # Record the fixture with pointer travel and a click (sampled from X11
      # with the cursor hidden), then restyle the cursor in the studio.
      "$BIN" record display; sleep 1
      for _ in $(seq 1 20); do [[ -n "$(window_titled 'Capture Bar')" ]] && break; sleep 0.25; done; sleep 1.5
      xdotool mousemove 300 300; sleep 0.3; xdotool mousemove 900 500; sleep 0.3
      xdotool mousedown 1; sleep 0.15; xdotool mouseup 1; sleep 0.5
      xdotool mousemove 400 600; sleep 0.6
      "$BIN" record stop; sleep 4
      local package; package="$(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | head -1)"
      [[ -n "$package" ]] || fail "no recording package"
      python3 - "$package/capture.json" "$package/input.json" <<'PY' || fail "the pointer was not sampled"
import json, sys
m = json.load(open(sys.argv[1])); p = json.load(open(sys.argv[2]))
print(len(p["travel"]), "travel samples", [(e["phase"], round(e["x"], 2), round(e["y"], 2)) for e in p["presses"]], file=sys.stderr)
ok = m["pointerSynthesized"] is True and len(p["travel"]) >= 3
ok = ok and [e["phase"] for e in p["presses"]] == ["down", "up"] and abs(p["presses"][0]["x"] - 900 / 1280) < 0.01
sys.exit(0 if ok else 1)
PY
      "$BIN" edit "$package"; sleep 4
      local studio; studio="$(window_titled '*BetterShot Studio')"
      [[ -n "$studio" ]] || fail "the studio did not open"
      xdotool windowactivate --sync "$studio" 2>/dev/null || true; sleep 0.5
      eval "$(xdotool getwindowgeometry --shell "$studio")"
      local sx=$X sy=$Y
      # Step to the click (about 1.2 s in): the pulse and the recorded cursor show.
      xdotool key --repeat 38 --delay 15 Right; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-2-recorded-cursor"
      # Style › Arrow through the dropdown (below the preset bar).
      click_at $((sx + 190)) $((sy + 642)); sleep 0.6
      shot "$SCENARIO-$APPEARANCE-3-styles"
      xdotool key Down; sleep 0.2; xdotool key Return; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-4-arrow"
      # Click effects off hides the pulse; undo brings it back.
      click_at $((sx + 220)) $((sy + 784)); sleep 0.5
      xdotool key ctrl+z; sleep 0.5
      xdotool key ctrl+s; sleep 14
      python3 - "$package/edit.json" <<'PY' || fail "the saved project lacks the cursor style"
import json, sys
d = json.load(open(sys.argv[1]))
print(d["style"].get("cursor"), d.get("showsClickEffects"), file=sys.stderr)
sys.exit(0 if d["style"]["cursor"]["appearance"] == "macOS" and d["showsClickEffects"] is True and d["formatVersion"] == 8 else 1)
PY
      [[ -n "$(find "$package" -maxdepth 1 -name '*.mp4' | head -1)" ]] || fail "Save did not render the deliverable"
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "Save wrote $(saved_files | wc -l) files"
      xdotool key ctrl+w; sleep 1
      [[ -z "$(window_titled '*BetterShot Studio')" ]] || fail "the studio did not close after saving"
      ;;
    handles)
      # A recording with a camera: drag the bubble, a mask's handle, and the
      # crop on the preview; Save keeps all three.
      "$BIN" record display; sleep 1
      for _ in $(seq 1 20); do [[ -n "$(window_titled 'Capture Bar')" ]] && break; sleep 0.25; done; sleep 2
      "$BIN" record stop; sleep 4
      local package; package="$(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | head -1)"
      [[ -n "$package" && -f "$package/camera.mov" ]] || fail "no recording package with a camera"
      "$BIN" edit "$package"; sleep 4
      local studio; studio="$(window_titled '*BetterShot Studio')"
      [[ -n "$studio" ]] || fail "the studio did not open"
      xdotool windowactivate --sync "$studio" 2>/dev/null || true; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-2-camera"
      # The canvas draws at (293, 63) scaled 0.711 with no background, so the
      # bubble at (0.85, 0.82) sits near (1067, 529); drag it to the lower left.
      drag_to 1067 529 500 500
      shot "$SCENARIO-$APPEARANCE-3-bubble-moved"
      # C adjusts the crop: drag its top-left handle inward and apply with Return.
      xdotool key c; sleep 0.6
      shot "$SCENARIO-$APPEARANCE-5-crop-editing"
      drag_to 295 65 393 163
      xdotool key Return; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-6-crop-applied"
      # B adds a blur mask; widen it by its bottom-right handle (the mask
      # sits in cropped source space, drawn through the crop).
      xdotool key b; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-7-mask"
      # The mask lives in source space, so the crop moves it: its default
      # rect now spans (516, 169)-(873, 409) on screen.
      drag_to 873 409 960 480
      shot "$SCENARIO-$APPEARANCE-8-mask-resized"
      xdotool key ctrl+s
      # The render has a camera, a blur, and a crop: wait for the saved file.
      for _ in $(seq 1 120); do [[ $(saved_files | wc -l) -ge 1 ]] && break; sleep 1; done; sleep 2
      python3 - "$package/edit.json" <<'PY' || fail "the saved project lacks the dragged bubble, mask, or crop"
import json, sys
d = json.load(open(sys.argv[1]))
style, masks, crop = d["style"], d.get("masks", []), d.get("crop")
print(style["cameraCenterX"], style["cameraCenterY"], masks and (masks[0]["width"], masks[0]["height"]), crop, file=sys.stderr)
ok = abs(style["cameraCenterX"] - 0.227) < 0.03 and abs(style["cameraCenterY"] - 0.768) < 0.03
ok = ok and len(masks) == 1 and 0.4 < masks[0]["width"] < 0.5 and 0.4 < masks[0]["height"] < 0.5
ok = ok and crop is not None and abs(crop[0] - 0.11) < 0.03 and abs(crop[1] - 0.176) < 0.03 and abs(crop[2] - 0.89) < 0.03
sys.exit(0 if ok else 1)
PY
      xdotool key ctrl+w; sleep 1
      [[ -z "$(window_titled '*BetterShot Studio')" ]] || fail "the studio did not close after saving"
      ;;
    reframe)
      # Export › Aspect ratio 9:16 renders a vertical video that follows the
      # pointer; Save writes the project's exportAspect and a 450×800 file.
      "$BIN" record display; sleep 1
      for _ in $(seq 1 20); do [[ -n "$(window_titled 'Capture Bar')" ]] && break; sleep 0.25; done; sleep 1.5
      xdotool mousemove 200 300; sleep 0.4; xdotool mousemove 1100 600; sleep 0.8
      "$BIN" record stop; sleep 4
      local package; package="$(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | head -1)"
      [[ -n "$package" ]] || fail "no recording package"
      "$BIN" edit "$package"; sleep 4
      local studio; studio="$(window_titled '*BetterShot Studio')"
      [[ -n "$studio" ]] || fail "the studio did not open"
      xdotool windowactivate --sync "$studio" 2>/dev/null || true; sleep 0.5
      xdotool mousemove 6 400 click --repeat 25 --delay 60 5; sleep 0.6
      shot "$SCENARIO-$APPEARANCE-2-export"
      # Aspect ratio › 9:16 (the fifth entry) through the dropdown.
      click_at 202 563; sleep 0.6
      xdotool key Down Down Down Down; sleep 0.3; xdotool key Return; sleep 1
      shot "$SCENARIO-$APPEARANCE-3-vertical"
      xdotool key ctrl+s
      for _ in $(seq 1 120); do [[ $(saved_files | wc -l) -ge 1 ]] && break; sleep 1; done; sleep 2
      python3 - "$package/edit.json" <<'PY' || fail "the saved project lacks the reframe"
import json, sys
d = json.load(open(sys.argv[1]))
print(d.get("exportAspect"), d.get("exportAspectMode"), file=sys.stderr)
sys.exit(0 if d.get("exportAspect") == "vertical9x16" and d.get("exportAspectMode") == "fill" else 1)
PY
      local probe; probe="$(gst-discoverer-1.0 "$(saved_files)" 2>/dev/null || true)"
      [[ "$probe" == *"Width: 450"* && "$probe" == *"Height: 800"* ]] || fail "the reframed video is not 450x800: $probe"
      xdotool key ctrl+w; sleep 1
      [[ -z "$(window_titled '*BetterShot Studio')" ]] || fail "the studio did not close after saving"
      ;;
    captions)
      # A chord pressed while recording becomes a keystroke caption; subtitle
      # cues written into the project (as macOS transcription would) draw as
      # the bar; a style preset saves the current look.
      "$BIN" record display; sleep 1
      for _ in $(seq 1 20); do [[ -n "$(window_titled 'Capture Bar')" ]] && break; sleep 0.25; done; sleep 1.5
      xdotool keydown ctrl+shift+s; sleep 0.3; xdotool keyup ctrl+shift+s; sleep 1.5
      "$BIN" record stop; sleep 4
      local package; package="$(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | head -1)"
      [[ -n "$package" ]] || fail "no recording package"
      python3 - "$package/input.json" "$package/edit.json" "$package/capture.json" <<'PY' || fail "the chord was not recorded"
import json, sys
p = json.load(open(sys.argv[1]))
print(json.load(open(sys.argv[3]))["duration"], p["keystrokes"], file=sys.stderr)
chords = [k for k in p["keystrokes"] if k["key"] == "S" and "Ctrl" in k["modifiers"] and "Shift" in k["modifiers"]]
if not chords:
    sys.exit(1)
# Subtitles as the macOS app's transcription writes them, timed to the chord.
t = chords[0]["time"]
doc = {"formatVersion": 8, "style": {"background": {"none": {}}, "padding": 0.08, "cornerRadius": 0.018, "shadow": 0.36,
       "cameraIsVisible": True, "cameraCenterX": 0.85, "cameraCenterY": 0.82, "cameraSize": 0.26, "cameraRoundness": 0.25},
       "zoomEnabled": True, "zoomCues": [], "trimStart": 0, "trimEnd": 99,
       "subtitleCues": [{"id": "0B5B8D2E-9D1B-4C4A-9F9B-0B0B0B0B0B01", "start": 0.0, "end": 99.0, "text": "hello from the subtitle bar"}],
       "subtitleWords": [{"text": "hello", "start": 0.0, "end": 0.4}, {"text": "from", "start": 0.5, "end": 0.8},
                         {"text": "the", "start": 0.9, "end": 1.1}, {"text": "subtitle", "start": 1.2, "end": 1.8}, {"text": "bar", "start": 1.9, "end": 2.4}],
       "subtitleWordHighlight": True}
json.dump(doc, open(sys.argv[2], "w"))
open(sys.argv[2] + ".frames", "w").write(str(int(round(t * 30)) + 6))
PY
      "$BIN" edit "$package"; sleep 4
      local studio; studio="$(window_titled '*BetterShot Studio')"
      [[ -n "$studio" ]] || fail "the studio did not open"
      xdotool windowactivate --sync "$studio" 2>/dev/null || true; sleep 0.5
      # Step onto the chord so its caption shows with the subtitle bar.
      xdotool key --repeat "$(cat "$package/edit.json.frames")" --delay 15 Right; sleep 0.8
      rm -f "$package/edit.json.frames"
      shot "$SCENARIO-$APPEARANCE-2-captions"
      # Save the look as a preset named "Plain" from the Background section.
      click_at 232 541; sleep 0.6
      xdotool type --delay 40 "Plain"; sleep 0.2; xdotool key Return; sleep 0.8
      shot "$SCENARIO-$APPEARANCE-3-preset"
      python3 - "$BETTERSHOT_TEST_ROOT/config/settings.json" <<'PY' || fail "the preset was not saved"
import json, sys
s = json.load(open(sys.argv[1]))
lib = s.get("recordingStudioStylePresetLibrary.v1")
print(lib and [p["name"] for p in lib["presets"]], file=sys.stderr)
sys.exit(0 if lib and [p["name"] for p in lib["presets"]] == ["Plain"] and lib.get("activePresetID") else 1)
PY
      xdotool key space; sleep 1.0; xdotool key space; sleep 0.5
      xdotool key ctrl+s
      for _ in $(seq 1 120); do [[ $(saved_files | wc -l) -ge 1 ]] && break; sleep 1; done; sleep 2
      python3 - "$package/edit.json" <<'PY' || fail "the saved project lost its captions"
import json, sys
d = json.load(open(sys.argv[1]))
print(d.get("showsKeystrokes"), d.get("keystrokePlacement"), d.get("showsSubtitles"), len(d.get("subtitleCues", [])), d.get("subtitleWordHighlight"), file=sys.stderr)
sys.exit(0 if d["showsKeystrokes"] is True and d["keystrokePlacement"] == "bottomCenter" and d["showsSubtitles"] is True and len(d["subtitleCues"]) == 1 and d["subtitleWordHighlight"] is True else 1)
PY
      xdotool key ctrl+w; sleep 1
      [[ -z "$(window_titled '*BetterShot Studio')" ]] || fail "the studio did not close after saving"
      ;;
    teleprompter)
      # The prompter shows the script while recording and scrolls a line at
      # a time; the bar's Script button opens the composer.
      "$BIN" record display; sleep 1
      for _ in $(seq 1 20); do [[ -n "$(window_titled 'Capture Bar')" ]] && break; sleep 0.25; done; sleep 1.5
      local prompter; prompter="$(window_titled 'Prompter')"
      [[ -n "$prompter" ]] || fail "the prompter did not appear while recording"
      shot "$SCENARIO-$APPEARANCE-2-prompter"
      eval "$(xdotool getwindowgeometry --shell "$prompter")"
      xdotool mousemove $((X + WIDTH / 2)) $((Y + HEIGHT / 2)); sleep 0.3; xdotool click 5; sleep 0.5
      shot "$SCENARIO-$APPEARANCE-3-scrolled"
      "$BIN" record stop; sleep 4
      [[ -z "$(window_titled 'Prompter')" ]] || fail "the prompter stayed after the recording"
      "$BIN" record; sleep 1.5
      local bar; bar="$(window_titled 'Capture Bar')"
      [[ -n "$bar" ]] || fail "the capture bar did not open"
      eval "$(xdotool getwindowgeometry --shell "$bar")"
      shot "$SCENARIO-$APPEARANCE-4-bar"
      click_at $((X + 800)) $((Y + 32))
      [[ -n "$(window_titled 'Teleprompter')" ]] || fail "the Script button did not open the composer"
      shot "$SCENARIO-$APPEARANCE-5-composer"
      xdotool key Escape; sleep 0.6
      [[ -z "$(window_titled 'Teleprompter')" ]] || fail "Escape did not close the composer"
      ;;
    shots3d)
      # D adds a three-second Glide shot at the playhead: the preview warps
      # the card in perspective and Save keeps the shot as macOS encodes it.
      "$BIN" record display; sleep 1
      for _ in $(seq 1 20); do [[ -n "$(window_titled 'Capture Bar')" ]] && break; sleep 0.25; done; sleep 2.5
      "$BIN" record stop; sleep 4
      local package; package="$(find "$BETTERSHOT_TEST_ROOT/data/Recordings" -maxdepth 1 -name '*.bettershotrec' | head -1)"
      [[ -n "$package" ]] || fail "no recording package"
      "$BIN" edit "$package"; sleep 4
      local studio; studio="$(window_titled '*BetterShot Studio')"
      [[ -n "$studio" ]] || fail "the studio did not open"
      xdotool windowactivate --sync "$studio" 2>/dev/null || true; sleep 0.5
      xdotool key d; sleep 1
      xdotool key --repeat 40 --delay 15 Right; sleep 2
      shot "$SCENARIO-$APPEARANCE-2-shot"
      xdotool key ctrl+s
      # The 3D pass runs on the CPU: a nine-second clip takes a minute or two here.
      for _ in $(seq 1 400); do [[ $(saved_files | wc -l) -ge 1 ]] && break; sleep 1; done; sleep 2
      python3 - "$package/edit.json" <<'PY' || fail "the saved project lacks the 3D shot"
import json, sys
d = json.load(open(sys.argv[1]))
shots = d.get("shots3D", [])
print(len(shots), shots and (shots[0]["start"], shots[0]["end"], shots[0]["startPose"]["camera"]["tiltX"], shots[0]["blur"]["mode"]), file=sys.stderr)
ok = len(shots) == 1 and shots[0]["start"] == 0 and abs(shots[0]["end"] - 3) < 1e-9
ok = ok and abs(shots[0]["startPose"]["camera"]["tiltX"] + 46.65) < 1e-9 and shots[0]["easing"] == "Linear" and shots[0]["blur"]["bokeh"] is True
sys.exit(0 if ok else 1)
PY
      [[ $(saved_files | wc -l) -eq 1 ]] || fail "Save wrote $(saved_files | wc -l) files"
      xdotool key ctrl+w; sleep 1
      [[ -z "$(window_titled '*BetterShot Studio')" ]] || fail "the studio did not close after saving"
      ;;
    hide)
      "$BIN" capture region; sleep 1.5
      select_area; xdotool key Return; sleep 1
      xdotool mousemove 1000 700; sleep 3.5
      shot "$SCENARIO-$APPEARANCE-2-after-hide"
      [[ -z $(xdotool search --onlyvisible --name '^Captures$' 2>/dev/null) ]] || fail "the card did not hide after 2 seconds"
      ;;
  esac
  "$BIN" quit || true
  sleep 0.5
  pkill -x openbox || true
  pkill -f "^python3 .*fake-r2[.]py" || true
}

if [[ "${1:-}" == "--inside" ]]; then
  inside
  exit 0
fi

WORK="$(mktemp -d)"
trap 'kill "$XVFB_PID" 2>/dev/null || true; pkill -f "^python3 .*fake-r2[.]py" || true; rm -rf "$WORK"' EXIT
rm -rf "$OUT" && mkdir -p "$OUT"
[[ -x "$BIN" ]] || { echo "Build first: cargo build" >&2; exit 1; }

Xvfb "$DISPLAY" -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB_PID=$!
sleep 1

python3 - "$WORK/screen.png" <<'PY'
import sys
from PIL import Image, ImageDraw
W, H = 1280, 800
img = Image.new("RGB", (W, H))
d = ImageDraw.Draw(img)
for y in range(H):
    d.line([(0, y), (W, y)], fill=(30 + y // 8, 60 + y // 10, 120 + y // 12))
d.rectangle([200, 150, 700, 450], fill=(245, 245, 240))
d.rectangle([800, 500, 1100, 700], fill=(240, 90, 80))
from PIL import ImageFont
font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 22)
d.text((240, 190), "BetterShot capture notes", font=font, fill=(29, 29, 31))
d.text((240, 260), "Contact jane@example.com", font=font, fill=(29, 29, 31))
img.save(sys.argv[1])
PY
# A fake R2 bucket: PUT stores under the folder, GET reads, DELETE removes.
export FAKE_R2_SCRIPT="$WORK/fake-r2.py"
cat > "$FAKE_R2_SCRIPT" <<'PY'
import http.server, os, sys
root = sys.argv[1]
class Bucket(http.server.BaseHTTPRequestHandler):
    def target(self):
        return os.path.join(root, self.path.lstrip("/").split("?")[0])
    def reply(self, code, body=b""):
        self.send_response(code)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def do_PUT(self):
        if not self.headers.get("Authorization", "").startswith("AWS4-HMAC-SHA256 Credential=AKIDFIXTURE/"):
            return self.reply(403, b"<Error><Code>AccessDenied</Code></Error>")
        data = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        os.makedirs(os.path.dirname(self.target()), exist_ok=True)
        open(self.target(), "wb").write(data)
        self.reply(200)
    def do_GET(self):
        path = self.target()
        if os.path.isfile(path):
            return self.reply(200, open(path, "rb").read())
        self.reply(404, b"<Error><Code>NoSuchKey</Code></Error>")
    def do_DELETE(self):
        if os.path.isfile(self.target()):
            os.remove(self.target())
        self.reply(204)
    def log_message(self, *args):
        pass
http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[2])), Bucket).serve_forever()
PY

# A window with a 60 px header over a long page, scrolled 0, 250, 500, 500 px.
export SCROLL_FRAMES="$WORK/scroll-frames"
mkdir -p "$SCROLL_FRAMES"
python3 - "$WORK/screen.png" "$SCROLL_FRAMES" <<'PY'
import random, sys
from PIL import Image, ImageDraw
random.seed(7)
page = Image.new("RGB", (600, 1400), (250, 250, 250))
d = ImageDraw.Draw(page)
y = 6
while y < 1380:
    x = 10
    while x < 560:
        w = random.randint(4, 30)
        shade = random.randint(0, 120)
        d.rectangle([x, y, min(x + w, 570), y + 9], fill=(shade, shade // 2, 60))
        x += w + random.randint(4, 10)
    y += random.randint(16, 26)
page.save(sys.argv[2] + "-page.png")
screen = Image.open(sys.argv[1])
for index, scroll in enumerate([0, 250, 500, 500]):
    frame = screen.copy()
    window = Image.new("RGB", (600, 640), (255, 255, 255))
    ImageDraw.Draw(window).rectangle([0, 0, 600, 59], fill=(52, 101, 164))
    window.paste(page.crop((0, scroll, 600, scroll + 580)), (0, 60))
    frame.paste(window, (300, 80))
    frame.save(f"{sys.argv[2]}/frame-{index}.png")
PY
export BETTERSHOT_TESTING=1 BETTERSHOT_FAKE_SCREEN="$WORK/screen.png"


for appearance in light dark; do
  # Any bs_ key marks an upgrade, which keeps automatic saving off unless set.
  scenario autosave "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"afterCapture.screenshot.save\": true}"
  scenario manual "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_playSound\": false, \"afterCapture.screenshot.save\": false, \"bs_onboardingSeenVersion\": 2}"
done
for appearance in light dark; do
  scenario editor "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2}"
done
scenario crop light '{"bs_onboardingSeenVersion": 2}'
scenario hide light '{"bs_overlayDismissDelay": 2, "bs_onboardingSeenVersion": 2}'
scenario text light '{"bs_onboardingSeenVersion": 2, "bs_playSound": false}'
for appearance in light dark; do
  # An empty settings file is a new install.
  scenario onboarding "$appearance" ""
  scenario whatsnew "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2}"
done
R2_SETTINGS='"bs_r2_accountID": "acct", "bs_r2_bucket": "shots", "bs_r2_publicBaseURL": "https://share.example.com", "bs_r2_enabled": true, "bs_r2_enabledMigrated": true'
for appearance in light dark; do
  scenario share "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2, \"afterCapture.screenshot.save\": false, $R2_SETTINGS}"
done
scenario pin light '{"bs_onboardingSeenVersion": 2, "afterCapture.screenshot.save": true}'
for appearance in light dark; do
  scenario scroll "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2, \"afterCapture.screenshot.save\": true}"
done
for appearance in light dark; do
  scenario color "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2}"
  scenario redact "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2}"
done
for appearance in light dark; do
  scenario studio "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2, \"bs_playSound\": false, \"afterCapture.recording.save\": false}"
done
for appearance in light dark; do
  scenario record "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2, \"bs_playSound\": false, \"afterCapture.recording.save\": true}"
done
scenario handles light '{"bs_onboardingSeenVersion": 2, "bs_playSound": false, "afterCapture.recording.save": false, "recordingCameraDeviceID": "cam"}'
scenario reframe light '{"bs_onboardingSeenVersion": 2, "bs_playSound": false, "afterCapture.recording.save": false}'
# System audio (a test source) gives the studio a soundtrack to play and a Transcribe control.
scenario captions light '{"bs_onboardingSeenVersion": 2, "bs_playSound": false, "afterCapture.recording.save": false, "recordingSystemAudio": true}'
scenario teleprompter light '{"bs_onboardingSeenVersion": 2, "bs_playSound": false, "afterCapture.recording.save": false, "recordingTeleprompterEnabled": true, "recordingTeleprompterLineCount": 2, "recordingTeleprompterScript": "Welcome to BetterShot for Linux. This prompter shows your script near the top of the screen while you record.\nScroll it with the mouse wheel or the arrow keys, one line at a time, and keep talking."}'
scenario shots3d light '{"bs_onboardingSeenVersion": 2, "bs_playSound": false, "afterCapture.recording.save": false}'
for appearance in light dark; do
  scenario cursor "$appearance" "{\"bs_appAppearance\": \"$appearance\", \"bs_onboardingSeenVersion\": 2, \"bs_playSound\": false, \"afterCapture.recording.save\": false}"
done
echo "Snapshots: $OUT"
