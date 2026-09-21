# Plan

The tool is the experiment. Nothing about the fabric side can be proven in
QEMU, so the plan front-loads everything that *can* be proven on the host
(word composition, PLL arithmetic, table transcription) and makes the first
hardware run as small and as diagnosable as possible.

## 1. Phases

| Phase | Goal | Proof | Where |
|-------|------|-------|-------|
| 0 | Repo skeleton, CI, cross-build | `cargo test` on host, `cargo build` for both ARM targets in CI | this repo |
| 1 | Pure core: mailbox framing, PLL solver, SET_VIDEO and SET_FBUF composers, ADV7513 tables | golden-vector tests derived from Main_MiSTer's arithmetic; recording-fake tests for the command bursts | this repo |
| 2 | Hardware paths and CLI | builds, `probe` runs on the rig and reports sanely | this repo |
| 3 | First light on the rig | `hdmi` gives the menu core's picture; `up` + `say` gives text on HDMI; results logged | rig + this repo's `docs/testlogs/` |
| 4 | Buildroot package + installer integration | package builds in the installer config; QEMU installer test still passes with the tool absent and present-but-failing; card test on the rig shows the splash | Buildroot_MiSTer |
| 5 | Hardening | direct-draw fallback if fbcon does not bind; `leds`; 480p verified; a second monitor | this repo |

Phases 0 to 2 are host-only and can run as parallel agents. Phase 3 is a
human-in-the-loop rig session. Phase 4 lands in the other repo and is gated
on phase 3.

## 2. Unknowns only the rig can settle

Listed with the expected answer and what changes if it is wrong.

1. **Does fbcon bind to `MiSTer_fb` and follow the sysfs mode change without
   Main?** Expected yes: stock's script terminal relies on exactly this, and
   `CONFIG_FRAMEBUFFER_CONSOLE=y`, `CONFIG_VT=y`, `CONFIG_FONT_8x16=y` are in
   our resolved kernel config. If no: `say` gets a direct-draw path (phase 5)
   that renders an embedded 8x16 font into `/dev/fb0` itself.
2. **Are the fabric's DDR read ports out of reset when U-Boot, not Main,
   loaded the core?** Expected yes: stock U-Boot's boot command runs
   `bridge enable` after loading the bitstream (Buildroot_MiSTer
   `docs/boot-chain.md` §6). If no: the frame reader scans garbage or hangs
   the AXI; step 3 in `hdmi` would still work, so the picture would be the
   core's own and `fb enable` gets a bridge-state check.
3. **Which `/dev/i2c-N` carries the ADV7513 in the installer kernel?**
   Expected `/dev/i2c-1` as in stock, discovered by probing 0 to 2. The
   installer kernel is our normal kernel relinked with a different
   initramfs, so the DT is the same.
4. **Does the installer's `console=ttyS0` cmdline leave `/dev/tty1` usable
   for `say`?** Expected yes: VT devices exist independent of which console
   gets printk. If no: same fallback as 1.
5. **Does the menu core light the I/O board Power LED before Main
   attaches?** From `sys_top.v` and `Menu.sv` it should breathe (the core
   drives `LED_POWER[1]=1` with a PWM sawtooth on `[0]` while `FB` is off).
   Not needed by this tool, but worth one glance during phase 3 because it
   answers Buildroot_MiSTer ADR 0020 §7's open question for free.
6. **Is the core actually found in reset, so that the GPO-zero store
   matters?** `Mailbox::new` writes GPO = 0 because releasing a latched
   core reset needs `GPO[31:30]` sampled `2'b00` then `2'b10` (ARCHITECTURE
   §2). Whether the reset *is* latched after a U-Boot configuration cannot
   be settled by reading: if the FPGA manager's GPO happens to read 0 at HPS
   boot, our first write would have performed the transition by luck. The
   store is correct either way and costs one word. If the screen is dark,
   this is not the thing to suspect first — but it is worth confirming the
   store is on the wire before suspecting anything else.
7. **Does `0x94 = 0x00` cost anything visible?** ARCHITECTURE §4 records it
   as our one deliberate byte divergence from stock in the 92. Nothing here
   reads the ADV7513 INT pin, so expect no difference; verify rather than
   assume, and if a sink refuses to lock, try `0xC0` before suspecting the
   tables.
8. **Does the sink lock without the SPD InfoFrame?** Stock's steady state
   has `0x40` bit 6 set, through a read-modify-write on a path we do not
   walk, so stock settles at `0x40 = 0x40` and we stay at `0x00`. The C
   cannot settle whether it is *needed*: it puts a picture up as much as
   500 ms before that write happens. Bring up with `0x00`; if the monitor
   syncs and then drops, or refuses audio-capable sinks, that is the first
   thing to try. Adding it properly means the whole faithful sequence
   (read-modify-write on `0x39`, then the bracketed payload on the `0x38`
   sub-map of the *already-pinned* bus), never a bare bit-set — enabling a
   packet with no packet memory behind it is a state the C deliberately
   refuses.
