#!/bin/sh
# Trunk post-build hook: the server's Content-Security-Policy forbids inline
# scripts, so move trunk's inline module loader into /loader.js.
set -eu
dir="${TRUNK_STAGING_DIR:?}"
html="$dir/index.html"
awk -v out="$dir/loader.js" '
  /<script type="module"[^>]*>/ && !done { inside = 1; sub(/.*<script type="module"[^>]*>/, ""); }
  inside {
    if (index($0, "</script>")) {
      line = $0; sub(/<\/script>.*/, "", line); print line > out;
      inside = 0; done = 1;
      print "<script type=\"module\" src=\"/loader.js\"></script>";
      next;
    }
    print > out; next;
  }
  { print }
' "$html" > "$html.tmp"
mv "$html.tmp" "$html"
grep -q 'src="/loader.js"' "$html" || { echo "extract-loader: no inline loader found" >&2; exit 1; }
