#!/bin/sh
# Regenerates every platform icon (macOS: needs swift, iconutil, python3 + Pillow).
# Three sources, so the cut stays visible at every size:
#   quarry-16.svg     16px, pixel-aligned with a staircase cut
#   quarry-small.svg  17–47px, simplified layers with full-pixel gaps
#   quarry.svg        48px and up, the full mark
set -e
cd "$(dirname "$0")"
tmp=$(mktemp -d)
render() {
  if [ "$1" -le 16 ]; then src=quarry-16.svg
  elif [ "$1" -lt 48 ]; then src=quarry-small.svg
  else src=quarry.svg; fi
  swift render.swift "$src" "$2" "$1"
}

# macOS: .icns from the standard iconset sizes
mkdir "$tmp/quarry.iconset"
for s in 16 32 128 256 512; do
  render $s "$tmp/quarry.iconset/icon_${s}x${s}.png"
  render $((s * 2)) "$tmp/quarry.iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$tmp/quarry.iconset" -o quarry.icns

# Linux and the runtime window icon: plain PNGs
for s in 16 32 128 256 512; do render $s "quarry-$s.png"; done

# Windows: one .ico, each size rendered from its own source rather than downscaled
for s in 16 24 32 48 64 128 256; do render $s "$tmp/ico-$s.png"; done
python3 - "$tmp" <<'PY'
import sys
from PIL import Image
t = sys.argv[1]
sizes = [16, 24, 32, 48, 64, 128, 256]
imgs = [Image.open(f"{t}/ico-{s}.png") for s in sizes]
imgs[-1].save("quarry.ico", sizes=[(s, s) for s in sizes], append_images=imgs[:-1])
PY

rm -rf "$tmp"
