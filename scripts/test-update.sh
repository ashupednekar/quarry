#!/bin/sh
# Tries the auto-update flow locally on macOS: packages the current version into dist/update-test/,
# builds a signed copy versioned 99.0.0, serves its latest.json on localhost and launches the old
# copy against it. Click "Update to 99.0.0" in the status bar; Ctrl-C stops the server.
set -eu
cd "$(dirname "$0")/.."

key=~/.quarry-release/updater.key
CARGO_PACKAGER_SIGN_PRIVATE_KEY=$(cat "$key")
CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD=$(cat "$key.password")
export CARGO_PACKAGER_SIGN_PRIVATE_KEY CARGO_PACKAGER_SIGN_PRIVATE_KEY_PASSWORD
out=dist/update-test
rm -rf "$out"
mkdir -p "$out"

scripts/package-macos.sh
cp -R dist/Quarry.app "$out/"

# The newer version, bumped for this build only
cp Cargo.toml "$out/Cargo.toml.bak"
cp Cargo.lock "$out/Cargo.lock.bak"
trap 'cp "$out/Cargo.toml.bak" Cargo.toml; cp "$out/Cargo.lock.bak" Cargo.lock' EXIT
sed -i '' 's/^version = .*/version = "99.0.0"/' Cargo.toml
scripts/package-macos.sh
mv dist/Quarry.app.tar.gz dist/Quarry.app.tar.gz.sig "$out/"
cp "$out/Cargo.toml.bak" Cargo.toml
cp "$out/Cargo.lock.bak" Cargo.lock

jq -n --arg sig "$(cat "$out/Quarry.app.tar.gz.sig")" '{version: "99.0.0", notes: "Local test",
  platforms: {"macos-aarch64": {url: "http://localhost:8765/Quarry.app.tar.gz", signature: $sig, format: "app"}}}' \
  > "$out/latest.json"

QUARRY_UPDATE_URL=http://localhost:8765/latest.json "$out/Quarry.app/Contents/MacOS/quarry" &
cd "$out" && python3 -m http.server 8765