9. **Before blaming this tool for a dark screen**, check the f2sdram
   bridges are up: `devmem2 0xFFC25080` should read `0x00003FFF`. They are
   raised by U-Boot's `bridge enable`, not by anything in userspace, and
   without them the fabric cannot reach HPS DDR at `0x22000000` at all —
   `hdmi` would still work and `fb enable` would scan nothing.

A note on diagnosis, because three of these produce the same symptom: a
missing §1 step 3, an unreleased core reset, and painting before the sysfs
knob all give a black or NO-SIGNAL screen with every host-side signal green
and every exit code 0. Settle them in that order — the commit word is on the
wire and cheapest to confirm.

## 3. Rig protocol (phase 3)

Rig: the DE10-Nano at `192.168.0.160` (`mister.lan`), SSH key
`mister_rig_ed25519`, serial console available, netconsole receiver on
`ubuntu01`. Runs from the *installed* system, which already has the menu
core in the fabric:

1. Copy the cross-built `itsalive` to `/tmp` on the rig (not to the exFAT
   card; nothing persists).
2. Stop the daemon and its respawn (`killall MiSTer`; check `inittab` for
   the respawn line and hold it). Confirm HDMI goes to whatever the core
   shows without Main. Note it.
3. `itsalive probe --json` → log.
4. `itsalive hdmi` → expect the menu core's own picture. Log dmesg, exit
   code, the monitor's reported mode. If the monitor says NO SIGNAL rather
   than showing black, the commit word (§1 step 3) is the first suspect:
   confirm `UIO_BUT_SW` went out after the three mode registers.
5. `itsalive fb enable` then `itsalive say --clear "It's alive"` → expect
   text. Log the same.
6. `itsalive hdmi --mode 480p` after `fb disable` → expect a picture again.
7. Restart Main (or reboot). Confirm Main comes up normally after the tool
   touched the fabric; log it.
8. Write `docs/testlogs/YYYY-MM-DD-rig-first-light.md` with the
   `probe --json` output, every exit code, and photos or a description of
   what the monitor showed at each step.

If step 4 shows nothing: try the ADV7513 mode registers (`0x17/0x3B/0x3C`),
then the 480p mode, then compare against a `strace -e ioctl` of stock Main's
start-up on the same board. Each divergence goes in the test log.

## 4. Buildroot integration (phase 4, Buildroot_MiSTer repo)

- `package/itsalive/`: `itsalive.mk` using `$(cargo-package)` with a GitHub
  tag tarball, `.hash`, `Config.in`, license `GPL-3.0-or-later`. Model on
  `package/azcopy/` for the file layout and comment voice, and on upstream
  Buildroot's `package/ripgrep/` for the cargo shape.
- The installer config gains `BR2_PACKAGE_ITSALIVE=y`; this pulls
  `host-rust-bin`, which is a download, not a compile, but a large one.
  Measure the CI-minute cost on the first run and record it in the PR.
- Installer `init`: one `itsalive up` after the payload is staged in RAM,
  then `itsalive say` at each `splash_step`, all guarded, all under
  `timeout`, all `|| true`. `scripts/test-installer-splash.sh` and
  `scripts/test-sdcard-install.sh` must pass with the binary absent (QEMU
  has no fabric) and with a stub that returns exit 10.
- Docs: ADR 0020 §6 amended with the outcome; `docs/user/sdcard-flashing.md`
  loses "the screen stays blank the whole time" once the card test confirms
  the splash. The "static-musl" wording for the installer in ADR 0020,
  `scripts/mk-sdcard.sh` and `docs/ci.md` is already stale if the installer
  has moved to glibc; fix it in whichever PR moves it.

## 5. Risks

- **Fabric protocol drift.** The opcodes and word layouts are Main_MiSTer's
  private contract with `sys_top.v`. They have been stable for years, but a
  framework change would break this tool silently. Mitigation: `probe`
  reads the framework's `UIO_GET_STRING`-free path (GPI user-mode bit only);
  a version check is a follow-up if drift ever happens. Pin the Main_MiSTer
  commit the tables were transcribed from in `docs/ARCHITECTURE.md` and
  re-diff on each Main release.
- **Two writers.** Running while Main is up corrupts both. Documented; the
  installer never has Main; the rescue use case is the operator's
  responsibility.
- **A sink that needs the mode registers or a different mode.** Two modes
  and the optional mode write cover the common cases; anything else is out
  of scope for a "do not power off" screen.
- **Rust toolchain cost in the installer build.** Known and accepted by the
  owner; the design is not Rust-specific and a C port is a drop-in if the
  cost bites.
