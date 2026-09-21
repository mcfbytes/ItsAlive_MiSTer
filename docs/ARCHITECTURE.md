# Architecture

This document is the contract the implementation is built to. Every register
sequence below is transcribed from the public
[Main_MiSTer](https://github.com/MiSTer-devel/Main_MiSTer) source; file:line
references are against upstream commit `6cda9cc` ("Add support for NeXT -
Ethernet and RTC"). When the transcription and Main_MiSTer disagree,
Main_MiSTer is right and this document has a bug.

## 1. What a picture needs, and why the daemon is not one of them

On a MiSTer the HDMI transmitter (an ADV7513), its I2C lines, the pixel clock
PLL, and the DDR frame reader that scans out `/dev/fb0` all live **behind the
FPGA fabric**. The framework every core is built on (`sys_top.v`) routes the
HPS I2C peripheral to the chip through the fabric
(`cyclonev_hps_interface_peripheral_i2c hdmi_i2c`), owns the PLL, and gates
HDMI output on the HPS having programmed that PLL. So:

- **With no bitstream loaded, nothing can be done from Linux.** Not even I2C
  reaches the chip. `itsalive` therefore requires a core in the fabric and
  refuses cleanly (`probe` exit code) when there is none.
- **With a core loaded, Main_MiSTer is not required.** Everything the daemon
  does to get a picture is three register sequences sent from userspace. The
  installer already has `menu.rbf` in the fabric (U-Boot loads it before
  Linux starts), so the daemon is the only missing piece, and it is a
  replaceable one.

The three sequences, in the order they must run:

| # | What | Channel | Source in Main_MiSTer |
|---|------|---------|-----------------------|
| 1 | ADV7513 bulk configuration (92 register writes: 51 + 13 audio + 28 CSC) | I2C, chip address `0x39` | `video.cpp:1462-1615` `hdmi_config_init()`, then `hdmi_config_audio()`, `hdmi_config_set_csc()` (`:1180`) |
| 2 | Video PLL block + timings (UIO_SET_VIDEO, 26 words), then the ADV7513's three mode registers | fabric mailbox, then I2C | `video.cpp:2266-2290` `set_video()`, `video.cpp:262-318` `setPLL()`, `video.cpp:1691-1727` `hdmi_config_set_mode()` |
| 3 | Frame reader enable (UIO_SET_FBUF, 10 words) + kernel mode knob | fabric mailbox + sysfs | `video.cpp:3474-3530` `video_fb_enable()`, `video.cpp:3459-3471` `fb_write_module_params()` |

After 1 and 2 the screen shows **the core's own output** (the menu core draws a
fabric-generated pattern with no HPS help). After 3 the screen shows
`/dev/fb0`, and because our kernel has `CONFIG_FRAMEBUFFER_CONSOLE=y`, text
written to `/dev/tty1` is painted by fbcon. That is the same mechanism stock
MiSTer uses to show `update_all` on screen.

Main_MiSTer does more after step 2 that we intentionally skip: HDR metadata,
the SPD InfoFrame, CEC, EDID parsing and interrupt arming. None of it is
needed for a picture on a DVI or HDMI monitor. The 3-register mode write is
**not** skipped: Main sends it for the menu core unconditionally and it is
what fixes sync polarity for the preset modes (§3, §4).

## 2. The fabric mailbox ("SPI")

Main_MiSTer calls it SPI; it is a bit-banged handshake over two 32-bit
registers in the Cyclone V **FPGA manager** block, not the lightweight bridge.

| Item | Value | Source |
|------|-------|--------|
| FPGA manager base | `0xFF706000` | `fpga_base_addr_ac5.h:15` `SOCFPGA_MGR_ADDRESS` |
| GPO (HPS to fabric) | base `+0x10`, write-only; Main keeps a shadow copy (`gpo_copy`) because it never reads the register back | `fpga_io.cpp:511-519` |
| GPI (fabric to HPS) | base `+0x14`, read | `fpga_io.cpp:519` |
| Data | GPO `[15:0]` | `fpga_io.cpp:690` |
| Strobe | GPO bit 17 | `fpga_io.cpp:665` `SSPI_STROBE` |
| Ack | GPI bit 17 (same bit position as strobe) | `fpga_io.cpp:666` `SSPI_ACK` |
| Enables (chip selects) | GPO bit 18 `SSPI_FPGA_EN`, bit 19 `SSPI_OSD_EN`, bit 20 `SSPI_IO_EN` | `spi.cpp:5-7` |
| "FPGA not in user mode" | GPI read as a signed int is negative (bit 31 set) | `fpga_io.cpp:700-706` |

One 16-bit word transfer (`fpga_spi()`, `fpga_io.cpp:688-720`):

1. `gpo = (shadow & ~(0xFFFF | STROBE)) | word`; write `gpo`; write `gpo | STROBE`.
2. Poll GPI until ack bit set. If GPI bit 31 is set, the fabric is not
   configured: **abort with a distinct exit code**, never spin.
3. Write `gpo` (strobe low). Poll GPI until ack bit clear.
4. Return GPI `[15:0]`.

A command is: raise `SSPI_IO_EN` (`EnableIO()`, `spi.cpp:45`), send the opcode
word, send the payload words, drop `SSPI_IO_EN` (`DisableIO()`). Main leaves
the enable bits in the shadow between calls; we start from a shadow of zero and
always drop the enable on every exit path, including errors.

Every word transfer returns a 16-bit reply, and one of our two commands reads
it. `set_video()` throws the reply to its opcode away and sends all 26 payload
words regardless (`video.cpp:2266-2294`). `video_fb_enable()` does not: its
payload sits inside `if (res)` on the reply to `UIO_SET_FBUF`
(`video.cpp:3480-3481`), so a core without the HPS frame buffer is sent the
opcode and nothing else (§5). The mailbox therefore offers two senders — one
ungated, one gated — and `UIO_SET_FBUF` uses the gated one.

**Bounded, always.** Every ack poll gets a deadline (10 ms is generous; a
healthy fabric acks in microseconds). A timeout is an error exit, not a hang.
This tool runs inside an installer that must never be able to fail; the tool
itself is allowed to fail, loudly and quickly, and the installer treats every
non-zero exit as "no splash this time".

Opcodes used (`user_io.h`):

| Opcode | Value | Line |
|--------|-------|------|
| `UIO_SET_VIDEO` | `0x20` | `:42` |
| `UIO_LEDS` | `0x25` | `:47` (optional `leds` subcommand only) |
| `UIO_SET_FBUF` | `0x2F` | `:57` |

Access is `mmap` of `/dev/mem` at the FPGA manager page (`O_SYNC`, one 4 KiB
page). Our kernel has `CONFIG_DEVMEM=y` and no `STRICT_DEVMEM`.

## 3. UIO_SET_VIDEO: 26 words

From `set_video()` (`video.cpp:2266-2290`). `item[1..8]` are the timings,
`item[9..20]` the PLL block that `setPLL()` filled in.

```
opcode 0x20
words 1..8   timings:  hact hfp hs hbp vact vfp vs vbp
             word 1 |= (pixel_repeat << 15) | (vrr << 14)     -- both 0 for us
             word 3 |= (hpol << 15)                          -- hsync polarity
             word 7 |= (vpol << 15)                          -- vsync polarity
words 9..20  PLL block, sent as:
             odd  i (9,11,...,19): one word  item[i] | 0x4000
             even i (10,...,20):   two words item[i] & 0xFFFF, item[i] >> 16
```

The `0x8000` bit Main sometimes ORs into word 9 is gated on
`Fpix && cfg.vsync_adjust == 2 && !is_menu()`. We are the menu case: **never
set it.** Note the even entries contribute two words each, so the burst is
8 + 6 + 12 = 26 payload words.

PLL block (`setPLL()`, `video.cpp:301-314`), for a target pixel clock `Fout`
MHz from a 50 MHz reference:

```
item[9]  = 4            item[10] = pll_div(M)
item[11] = 3            item[12] = 0x10000
item[13] = 5            item[14] = pll_div(C)
item[15] = 9            item[16] = 2
item[17] = 8            item[18] = 7
item[19] = 7            item[20] = K   (fractional, 32-bit; 1 when K == 0)
```

where `findPLLpar()` (`video.cpp:224`) searches for integer `C`, `M` and
fractional `K` with `Fvco = 50 * (M + K)` in the PLL's legal range and
`Fpix = Fvco / C`, falling back to the loop at `video.cpp:273-291`, and
`getPLLdiv()` (`video.cpp:218`) packs a divider into the high/low-count form
the reconfig block wants. Transcribe all three functions exactly; they are
pure arithmetic and get golden-vector tests.

**One deviation, and it is a refusal rather than a different number.**
`while ((Fout*c) < 400) c++;` (`video.cpp:227` and again at `:275`) has no
exit for an `Fout` whose product can never reach 400 MHz: `c` runs past
`UINT32_MAX`, wraps, and the loop spins for ever. Upstream is safe without a
guard because every call site hands it a `vmodes[]` row's `Fpix`; ours is a
public function, and §2's rule is that a search which cannot finish is an
error and never a hang. So the solver bounds `C` at 510 — the largest divider
`getPLLdiv()`'s two 8-bit half-counts can carry, `400 / 510` being 0.78 MHz
and no video mode — and reports "no solution" for anything below that, for a
non-finite `Fout` and for zero or negative. Every clock the C solves is
solved bit for bit identically; that is what the golden vectors check.

**Sync polarity of preset modes is zero on the wire.** `vmodes[]` rows carry
timings, `Fpix`, VIC and a pixel-repeat flag only; `hpol`/`vpol` are set
solely by custom-mode parsing (`video.cpp:2338-2341`, `:2378-2379`), so for
a preset they stay at their zero initialisation and bits 15 of words 3 and 7
are 0. The polarity the monitor sees is then corrected by the ADV7513's
`0x17` sync-invert bits (§4). Reproduce that exactly; do not "fix" it by
setting the bits on the wire.

Default mode: **1280x720@60**, `Fpix = 74.25`, timings
`1280 110 40 220 720 5 5 20`, VIC 4 (entry 0 of `vmodes[]`,
`video.cpp:127`). Also carry entry 6, 640x480@60 (`Fpix = 25.175`,
`640 16 96 48 480 10 2 33`, VIC 1, `video.cpp:133`), selectable with
`--mode 480p` for sinks that dislike 720p. No other modes in v1.

## 4. ADV7513 over I2C

- **Bus discovery.** Main opens `0x39` on `/dev/i2c-0`, `-1`, `-2` in turn
  (`i2c_open()` with bus `-1`, `video.cpp:1470`). We do the same, using an
  SMBus receive-byte as the presence probe, and refuse with a distinct exit
  code if more than one bus answers (a fourth adapter in the DT would make
  the wrong bus win silently). `CONFIG_I2C_CHARDEV=y` in our kernel.
- **Transport.** `ioctl(I2C_SLAVE, 0x39)` then SMBus *write byte data* per
  register (`i2c_smbus_write_byte_data`, `video.cpp:1610`). One NAK is logged
  and the table continues, as Main does.
- **Tables.** Transcribe verbatim, in order, as `(reg, value)` byte pairs:
  `init_data[]` in `hdmi_config_init()` (`video.cpp:1499-1607`, the table
  whose first row is `0x98, 0x03` and last row `0xFA, 0x7D`), then the audio
  table from `hdmi_config_audio()` (`video.cpp:1417`), then the CSC table from
  `hdmi_config_set_csc()` (`video.cpp:1180`), evaluated for the defaults Main
  uses when `MiSTer.ini` is absent (no HDR, no ypbpr, no direct video, RGB
  full range, 8-bit 444). Record the evaluated defaults in a comment next to
  each row that depends on them so a reviewer can re-derive it.
- **Mode registers, after SET_VIDEO** (`hdmi_config_set_mode()`,
  `video.cpp:1691-1727`), three writes:
  `0x17 = 0b00000010 | sync_invert` where `sync_invert = (1<<5)` when
  `hpol == 0` and `(1<<6)` when `vpol == 0`, so for our presets `0x17 = 0x62`;
  `0x3B = 0b01000000` (manual pixel repetition) for a mode with `pr == 0`,
  which both presets are, and `0b01001000` (2x clock) for one with `pr != 0`
  (`video.cpp:1699`); the third arm, `0` for direct video in the menu, cannot
  arise here because `cfg.direct_video` defaults to 0;
  `0x3C = VIC` (4 for 720p, 1 for 480p). Main also caches the last three
  values and skips the writes when none changed (`video.cpp:1706`); we set the
  mode once per run, so we always write.
- **Power.** `0x41 = 0x10` (power up) is inside the bulk table; `0x41 = 0x50`
  powers down. `hdmi --off` writes only that.
- **Not done:** EDID read (`0x3F` map), SPD InfoFrame (`0x38`), CEC (`0x3C`
  map), HDR, interrupt arming. None is needed for a picture on a DVI or HDMI
  monitor. Interrupt arming is the one place this shows up as a byte on the
  wire: `0x94` is the INT1 enable mask, and Main writes
  `hdmi_has_int() ? 0xC0 : 0x00` there (`video.cpp:1465`, `:1568`), asking the
  fabric whether the core routes the interrupt pin. We ask nothing and write
  `0x94 = 0x00` unconditionally. That is the only byte of the 92 that differs
  from Main on a core with the pin; if HPD ever gets serviced, this row and
  the mailbox query have to land together.

## 5. UIO_SET_FBUF: 10 words, plus the kernel knob

From `video_fb_enable()` (`video.cpp:3474-3543`) with `n = 0` and no direct
video:

```
opcode 0x2F
word 1   FB_EN | FB_FMT_RxB | FB_FMT_8888   = 0x8000 | 0b10000 | 0b00110 = 0x8016
word 2   fb_addr & 0xFFFF
word 3   fb_addr >> 16
word 4   width
word 5   height
word 6   0                     scaled left
word 7   hact - 1              scaled right   (item[1] of the current mode)
word 8   0                     scaled top
word 9   vact - 1              scaled bottom  (item[5] of the current mode)
word 10  width * 4             stride, bytes
```

with `fb_addr = FB_ADDR + 4096` for buffer 0, `FB_ADDR = 0x22000000`
(`video.cpp:37`, "512 MB + 32 MB"); the 4 KiB skip is the driver's header
page. Width and height are the mode's active area (1280x720). Disable is the
opcode followed by a single `0` word.

**The payload is gated on the reply to the opcode.** The command is
`int res = spi_uio_cmd_cont(UIO_SET_FBUF);` (`video.cpp:3480`) and then
`if (res)` (`:3481`). The ten words above (`:3502-3511`) and the single `0` of
the disable path (`:3527`) are both inside that `if`; a core that answers `0`
gets "Core doesn't support HPS frame buffer" (`:3535`) and no payload at all.
`DisableIO()` (`:3539`) runs either way. Reproduce that: send the opcode, read
the reply, and send the payload only when it is non-zero. A core that answers
`0` has no HPS frame buffer to switch to, so `fb enable` reports that instead
of pushing ten words at a core that is not listening. `UIO_SET_VIDEO` has no
such gate (§2).

Then write `8888 1 <width> <height> <width*4>` to
`/sys/module/MiSTer_fb/parameters/mode` (`fb_write_module_params()`,
`video.cpp:3459-3471`), which makes the kernel driver re-register `/dev/fb0`
at that geometry so fbcon follows. Do the sysfs write **after** the fabric
command, as Main does.

Pixel format on the Linux side is XRGB8888 with the RxB bit set, i.e. what the
kernel driver calls `8888` with `rb = 1`. The tool never writes pixels itself
in v1; fbcon does.

## 6. The crate

One Cargo workspace member, a binary crate with a library target so the pure
parts are unit-tested on the host.

```
src/
  main.rs        argument parsing (hand-rolled; no clap), exit codes, logging
  mailbox.rs     /dev/mem mapping, spi_w, enable/disable, bounded polling
  video.rs       Modeline, vmodes subset, PLL solver, SET_VIDEO word composer
  adv7513.rs     bus discovery, SMBus writes, the three tables
  fb.rs          SET_FBUF composer, sysfs mode write
  say.rs         write text to /dev/tty1 (v1); direct 8x16 draw is a later task
  hw.rs          the only module that touches /dev/mem, /dev/i2c-*, sysfs, tty
```

Rules:

- **Dependencies: `libc` only.** No `nix`, no `clap`, no `anyhow`. The binary
  ships inside an initramfs and the Buildroot vendoring step gets simpler
  with every crate we do not pull.
- **Pure vs. hardware.** `video.rs`, `fb.rs` and the table halves of
  `adv7513.rs` take and return plain values and have no `unsafe`. Only
  `hw.rs` and `mailbox.rs` do I/O, behind small traits so the composers are
  tested with a recording fake.
- **`#![forbid(unsafe_op_in_unsafe_fn)]`, `unsafe` confined to `hw.rs` and
  `mailbox.rs` with a comment on every block.**
- **Edition 2024, MSRV = the `rustc` Buildroot 2026.08 ships (1.97).**
- **Target.** Whatever toolchain the consuming Buildroot config uses. The
  installer is moving to the shared glibc toolchain
  (`armv7-unknown-linux-gnueabihf`); the crate must also build for
  `armv7-unknown-linux-musleabihf` with `+crt-static`, since that is what
  the installer config on `master` still says today. No `std` features that
  differ between the two.
- **No panics on the hardware paths.** Every error is a typed enum mapped to
  an exit code (§7). `unwrap()` is banned outside tests.

## 7. CLI and exit codes

```
itsalive probe [--json]
itsalive hdmi [--mode 720p|480p] [--off]
itsalive fb enable [--mode 720p|480p] | disable
itsalive say [--clear] <text>...
itsalive up [--mode ...]        = hdmi + fb enable, the installer's one call
itsalive leds <mask>            optional, v1.1: UIO_LEDS 0x25, on-board LEDs
```

| Exit | Meaning | Installer's reaction |
|------|---------|----------------------|
| 0 | done | continue |
| 2 | usage error | continue (bug in the caller) |
| 10 | no bitstream in the fabric (GPI bit 31) | continue silently |
| 11 | mailbox timeout (ack never came) | continue silently |
| 12 | ADV7513 not found on any bus | continue silently |
| 13 | ADV7513 on more than one bus | continue, log |
| 14 | `/dev/mem`, `/dev/i2c-*`, sysfs or tty could not be opened | continue silently |

`probe` prints one line per finding and exits with the first failing code, so
the installer can decide before it tries `up`. `--json` is for the rig log.

Every hardware subcommand is idempotent: running `hdmi` twice is harmless,
and `fb enable` after `hdmi` after `up` is harmless.

## 8. Where it runs

- **Installer initramfs** (Buildroot_MiSTer `board/mister/de10nano/installer-overlay/init`):
  `itsalive up` once after the payload is in RAM, then `itsalive say` per
  step. Every call is `|| true` with a `timeout`, and the splash section's
  existing rule holds: **nothing here may fail an install**.
- **Installed system, daemon stopped:** a rescue shell over serial or SSH
  runs `itsalive up` and has a screen. This is also the development loop
  (see PLAN §3): no installer card needed to iterate.
- **Not while Main_MiSTer runs.** Two writers on the mailbox would corrupt
  each other's transfers. `probe` cannot detect a running daemon from the
  hardware side; the caller is responsible, and the README says so.
