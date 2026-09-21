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
| 3 | First light on the rig | `hdmi` gives the menu core's picture; `up` + `say` gives text on HDMI; `image` puts a picture on it with red where red belongs; results logged | rig + this repo's `docs/testlogs/` |
| 4 | Buildroot package + installer integration | package builds in the installer config; QEMU installer test still passes with the tool absent and present-but-failing; card test on the rig shows the splash | Buildroot_MiSTer |
| 5 | Hardening | direct-draw fallback if fbcon does not bind; `leds`; 480p verified; a second monitor | this repo |

Phases 0 to 2 are host-only and can run as parallel agents. Phase 3 is a
human-in-the-loop rig session. Phase 4 lands in the other repo and is gated
on phase 3.

## 2. Unknowns only the rig can settle

**All eleven were settled on 2026-09-21** — see
[`testlogs/2026-09-21-rig-first-light.md`](testlogs/2026-09-21-rig-first-light.md)
for the commands, the register dumps and what the monitor showed. Each is kept
below with its original reasoning intact, so that a wrong prediction stays
visible as a wrong prediction, followed by what the rig returned.

1. **Does fbcon bind to `MiSTer_fb` and follow the sysfs mode change without
   Main?** Expected yes: stock's script terminal relies on exactly this, and
   `CONFIG_FRAMEBUFFER_CONSOLE=y`, `CONFIG_VT=y`, `CONFIG_FONT_8x16=y` are in
   our resolved kernel config. If no: `say` gets a direct-draw path (phase 5)
   that renders an embedded 8x16 font into `/dev/fb0` itself.

   **Answered 2026-09-21: yes.** `vtcon1` is `(M) frame buffer device` with
   `bind=1`, and `say` painted both lines. No direct-draw path needed.
2. **Are the fabric's DDR read ports out of reset when U-Boot, not Main,
   loaded the core?** Expected yes: stock U-Boot's boot command runs
   `bridge enable` on the same line that loads the bitstream --
   `fpgaload` in `include/configs/socfpga_de10_nano.h:47` of
   MiSTer-devel/U-Boot_MiSTer, traced in ARCHITECTURE §1. If no: the frame reader scans garbage or hangs
   the AXI; step 3 in `hdmi` would still work, so the picture would be the
   core's own and `fb enable` gets a bridge-state check.

   **Answered 2026-09-21: yes.** `fb enable` gave clean black, not garbage, and
   `0xFFC25080` read `0x00003FFF` before anything ran. No bridge check added.
3. **Which `/dev/i2c-N` carries the ADV7513 in the installer kernel?**
   Expected `/dev/i2c-1` as in stock, discovered by probing 0 to 2. The
   installer kernel is our normal kernel relinked with a different
   initramfs, so the DT is the same.

   **Answered 2026-09-21: `/dev/i2c-1`**, chosen unaided from three candidates,
   no `AmbiguousBus`. Checked on the installed system only; the installer
   kernel remains inferred from the shared DT.
4. **Does the installer's `console=ttyS0` cmdline leave `/dev/tty1` usable
   for `say`?** Expected yes: VT devices exist independent of which console
   gets printk. If no: same fallback as 1.

   **Answered 2026-09-21: yes**, on the installed system. Untested under the
   installer's `console=ttyS0`, which is Phase 4's to confirm.
5. **Does the menu core light the I/O board Power LED before Main
   attaches?** From `sys_top.v` and `Menu.sv` it should breathe (the core
   drives `LED_POWER[1]=1` with a PWM sawtooth on `[0]` while `FB` is off).
   Not needed by this tool, but worth one glance during phase 3 because it
   answers Buildroot_MiSTer ADR 0020 §7's open question for free.

   **Answered 2026-09-21: yes** — observed breathing throughout, with no Main
   attached. ADR 0020 §7 can be closed on this.
