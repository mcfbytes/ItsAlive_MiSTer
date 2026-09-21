#!/bin/sh
#
# Regenerate tests/golden/adv7513.json from tests/golden/adv7513.cpp, in place.
#
# adv7513.cpp holds hdmi_config_init(), hdmi_config_audio(),
# hdmi_config_set_csc() and hdmi_config_set_mode() transcribed from
# Main_MiSTer's video.cpp, and mat4x4.h beside it is upstream's matrix class
# verbatim.  Compiling and running them is the only way the golden bytes are
# allowed to be produced.  Nobody hand-computes them, and in particular nobody
# copies them out of src/adv7513.rs -- that would make the test a restatement
# of the transcription instead of a check on it.
#
# No build.rs, no `cc` crate, no new Cargo dependency: this is a plain shell
# script a reviewer runs, and the JSON it writes is checked in.
#
# It is C++ and not C because mat4x4.h is a C++ class, and the CSC chain has to
# run against that class rather than a reimplementation of it.
#
# -ffp-contract=off keeps the host compiler from fusing the multiply-adds in
# mat4x4::operator* and in the hue matrix.  A fused multiply-add keeps more
# precision than the ARM target's separate instructions would, so leaving it on
# could hide a rounding difference at exactly the place -- int16_t(x * 2048.0f),
# which truncates toward zero -- where one byte of the CSC table turns from 0x08
# into 0x07.
#
# The file names here are adv7513-prefixed on purpose: tests/golden/ is shared
# with the PLL golden vectors, and a second `gen.sh` would collide with that
# branch's for no benefit.
#
# Usage:  tests/golden/adv7513-gen.sh   [CXX=clang++]
#
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT HUP TERM

"${CXX:-c++}" -std=c++11 -O2 -Wall -Wextra -ffp-contract=off \
	-o "$tmp/adv7513" "$here/adv7513.cpp"

"$tmp/adv7513" >"$tmp/adv7513.json"
cat "$tmp/adv7513.json" >"$here/adv7513.json"

echo "wrote $here/adv7513.json"
