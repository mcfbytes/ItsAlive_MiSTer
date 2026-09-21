# Tasks

Each task is sized for one Opus 5 agent run. "Inputs" are what the agent is
handed; "Done when" is checked by the reviewer, not by the agent. Tasks in the
same phase with no `Depends` line run in parallel. Every agent works on its own
branch and opens a PR; nothing lands on `master` without a review pass.

House rules for every task:

- Read `docs/ARCHITECTURE.md` first and treat it as the spec. If the code has
  to deviate, the PR changes the doc in the same commit and says why.
- Transcribe from Main_MiSTer at commit `6cda9cc` (a clone is at
  `/mnt/source/main-prs`). Cite `file:line` in a comment beside every
  transcribed constant or table.
- `libc` is the only dependency. `cargo clippy -- -D warnings` and
  `cargo fmt --check` are part of "done".
- Commit messages: imperative subject, a body that says why, and the
  `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` trailer.

## Phase 0 — skeleton

### T0.1 Crate skeleton and CI
- Inputs: this repo as it is.
- Do: `cargo init` a binary crate with a `lib.rs`, edition 2024, `libc` dep,
  the module files from ARCHITECTURE §6 as stubs with doc comments, a
  `.cargo/config.toml` with the two ARM targets, and a GitHub Actions
  workflow that runs `fmt`, `clippy`, `test` on the host and `build
  --release` for `armv7-unknown-linux-gnueabihf` and
  `armv7-unknown-linux-musleabihf` (cross linker via the `cross` crate or
  `gcc-arm-linux-gnueabihf` from apt; pick the one that needs no Docker).
  Add `rust-toolchain.toml` pinning 1.97.
- Done when: CI is green on an empty `main()` that prints usage and exits 2,
  and the two ARM binaries are uploaded as workflow artifacts.

## Phase 1 — pure core (parallel)

### T1.1 Mailbox framing
- Inputs: ARCHITECTURE §2; `fpga_io.cpp:511-519, 665-666, 688-720`, `spi.cpp:5-53`.
- Do: `mailbox.rs` with a `Mailbox<R: Regs>` generic over a two-register
  trait (`gpo_write`, `gpi_read`), implementing `spi_w`, `enable_io`,
  `disable_io`, `command(opcode, &[u16])`, and the bounded ack poll with a
  `Deadline` abstraction that tests can drive. A recording fake `Regs`
  records every GPO write and scripts GPI responses.
- Done when: tests prove the exact GPO write sequence for one word matches
  the four-step handshake, the enable bit is dropped on every error path,
  bit-31 aborts with `Error::NoBitstream`, and a never-acking fake yields
  `Error::Timeout` within the deadline.

### T1.2 PLL solver and SET_VIDEO composer
- Inputs: ARCHITECTURE §3; `video.cpp:125-149` (`vmodes[]`), `:262-318`
  (`setPLL`), plus `findPLLpar` and `getPLLdiv` wherever they live in the
  same file; `:2266-2290` (`set_video`).
- Do: `video.rs` with `Modeline`, the two modes, `solve_pll(f_out_mhz) ->
  PllBlock`, and `set_video_words(&Modeline, &PllBlock) -> [u16; 26]`.
- Done when: golden tests for 74.25 MHz and 25.175 MHz reproduce the C, M,
  K and the twelve `item[9..20]` values; the golden values are derived by
  compiling the transcribed C functions on the host in a `tests/golden/`
  harness (a `build.rs`-free `cc` invocation in a test, or a checked-in
  `.c` file plus a script that regenerates a `.json`), never by hand; the
  26-word burst for 720p is asserted word by word including the `0x4000`
  on the odd PLL entries and the absence of `0x8000`.

### T1.3 ADV7513 tables
- Inputs: ARCHITECTURE §4; `video.cpp:1180` (`hdmi_config_set_csc`),
  `:1417` (`hdmi_config_audio`), `:1462-1615` (`hdmi_config_init`),
  `:1691-1727` (`hdmi_config_set_mode`).
- Do: `adv7513.rs` table half: `INIT: &[(u8, u8)]`, `AUDIO`, `CSC` evaluated
  for the no-ini defaults with each dependent row commented, `POWER_UP`,
  `POWER_DOWN`, `mode_regs(&Modeline) -> [(u8, u8); 3]`, and a `Writes`
  iterator that yields the bulk tables in Main's order.