6. **Is the core actually found in reset, so that the GPO-zero store
   matters?** `Mailbox::new` writes GPO = 0 because releasing a latched
   core reset needs `GPO[31:30]` sampled `2'b00` then `2'b10` (ARCHITECTURE
   §2). Whether the reset *is* latched after a U-Boot configuration cannot
   be settled by reading: if the FPGA manager's GPO happens to read 0 at HPS
   boot, our first write would have performed the transition by luck. The
   store is correct either way and costs one word. If the screen is dark,
   this is not the thing to suspect first — but it is worth confirming the
   store is on the wire before suspecting anything else.

   **Answered 2026-09-21: unprovable, as expected.** GPO read `0x00000000` at
   cold boot, so our store was a no-op on this path and its necessity stays
   unsettled. It costs one word and is correct either way; keep it.
7. **Does `0x94 = 0x00` cost anything visible?** ARCHITECTURE §4 records it
   as our one deliberate byte divergence from stock in the 92. Nothing here
   reads the ADV7513 INT pin, so expect no difference; verify rather than
   assume, and if a sink refuses to lock, try `0xC0` before suspecting the
   tables.

   **Answered 2026-09-21: nothing.** The sink locked and held for the whole
   session. Keep `0x00`.
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

   **Answered 2026-09-21: it locks.** We stayed at `0x40 = 0x00` across `hdmi`,
   `fb enable`, `say` and two blits with no loss of sync, and `0x42` reached
   Main's own `0xf8`. The faithful sequence stays unimplemented, on evidence.
9. **Does fbcon fight `itsalive image` for the same pixels?** They write
   the same `/dev/fb0`: `say` goes through the console and `image` writes it
   directly (ARCHITECTURE §5). Expect the last writer to win, and expect
   fbcon's cursor to repaint one character cell on a timer even with nothing
   being written — `fbcon_cursor_blink` is on by default
   (`fbcon.c:177`, `:416`). If a blinking block sits on the splash, turn it
   off with `echo 0 > /sys/class/graphics/fbcon/cursor_blink`
   (`fbcon.c:3261-3321`) and record whether that was enough, or whether the
   console has to be given up altogether (`setterm -cursor off`,
   `con2fbmap`). This is the question that decides whether the installer can
   mix `say` and `image` or has to choose one.

   **Answered 2026-09-21: it can mix them.** The cursor did land on the image,
   and `echo 0 > /sys/class/graphics/fbcon/cursor_blink` was enough on its own
   — no `con2fbmap`, no `KD_GRAPHICS`. The knob is volatile (back to `1` after
   reboot), so the installer sets it per boot and never restores it.
10. **Does `write(2)` to `/dev/fb0` reach the fabric's frame reader?** It
   should: `.fb_write = fb_sys_write` (`MiSTer_fb.c:137`) copies into
   `screen_base`, which is a `memremap(..., MEMREMAP_WT)` write-through
   mapping of the reserved DDR the frame reader scans (`MiSTer_fb.c:261`), so
   there is nothing to flush. If a written image does not appear but `say`
   does, the difference is the offset: fbcon starts at `screen_base` too, so
   suspect the geometry read-back before suspecting the write.

   **Answered 2026-09-21: yes.** 3686400 bytes landed in 0.132 s, ~27 MB/s, and
   the picture was correct to the pixel.

11. **Before blaming this tool for a dark screen**, check the f2sdram
   bridges are up: `devmem2 0xFFC25080` should read `0x00003FFF`. They are
   raised by U-Boot's `bridge enable`, not by anything in userspace, and
   without them the fabric cannot reach HPS DDR at `0x22000000` at all —
   `hdmi` would still work and `fb enable` would scan nothing.

   **Answered 2026-09-21:** `0x00003FFF` both with Main running and at cold
   boot with no Main. U-Boot raises them and nothing lowers them.

A note on diagnosis, because three of these produce the same symptom: a
missing §1 step 3, an unreleased core reset, and painting before the sysfs
knob all give a black or NO-SIGNAL screen with every host-side signal green
and every exit code 0. Settle them in that order — the commit word is on the
wire and cheapest to confirm.

## 3. Rig protocol (phase 3)

