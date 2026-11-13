#!/usr/bin/env bash
# File size budget: no tracked source, feature or YAML file may pass LIMIT lines.
# A file already over the limit is listed in the baseline ("<path> <lines>")
# and may only shrink; the baseline is lowered in the same change. `--write`
# rewrites the baseline from the tree.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

LIMIT=1000
BASELINE=.github/file-size-baseline.txt

fail() { echo "::error file=${2:-$BASELINE}::$1"; status=1; }

declare -A now
while IFS= read -r -d '' file; do
  lines=$(wc -l < "$file")
  lines=${lines//[[:space:]]/}
  [[ $lines =~ ^[0-9]+$ ]] || { echo "::error file=$file::cannot count its lines"; exit 1; }
  if [ "$lines" -gt "$LIMIT" ]; then now[$file]=$lines; fi
done < <(git -c core.quotePath=false ls-files -z '*.rs' '*.feature' '*.yaml' '*.yml' ':!.github/')

if [ "${1:-}" = "--write" ]; then
  for file in "${!now[@]}"; do echo "$file ${now[$file]}"; done | sort > "$BASELINE"
  echo "wrote $BASELINE"
  exit 0
fi

status=0
declare -A base
while IFS= read -r line || [ -n "$line" ]; do
  line=${line%$'\r'}
  [ -z "$line" ] && continue
  file=${line% *}
  lines=${line##* }
  [[ $lines =~ ^[0-9]+$ ]] || { fail "malformed baseline line: $line"; continue; }
  base[$file]=$lines
done < "$BASELINE"

for file in "${!now[@]}"; do
  lines=${now[$file]}
  if [ -z "${base[$file]:-}" ]; then
    fail "$lines lines, the limit is $LIMIT; split it (a new file is never added to the baseline)" "$file"
  elif [ "$lines" -gt "${base[$file]}" ]; then
    fail "grew from ${base[$file]} to $lines lines; an over-limit file may only shrink" "$file"
  elif [ "$lines" -lt "${base[$file]}" ]; then
    fail "shrank to $lines lines; lower its baseline entry from ${base[$file]} (scripts/file-budget.sh --write)" "$file"
  fi
done

for file in "${!base[@]}"; do
  if [ -z "${now[$file]:-}" ]; then
    fail "$file is no longer over $LIMIT lines or no longer exists; remove its entry (scripts/file-budget.sh --write)"
  fi
done
exit "$status"
