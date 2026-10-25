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
rm -f "$dmg"
hdiutil create -volname Quarry -srcfolder "$stage" -format UDZO "$dmg"
rm -rf "$stage"

codesign --force -s - "$dmg"
echo "$dmg"