**Run on 2026-09-21; steps 1-6b, 6c's full-screen half and 8 passed.** The
results are in
[`testlogs/2026-09-21-rig-first-light.md`](testlogs/2026-09-21-rig-first-light.md),
which also lists the four steps that were *not* run and remain open: `--size`
centring on hardware (6c), `say` after `image` (6d), `image` before
`fb enable` (6's trailing paragraph), and 480p (7). The protocol is kept here
in full because those four still need running, and because a second board or a
second core reruns all of it.

Rig: a DE10-Nano on the LAN, reachable over SSH as root, with a serial console
and a netconsole receiver available. Runs from the *installed* system, which
already has the menu core in the fabric:

1. Copy the cross-built `itsalive` to `/tmp` on the rig (not to the exFAT
   card; nothing persists).
2. Stop Main. `killall MiSTer` is enough and is permanent until the next
   boot: `/etc/inittab:61` is `::sysinit:/media/fat/MiSTer &`, which is
   **`sysinit`, not `respawn`** — Main is started once and backgrounded, and
   nothing restarts it. (This step previously said to find and hold a respawn
   line. There is no respawn line; the 2026-09-21 session corrected it.)
   Stricter, and what that session actually did: rename the binary to
   `MiSTer.off` and power-cycle, which also exercises the cold-boot path the
   installer will really run on. Confirm HDMI goes to whatever the core shows
   without Main. Note it.
3. `itsalive probe --json` → log.
4. `itsalive hdmi` → expect the menu core's own picture. Log dmesg, exit
   code, the monitor's reported mode. If the monitor says NO SIGNAL rather
   than showing black, the commit word (§1 step 3) is the first suspect:
   confirm `UIO_BUT_SW` went out after the three mode registers.
5. `itsalive fb enable` then `itsalive say --clear "It's alive"` → expect
   text. Log the same.
6. `itsalive image` — the new subcommand, in four steps, because each one
   fails differently:
   a. `cat /sys/module/MiSTer_fb/parameters/mode` → expect
      `8888 1 1280 720 5120`. This is the read-back the command itself does,
      and it is also the first evidence anywhere that the mode write of §5
      actually took: `mode_set` returns 0 even when the driver never probed.
      If it is empty, stop — nothing below can work and `image` will say so.
      `1280 720` is load-bearing in that line: the driver's *own* probe
      defaults are `8888 1 640 480 2560` (`MiSTer_fb.c:31` for `rb`/`format`,
      `:150-152` for the geometry, `:227` for the 8888), and they pass every
      check `image` makes. A knob that says 640x480 is the one reading that
      proves nothing.
   b. On the build host, make a test image whose four corners are different
      colours and whose left half is **red**, then copy it over:
      `ffmpeg -i test.png -vf scale=1280:720 -f rawvideo -pix_fmt bgra test.raw`.
      Red on the left is the whole point: if the screen shows blue there, the
      byte order is wrong and ARCHITECTURE §5's `rb` reasoning is wrong with
      it. A greyscale or monochrome test image proves nothing.
   c. `itsalive image test.raw` → expect the picture, full screen. Then
      `itsalive image --size 320x200 --clear small.raw` → expect it centred
      with black around it. Check the edges: a sheared or diagonally
      displaced picture means the stride, a picture offset by a constant
      means the centring. **Exit 0 is not a pass here** — it says the bytes
      reached `/dev/fb0`, nothing more — so the photograph is the result and
      the exit code is only a filter. Copy any stderr line about the stride
      into the log verbatim: it means the fbdev and the frame reader were
      given different numbers, which is the one case where a picture that is
      byte-for-byte right for the driver is sheared on the screen.
   d. `itsalive say hello` afterwards → expect text over the picture, and
      watch for a blinking cursor block (§2 unknown 9). Log what it does to
      the image; that answer decides what the installer is allowed to call.
   Also run `itsalive image` *before* `fb enable` on a fresh boot once, and
   log the knob and the exit code together, because which of two things
   happens is itself the finding:
   - the knob is **empty** — `MiSTer_fb` never probed, `mode_get` returns 0
     bytes under `if(p_fbdev)` — and `image` exits 2 naming `fb enable`; or
   - the knob is **`8888 1 640 480 2560`**, the driver's probe defaults, and
     then a 1280x720 source is refused for its *size* while
     `itsalive image --size 320x200 small.raw` is **accepted and exits 0 with
     nothing on the screen**, because the frame reader is still on the core's
     own buffer.
   The second is the expected one on the installed system, and it is why the
   installer must not read `image`'s exit code as "a picture is up". If we
   ever want that guarantee, the only signal that carries it is
   `UIO_SET_FBUF`'s reply word, which `fb enable` already reads and `image`
   deliberately does not.
7. `itsalive hdmi --mode 480p` after `fb disable` → expect a picture again.
8. Restart Main (or reboot). Confirm Main comes up normally after the tool
   touched the fabric; log it. **Passed 2026-09-21, and more strongly than
   this asks:** Main not only restarted, it loaded a *different* bitstream
   into the fabric we had been driving and ran a core, with every ADV7513
   register back to its baseline and nothing angry in dmesg. This is the step
   that says the tool is safe to hand to an installer.
9. Write `docs/testlogs/YYYY-MM-DD-rig-first-light.md` with the
   `probe --json` output, every exit code, and photos or a description of
   what the monitor showed at each step.

The `mode` read-back in step 6a is worth running at every earlier step too: it
costs nothing, and it is the one line that separates "the knob was written" from
"the driver took it" — as far as it goes, which is to the edge of the SoC. It
says the driver probed and what layout it registered, and nothing at all about
whether the fabric's frame reader is pointed at the HPS buffer. At 1280x720 it
at least cannot be confused with the probe defaults; at 640x480 it can.

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
  The binary itself costs the installer image roughly 170-210 KB depending
  on the kernel's initramfs compression (443 KB raw, 212 KB gzip -9, 169 KB
  xz -9, measured on the phase 2 tree), and that lands in the kernel image
  because the cpio is embedded in it.
