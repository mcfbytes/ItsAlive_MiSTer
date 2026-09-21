#!/bin/sh
#
# Regenerate tests/golden/pll.json from tests/golden/pll.c, in place.
#
# pll.c holds getPLLdiv(), findPLLpar() and setPLL() transcribed from
# Main_MiSTer's video.cpp; compiling and running it is the only way the golden
# numbers are allowed to be produced.  Nobody hand-computes them.
#
# No build.rs, no `cc` crate, no new Cargo dependency: this is a plain shell
# script a reviewer runs, and the JSON it writes is checked in.
#
# -ffp-contract=off keeps the host compiler from fusing any multiply-add, so
# the C and the Rust agree bit for bit on the doubles.  There is nothing to
# fuse in these expressions anyway; it is there so a reviewer does not have to
# check that.
#
# Usage:  tests/golden/gen.sh   [CC=clang]
#
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT HUP TERM

"${CC:-cc}" -std=c11 -O2 -Wall -Wextra -ffp-contract=off \
	-o "$tmp/pll" "$here/pll.c"

"$tmp/pll" >"$tmp/pll.json"
cat "$tmp/pll.json" >"$here/pll.json"

echo "wrote $here/pll.json"
