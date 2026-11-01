#!/usr/bin/env bash
# Builds an AppImage of BetterShot for Linux. Run from linux/ after
# `cargo build --release` (or through `make appimage`).
#
# Needs appimagetool (https://github.com/AppImage/appimagetool/releases) on
# PATH, or APPIMAGETOOL pointing at it. Without it the AppDir is still
# assembled under target/appimage/BetterShot.AppDir, so packagers can
# inspect it. The AppImage bundles only BetterShot's binary and data: GTK 4,
# libadwaita, GStreamer and its plugins, the portals, and Tesseract come from
# the host, as they do for the .deb.
set -euo pipefail

cd "$(dirname "$0")/../.."
APP_ID=site.bettershot.BetterShot
VERSION="$(python3 -c 'import json,sys; print(json.load(open("../version.json"))["version"])')"
OUT=target/appimage
APPDIR="$OUT/BetterShot.AppDir"
BIN=target/release/bettershot
[[ -x "$BIN" ]] || { echo "Build first: cargo build --release" >&2; exit 1; }

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr"
make install DESTDIR="$PWD/$APPDIR" PREFIX=/usr >/dev/null
cp "data/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
cp "data/icons/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
ln -sf "$APP_ID.png" "$APPDIR/.DirIcon"
cat > "$APPDIR/AppRun" <<'RUN'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "$0")")"
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
exec "$HERE/usr/bin/bettershot" "$@"
RUN
chmod +x "$APPDIR/AppRun"
echo "AppDir assembled at $APPDIR"

TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
if [[ -z "$TOOL" ]]; then
  echo "appimagetool not found; set APPIMAGETOOL to build BetterShot-$VERSION-x86_64.AppImage" >&2
  exit 0
fi
ARCH="${ARCH:-$(uname -m)}" "$TOOL" "$APPDIR" "$OUT/BetterShot-$VERSION-$(uname -m).AppImage"
echo "Wrote $OUT/BetterShot-$VERSION-$(uname -m).AppImage"
