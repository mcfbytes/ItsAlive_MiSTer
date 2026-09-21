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

The four sequences, in the order they must run:

| # | What | Channel | Source in Main_MiSTer |
|---|------|---------|-----------------------|
| 1 | ADV7513 bulk configuration (92 register writes: 51 + 13 audio + 28 CSC) | I2C, chip address `0x39` | `video.cpp:1462-1615` `hdmi_config_init()`, then `hdmi_config_audio()`, `hdmi_config_set_csc()` (`:1180`) |
| 2 | Video PLL block + timings (UIO_SET_VIDEO, 26 words), then the ADV7513's three mode registers | fabric mailbox, then I2C | `video.cpp:2266-2290` `set_video()`, `video.cpp:262-318` `setPLL()`, `video.cpp:1691-1727` `hdmi_config_set_mode()` |
| 3 | **Commit the mode** (UIO_BUT_SW, one payload word) | fabric mailbox | `user_io.cpp:3047-3101` `user_io_send_buttons(1)`, called one line after every `video_set_mode()` (`video.cpp:2732`, `:4331`) |
| 4 | Frame reader enable (UIO_SET_FBUF, 10 words) + kernel mode knob | fabric mailbox + sysfs | `video.cpp:3474-3530` `video_fb_enable()`, `video.cpp:3459-3471` `fb_write_module_params()` |

**Step 3 is not optional and it is not a button-press emulation.** Steps 1 and
2 only *describe* a mode; step 3 is what applies it. The framework clears its
`cfg_set` flag on every `UIO_SET_VIDEO` payload word and raises it again only
on `UIO_BUT_SW`, and it is the rising edge of that flag which (a) writes the
PLL reconfiguration block's start register, so the staged PLL actually takes
effect, and (b) enables the scaler's output. Without it the scaler drives no
sync at all, which a monitor reports as **NO SIGNAL** rather than as a black
picture — and the host side looks perfectly healthy throughout, every word
acked. Main never programs a mode without it.

The payload is the `MiSTer.ini` switch bitmap that `user_io_send_buttons()`
assembles (`user_io.cpp:3055-3070`). At the defaults `cfg_parse()` installs
(`cfg.cpp:594-612`) exactly one bit is set, `CONF_CSYNC` (`user_io.h:144`),
because `cfg.csync = 1` (`cfg.cpp:595`) and every other flag is either memset
to zero or, in `cfg.dvi_mode`'s case, 2 against a test for 1
(`user_io.cpp:3064`). So we send `0x0008`. The bit itself only routes
composite sync on the analog VGA path, so HDMI is indifferent to its value;
we match stock because that is cheaper than justifying a difference.

After 1 to 3 the screen shows **the core's own output** (the menu core draws a
fabric-generated pattern with no HPS help). After 4 the screen shows
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
| `UIO_BUT_SW` | `0x01` | `:13` (the mode commit, §1 step 3) |
| `UIO_SET_VIDEO` | `0x20` | `:42` |
| `UIO_LEDS` | `0x25` | `:47` (optional `leds` subcommand only) |
| `UIO_SET_FBUF` | `0x2F` | `:57` |