- **Watch the linker override.** `.cargo/config.toml` pins
  `linker = "rust-lld"` for `armv7-unknown-linux-musleabihf`, which is what
  lets this repo's own CI cross-link with nothing installed. Buildroot will
  want to link with *its* toolchain, and cargo reads that file from the
  package source, so the two may fight. Unverified — nobody has built this
  under Buildroot yet. If it does fight, the fix is to keep the override out
  of the packaged tarball or override it back in `itsalive.mk`, not to drop
  `+crt-static`, which is mandatory (ARCHITECTURE §6).
- Installer `init`: one `itsalive up` after the payload is staged in RAM,
  then `itsalive say` at each `splash_step`, all guarded, all under
  `timeout`, all `|| true`. `scripts/test-installer-splash.sh` and
  `scripts/test-sdcard-install.sh` must pass with the binary absent (QEMU
  has no fabric) and with a stub that returns exit 10.
- **The reformat must leave `menu.rbf` where U-Boot can find it.** This is a
  dependency the splash work inherits rather than creates, and it is easy to
  miss because it does not bite during the install. The FPGA is configured at
  boot, from the *old* card, before the installer's initramfs exists
  (ARCHITECTURE §1) -- so `itsalive` works throughout the reformat no matter
  what happens to the filesystem underneath it. It bites on the *next* boot:
  if the freshly written card has no `menu.rbf` at a path `load mmc 0:1` can
  reach, U-Boot configures no fabric, and then neither Main nor this tool can
  put up a picture. `probe` would exit 10, correctly and uselessly. Note also
  that U-Boot reads that partition with ChaN's FatFs, patched in by the MiSTer
  fork with `_FS_EXFAT 1`, not with stock U-Boot's FAT driver; an exFAT the
  kernel mounts is not automatically an exFAT that U-Boot reads. Worth an
  explicit assertion in `scripts/test-sdcard-install.sh` rather than trust.
- Docs: ADR 0020 §6 amended with the outcome; `docs/user/sdcard-flashing.md`
  loses "the screen stays blank the whole time" once the card test confirms
  the splash. The "static-musl" wording for the installer in ADR 0020,
  `scripts/mk-sdcard.sh` and `docs/ci.md` needs no change: it is accurate.
  `configs/mister_installer_defconfig` sets `BR2_TOOLCHAIN_BUILDROOT_MUSL=y`
  and `BR2_STATIC_LIBS=y`, and `git log --all -S` on that first line finds
  exactly one commit — the one that added it. The installer rootfs has always
  been static musl and no branch changes it. What *did* consolidate onto the
  main glibc toolchain is the installer kernel re-link, which is a different
  step (`scripts/mk-sdcard.sh` step 2); do not let the two be confused, in
  either direction. See ARCHITECTURE §6.

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
