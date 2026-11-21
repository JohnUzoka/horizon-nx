#!/bin/sh
# Run with devkitPro's nacptool and elf2nro on PATH. Does not compile the game.
# Usage: sh packaging/switch/pack.sh <horizon-client ELF> <output.nro>
set -eu

if [ "$#" -ne 2 ]; then
    printf 'Usage: %s <horizon-client ELF> <output.nro>\n' "$0" >&2
    exit 2
fi

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
nacp=$(mktemp)
trap 'rm -f "$nacp"' EXIT
nacptool --create Horizon horizon-rust "${HX_NACP_VERSION:-dev}" "$nacp"
elf2nro "$1" "$2" --nacp="$nacp" --icon="${HX_NRO_ICON:-$here/icon.jpg}"