**GPO must be zeroed before the first transfer.** `fpga_io_init()`
(`fpga_io.cpp:532-539`) is the `mmap` followed by `fpga_gpo_write(0)` and
nothing else, and it runs before any mailbox traffic. This is not bookkeeping:
the framework releases a latched core reset only on a *two-sample* combination
of `GPO[31:30]` — `2'b00` and then `2'b10` — deliberately, so that one stray
write cannot drive the reset line. Every `EnableIO()` already asserts bit 31
(`fpga_io.cpp:668-672`), so without a preceding zero the fabric never sees the
`2'b00` sample and a reset found latched can never be cleared. The ack FSM is
not gated by that reset, so the failure looks exactly like a healthy mailbox
attached to a dark screen. Asserting reset needs `2'b01`, i.e. bit 30, which
this tool never sets (`fpga_io.cpp:649-652` is the C's only writer of it).

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
- **A bus that is missing and a bus that refuses us are different
  failures.** The C's scan makes every setup failure a `continue`
  (`smbus.cpp:228-239`) because its caller only wants a descriptor; our
  caller's whole output is the exit code, so the errno decides. A bus that
  is not there (`ENOENT`/`ENODEV` from the `open`) or that nothing answers on
  (`ENXIO`, `EREMOTEIO` — adapters differ over which one a NOACK becomes, so
  both count — or `ETIMEDOUT` from the probe) is skipped exactly as the C
  skips it, and if no bus answers that is exit 12. A bus that exists and
  cannot be used — `EACCES` because we are not root, `EBUSY` because a kernel
  driver has claimed `0x39`, an adapter that cannot do the transaction — is
  exit 14, which is what §7 promises for "`/dev/i2c-*` … could not be
  opened"; exit 12 would be a lie the installer is told to pass over
  silently. The scan still probes all three buses and only reports such an
  error when *nothing* answered, so an unusable bus can never hide the one
  the chip is on.
- **Transport.** `ioctl(I2C_SLAVE, 0x39)` then SMBus *write byte data* per
  register (`i2c_smbus_write_byte_data`, `video.cpp:1610`). One NAK is logged
  and the table continues, as Main does — and that holds at **every** i2c
  write site, not just the bulk tables: Main log-and-continues in
  `hdmi_config_set_csc()` (`video.cpp:1399-1403`), for the three mode
  registers (`:1716-1722`) and for `tmds_power()`'s `0x41` (`:2737-2745`)
  alike. So every write goes through `hw::write_table`, which owns the
  policy and counts the refusals; a refused register never fails the run.
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
  fabric over the mailbox (`UIO_HDMI_INT`, `user_io.h:78`). We ask nothing and
  write `0x94 = 0x00` unconditionally.

  Be precise about what that costs, because the obvious reading is too kind:
  the framework's answer to that opcode is a hardwired constant, not a
  per-core wire, so `hdmi_has_int()` returns 1 on every core built on it and
  **Main writes `0xC0` here, always**. This is therefore a deliberate
  divergence from stock, not a case where we happen to agree — the one byte of
  the 92 where we differ, unconditionally. It is inert for a one-shot tool
  because nothing in this crate ever reads the ADV7513's INT pin, and arming
  an interrupt that nobody services would only latch status bits. If HPD ever
  gets serviced, this row and the mailbox query have to land together.

## 5. UIO_SET_FBUF: 10 words, plus the kernel knob

From `video_fb_enable()` (`video.cpp:3474-3543`) with `n = 0` and no direct
video:

```
opcode 0x2F
word 1   FB_EN | FB_FMT_RxB | FB_FMT_8888   = 0x8000 | 0b10000 | 0b00110 = 0x8016
word 2   fb_addr & 0xFFFF
word 3   fb_addr >> 16
word 4   fb_width
word 5   fb_height
word 6   0                     scaled left
word 7   hact - 1              scaled right   (item[1] of the current mode)
word 8   0                     scaled top
word 9   vact - 1              scaled bottom  (item[5] of the current mode)
word 10  fb_width * 4          stride, bytes
```

with `fb_addr = FB_ADDR + 4096` for buffer 0, `FB_ADDR = 0x22000000`
(`video.cpp:37`, "512 MB + 32 MB"); the 4 KiB skip is the driver's header
page. Disable is the opcode followed by a single `0` word.

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

**A core that answers `0` is reported, not failed.** The tool says so on
stderr — Main's own line is "Core doesn't support HPS frame buffer"
(`video.cpp:3535`) — skips the sysfs write, and exits 0, because §7's table
has no code for it and inventing one would change what a non-zero exit means
to the installer. Skipping the sysfs write is the part that matters: a core
with no HPS frame buffer is not scanning `/dev/fb0` out, so re-registering it
at a new geometry would leave a rig log looking green and a monitor black. If
a later phase needs to tell this case apart programmatically, it gets a code
of its own here and in §7 together.

`fb_width`/`fb_height` are the **framebuffer's** size and are not in general the
mode's active area: `video_fb_config()` sets `fb_width = item[1] / fb_scale_x`
and `fb_height = item[5] / fb_scale_y` (`video.cpp:3575-3576`), where `fb_scale`
is 2 when `hact * vact > 1920*1080` and 1 otherwise (`video.cpp:3560-3568`, with
`cfg.fb_size` at 0 — it has no default in `cfg_parse()` and we write no
`MiSTer.ini`), and `fb_scale_y` doubles again for a pixel-repeat mode
(`video.cpp:3573`). For both modes in §3 the divisor is 1 and the two pairs
coincide at 1280x720 and 640x480, which is precisely why the code keeps them
distinguishable rather than collapsing them: words 4, 5 and 10 take
`fb_width`/`fb_height` while words 7 and 9 take `hact`/`vact`.

Then write `8888 1 <fb_width> <fb_height> <fb_width*4>` to
`/sys/module/MiSTer_fb/parameters/mode` (`fb_write_module_params()`,
`video.cpp:3459-3471`), which makes the kernel driver re-register `/dev/fb0`
at that geometry so fbcon follows. Do the sysfs write **after** the fabric
command, as Main does.

**And before any pixels.** The driver's store handler blanks the *whole*
reservation before it re-registers — `memset(p_fbdev->fb_base, 0,
resource_size(p_fbdev->fb_res))` (`MiSTer_fb.c:350`), ahead of the `sscanf`
and `fb_set`, and sized by the resource rather than by the visible geometry.
So the order is burst, then knob, then pixels, and `say` must never run before
`fb enable` has written the knob. Painting first produces a black screen with
every host-side signal healthy — the same symptom as a missing §1 step 3, and
easy to confuse with it.

Pixel format on the Linux side is XRGB8888 with the RxB bit set, i.e. what the
kernel driver calls `8888` with `rb = 1`. The tool never writes pixels itself
in v1; fbcon does.

## 6. The crate

One Cargo workspace member, a binary crate with a library target so the pure
parts are unit-tested on the host.

```
src/
  main.rs        argument parsing (hand-rolled; no clap), exit codes, logging
  mailbox.rs     spi_w, enable/disable, bounded polling, over a two-register trait
  video.rs       Modeline, vmodes subset, PLL solver, SET_VIDEO word composer
  adv7513.rs     the three tables, the chip address, the three mode registers
  fb.rs          SET_FBUF composer, the text of the sysfs mode line
  say.rs         write text to /dev/tty1 (v1); direct 8x16 draw is a later task
  hw.rs          the only module that touches /dev/mem, /dev/i2c-*, sysfs, tty:
                 the mmapped Regs, I2C bus discovery and writes, the two writers
```

The three lines that moved are `mailbox.rs`'s mapping, `adv7513.rs`'s bus
discovery and SMBus writes, and `fb.rs`'s sysfs write: the I/O of all three
lives in `hw.rs`, which is what the last line of the list said all along, and
the modules above it are left pure. The traits `hw.rs` publishes
(`mailbox::Regs`, `hw::I2cBus`, `hw::ModeSink`, `hw::TextSink`) are the seam,
so the CLI is driven end to end by fakes.

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
- **Target: both, and neither is optional.** This tool ships into two
  different Buildroot images that deliberately use two different C libraries,
  so it is built for two triples and CI gates on both.

  | Consumer | Buildroot config | libc | Rust target |
  |---|---|---|---|
  | Installer initramfs (§8), the reason this project exists | `mister_installer_defconfig` | musl, **static** (`BR2_TOOLCHAIN_BUILDROOT_MUSL=y`, `BR2_STATIC_LIBS=y`) | `armv7-unknown-linux-musleabihf` |
  | Installed system, for the rescue case | `mister_de10nano_defconfig` | glibc (Buildroot_MiSTer ADR 0001) | `armv7-unknown-linux-gnueabihf` |

  **The musl build must be fully static**, not merely statically linked in
  name: `BR2_STATIC_LIBS=y` means that initramfs contains no dynamic loader
  and no shared libc, so a binary with an `INTERP` segment cannot start at
  all. `.cargo/config.toml` sets `+crt-static` and `link-self-contained=yes`
  for that target, and CI asserts `statically linked` rather than trusting
  it. Verified: zero `NEEDED` entries, zero `INTERP` segments, and the
  binary runs under `qemu-arm` with no sysroot.

  **Do not delete the musl target as dead weight.** It is easy to conclude
  the project standardised on glibc, because it partly did — the installer
  *kernel* is re-linked against the already-built main glibc toolchain
  rather than bootstrapping a second one (`scripts/mk-sdcard.sh` step 2,
  which is a ~15 minute re-link instead of a ~3 hour from-scratch build).
  That consolidation is real, and it stops at the kernel. The installer
  *rootfs* is a separate, deliberately tiny static-musl cpio and was never
  going to follow: it is embedded into the kernel image, so its size is
  kernel size, and static musl is far smaller than static glibc for the
  same job. Dropping `armv7-unknown-linux-musleabihf` would silently
  produce a binary that cannot execute in the one image this tool was
  written for.

  No `std` features that differ between the two. Two `libc` types *do*
  differ between them and both
  are load-bearing in `hw.rs`: `libc::Ioctl` is `c_ulong` on gnueabihf and
  `c_int` on musleabihf, so ioctl request numbers are written in that type
  rather than a hardcoded one; and `libc::off_t` is 32-bit on gnueabihf but
  64-bit on musleabihf, which `0xFF706000` does not fit in, so the `/dev/mem`
  mapping calls `mmap64` on glibc and `mmap` elsewhere (Main_MiSTer gets the
  same wide call from `-D_FILE_OFFSET_BITS=64`, `Makefile:52`). Neither can be
  settled by reasoning: both targets get built.
- **No panics on the hardware paths.** Every error is a typed enum mapped to
  an exit code (§7). `unwrap()` is banned outside tests, and so are
  `eprintln!` and `println!`: both **panic** if the write fails (a full
  overlay, stderr into a consumer that exited, fd 2 closed), and with
  `panic = "abort"` in the release profile that panic is a SIGABRT whose
  status is not one of the §7 codes. Diagnostics go through `hw::log`, which
  drops the write error instead.

## 7. CLI and exit codes

```
itsalive probe [--json]
itsalive hdmi [--mode 720p|480p] [--off]
itsalive fb enable [--mode 720p|480p] | disable
itsalive say [--clear] <text>...
itsalive up [--mode ...]        = hdmi + fb enable, the installer's one call
itsalive leds <mask>            optional, v1.1: UIO_LEDS 0x25, on-board LEDs
itsalive --help                 the usage text, on stdout, exit 0
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
Its three findings are the bitstream, the ADV7513's bus and the presence of
the sysfs knob; it asks the first with a bare GPI read (`is_fpga_ready(1)`,
`fpga_io.cpp:655-662`) rather than a mailbox transfer, so it writes nothing at
all, can be run twice, and cannot itself report exit 11.

The same GPI read guards `hdmi`, `fb` and `up` before their first write. Main
has no such check — it meets an unconfigured fabric inside `fpga_spi()`
(`fpga_io.cpp:699`) and reboots the board — but §1 is why we need one: with no
bitstream the HPS i2c peripheral is not routed to the chip, so without it the
92 bulk writes would all NAK into the void before the first mailbox word
produced exit 10 anyway.

**Before the first write, and before the i2c bus is opened.** Bus discovery
(§4) probes `0x39` on all three adapters, and on an unconfigured fabric none of
them can answer, so a scan that runs first turns an empty fabric into exit 12 —
"ADV7513 not found on any bus", a hardware-absent diagnosis for a board whose
only problem is an unloaded core — and the *same board* then answers 10 to
`probe` and `fb enable` and 12 to `hdmi` and `up`. The GPI read therefore comes
before the `open`, in the two subcommands that touch both devices.

`hdmi --off` is guarded too, and it is the one place this costs something: a
single `0x41 = 0x50` write now needs `/dev/mem` open as well, and can report
exit 14 where it would otherwise have reported 12. That is the right trade —
an i2c write on an empty fabric reaches nothing either, and unguarded the NAK
is logged and swallowed (§4) and `--off` exits 0 having changed nothing — and
the installer never calls `--off` (§8).

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
