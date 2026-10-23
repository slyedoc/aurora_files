#!/usr/bin/env bash
# Fetches every MakeHuman community asset pack into raw/make_human, never overwriting a file
# already there; zips are kept in raw/make_human_packs so a rerun only downloads new releases.
# Then run scripts/make_human_library.sh to copy the library into zero.
#
#   scripts/make_human_packs.sh [DEST]
#     DEST  raw/make_human (default)
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
dest="${1:-$here/raw/make_human}"
cache="$here/raw/make_human_packs"
site=https://static.makehumancommunity.org/assets/assetpacks

mkdir -p "$dest" "$cache"
pages=$(curl -sfL "$site/index.html" | grep -oE 'href="/assets/assetpacks/[a-z0-9_]+\.html"' | sed -E 's|.*/([a-z0-9_]+\.html)"|\1|' | grep -vE '^(index|faq)\.html$' | sort -u)
for page in $pages; do
  url=$(curl -sfL "$site/$page" | grep -oE 'https://files\.makehumancommunity\.org/[^"]+\.zip' | head -1 || true)
  if [ -z "$url" ]; then echo "skip ${page%.html} (no zip)"; continue; fi
  zip="$cache/$(basename "$url")"
  [ -s "$zip" ] || { echo "fetch $(basename "$url")"; curl -sfL -o "$zip.part" "$url" && mv "$zip.part" "$zip"; }
  unzip -qn "$zip" -d "$dest"
done
echo "$(ls "$cache" | wc -l) packs in $cache, unpacked into $dest"
