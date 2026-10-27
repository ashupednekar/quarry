#!/bin/sh
# Builds dist/Quarry.app, ad-hoc signs it, and wraps it in dist/Quarry-<version>.dmg.
set -eu
cd "$(dirname "$0")/.."

version=$(cargo pkgid -p quarry-app | sed 's/.*[#@]//')
dmg="dist/Quarry-$version.dmg"

(cd crates/quarry-app && cargo packager --release -f app)

codesign --force --deep -s - dist/Quarry.app
codesign --verify --deep --strict dist/Quarry.app

# Drag-to-install layout: the app next to an Applications shortcut
stage=$(mktemp -d)
cp -R dist/Quarry.app "$stage/"
ln -s /Applications "$stage/Applications"

# Volume icon: the custom-icon flag must be set on the mounted volume root, not the source
# folder (hdiutil -srcfolder drops it), so build read-write, flag it, then compress
cp assets/icon/quarry.icns "$stage/.VolumeIcon.icns"
rw=$(mktemp -u).dmg
hdiutil create -volname Quarry -srcfolder "$stage" -format UDRW "$rw"
rm -rf "$stage"
mnt=$(hdiutil attach -nobrowse -noautoopen "$rw" | awk -F'\t' '/\/Volumes\//{print $NF}')
SetFile -a C "$mnt"
hdiutil detach "$mnt"
rm -f "$dmg"
hdiutil convert "$rw" -format UDZO -o "$dmg"
rm -f "$rw"

# DMG stays unsigned: a signed DMG gets its own Gatekeeper prompt on top of the app's
echo "$dmg"
