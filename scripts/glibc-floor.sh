#!/usr/bin/env bash
# Fails when a Linux binary needs a glibc newer than the floor (default 2.34).
# usage: scripts/glibc-floor.sh <binary> [max-version]
# readelf -V (the version needs), not objdump -T: it reads an ELF of any
# architecture, so the same check runs on an x86_64 and on an aarch64 binary from
# either host, and it also sees a need no symbol carries (GLIBC_ABI_DT_RELR).
set -euo pipefail
bin=${1:?usage: glibc-floor.sh <binary> [max-version]}
max=${2:-2.34}
[ -r "$bin" ] || { echo "$bin: not readable" >&2; exit 1; }
needs=$(readelf -V -W "$bin")
# A GLIBC_ABI_* need is newer than any floor this script knows.
if grep -q 'GLIBC_ABI_' <<<"$needs"; then
  echo "$bin needs a GLIBC_ABI_ version (e.g. -z pack-relative-relocs), above the floor $max" >&2
  exit 1
fi
found=$(grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' <<<"$needs" | sed 's/^GLIBC_//' | sort -Vu | tail -n 1 || true)
# No version at all is a wrong path or a static binary: never a silent pass.
if [ -z "$found" ]; then
  echo "$bin: no GLIBC_ symbol version found" >&2
  exit 1
fi
if [ "$(printf '%s\n%s\n' "$found" "$max" | sort -V | tail -n 1)" != "$max" ]; then
  echo "$bin needs glibc $found, above the floor $max" >&2
  exit 1
fi
echo "$bin: highest glibc $found, floor $max"
