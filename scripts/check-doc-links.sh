#!/usr/bin/env bash
# Fail when a relative link in a tracked markdown file points at nothing.
#
# A document that cites a path which does not exist is wrong in a way no
# reader can see from the page. The harness this project grew out of had at
# least fourteen such citations, one of them in every reviewer's instructions.
#
# Checked: inline links of the form [text](target) whose target is a path in
# this repository. Not checked: absolute URLs, mail links, and bare anchors.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

status=0
checked=0

while IFS= read -r file; do
  dir=$(dirname "$file")
  while IFS= read -r target; do
    case "$target" in
      ''|http://*|https://*|mailto:*|'#'*) continue ;;
    esac
    path=${target%%#*}
    path=${path%%\?*}
    [ -n "$path" ] || continue
    checked=$((checked + 1))
    if [ "${path#/}" != "$path" ]; then
      resolved=".$path"
    else
      resolved="$dir/$path"
    fi
    if [ ! -e "$resolved" ]; then
      echo "$file: link target does not exist: $target" >&2
      status=1
    fi
  done < <(grep -oE '\]\([^) ]+\)' "$file" | sed -E 's/^\]\(//; s/\)$//' || true)
done < <(git ls-files --cached --others --exclude-standard -- '*.md')

if [ "$status" -ne 0 ]; then
  exit 1
fi

echo "checked $checked relative links, all resolve"