- Done when: a test asserts the row count and the first and last row of
  each table against the source, a test asserts `0x41` appears exactly once
  in `INIT` with `0x10`, `mode_regs` for 720p is asserted as
  `[(0x17, 0x62), (0x3B, 0x40), (0x3C, 4)]` and for 480p as `(0x3C, 1)`,
  and every row that depends on a `cfg.*` default has the default named in
  a comment.

### T1.4 SET_FBUF composer and mode knob text
- Inputs: ARCHITECTURE §5; `video.cpp:37, 49-57, 3459-3471, 3474-3530`.
- Do: `fb.rs` with `enable_words(&Modeline) -> [u16; 10]`,
  `disable_words() -> [u16; 1]`, `mode_param_line(&Modeline) -> String`.
- Done when: the 720p burst is asserted word by word (`0x8016`, `0x1000`,
  `0x2200`, `1280`, `720`, `0`, `1279`, `0`, `719`, `5120`) and the sysfs
  line is `8888 1 1280 720 5120\n`.

## Phase 2 — hardware paths and CLI

### T2.1 `hw.rs`: /dev/mem, i2c-dev, sysfs, tty
- Depends: T1.1, T1.3.
- Do: real `Regs` over an `mmap` of one page at `0xFF706000`; an `I2c`
  type over `open`/`ioctl(I2C_SLAVE)`/`I2C_SMBUS` write-byte-data and
  receive-byte, with bus discovery 0..=2 and the ambiguity rule; a sysfs
  writer; a `tty` writer. All `unsafe` here, each block commented.
- Done when: it compiles for both ARM targets, the host build gates the
  hardware paths behind `cfg(target_os = "linux")` and the unit tests for
  the pure modules still run on the host.

### T2.2 CLI and exit codes
- Depends: T1.2, T1.4, T2.1.
- Do: `main.rs` implementing ARCHITECTURE §7 exactly, `probe --json`,
  logging to stderr, every error mapped to its exit code.
- Done when: `itsalive` with no args exits 2 with usage; a test harness
  with fake `Regs`/`I2c` runs `up` end to end and asserts the full ordered
  list of writes (I2C bulk first, then SET_VIDEO, then the three mode
  registers over I2C, then SET_FBUF, then the sysfs line) and that `--off`
  writes only `0x41 = 0x50`.

## Phase 3 — first light (human + agent on the rig)

### T3.1 Rig session
- Depends: T2.2 built for the installed image's target.
- Do: PLAN §3 step by step. The agent drives SSH and serial and writes the
  test log; the human watches the monitor and says what it shows.
- Done when: `docs/testlogs/<date>-rig-first-light.md` exists with every
  step's exit code and observation, and PLAN §2 has each unknown marked
  answered or reopened.

## Phase 4 — Buildroot_MiSTer integration

### T4.1 Package
- Depends: T3.1 green, a tagged release of this repo.
- Do: `package/itsalive/` per PLAN §4; `BR2_PACKAGE_ITSALIVE=y` in the
  installer config; measure the CI cost.
- Done when: the installer image contains `/usr/bin/itsalive`, the package
  builds from a clean `dl/`, and the hash file is Renovate-managed the way
  the repo's other GitHub-tarball packages are.

### T4.2 Installer splash on HDMI
- Depends: T4.1.
- Do: wire `up` and `say` into `installer-overlay/init` under the
  never-fail rule; update the two test scripts and the docs listed in
  PLAN §4; amend ADR 0020 §6.
- Done when: both QEMU tests pass with the binary absent and with an
  exit-10 stub, and a card flashed from the PR's `sdcard.img` shows the
  splash on the rig, logged in `docs/testlogs/`.

## Phase 5 — hardening (only if phase 3 asks for it)

- T5.1 direct-draw `say` with an embedded 8x16 font, if fbcon does not bind.
- T5.2 EDID read (`0x3F` map) to pick 720p vs 480p from the sink instead of a flag.
- T5.3 `leds` subcommand (`UIO_LEDS`).
- T5.4 Framework version drift check.
