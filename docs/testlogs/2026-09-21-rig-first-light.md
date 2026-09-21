# Rig first light — 2026-09-21

First run of `itsalive` on real hardware. Every unknown in PLAN §2 is settled
below; §3's protocol was followed except for four steps recorded as **not run**
in §9, which stay open.

The headline: a DE10-Nano booted with no Main_MiSTer on the card went from a
dark HDMI output to a 1280x720 picture, then to console text, then to a
photograph, using nothing but the `menu.rbf` U-Boot had already loaded. Main
was restored afterwards and reloaded the fabric with a different core without
complaint.

## 1. Rig

```
Linux MiSTer 7.2.6 #1 SMP PREEMPT_RT Fri Sep  4 15:16:40 UTC 2026 armv7l GNU/Linux
```

DE10-Nano at `192.168.0.160`. `/` is `/dev/loop0` ext4; `/media/fat` is
`/dev/mmcblk0p1` exfat. `/lib/ld-linux-armhf.so.3` is present, so this is the
**glibc** installed system, not the installer's musl initramfs — see §8.

Binaries under test, both cross-built from `0a36795`:

| Target | Bytes | Used here |
|---|---|---|
| `armv7-unknown-linux-gnueabihf` | 396352 | yes — every run below |
| `armv7-unknown-linux-musleabihf` | 445876 | staged, `--help` only |

Staged to `/media/fat/itsalive-test/`, copied to `/tmp/itsalive` before running,
because `/media/fat` is exfat and mounted `sync,dirsync`.

## 2. Baseline, Main running

Taken before anything was touched, so that every later register read has
something to be compared against.

```
fb knob      : 8888 1 1920 1080 7680
fb0          : 1920,1080
i2c buses    : /dev/i2c-0 /dev/i2c-1 /dev/i2c-2
  bus 0: no      bus 1: ADV7513 ANSWERS (reg00=0x13)      bus 2: no
f2sdram      : 0xFFC25080 = 0x00003FFF
```

ADV7513, Main at 1080p: `0x17=0x62 0x3B=0x40 0x3C=0x10 0x40=0x40 0x41=0x10
0x42=0xf8 0x94=0xc0 0x96=0x20 0xAF=0x06`.

`0x3C=0x10` is VIC 16, 1920x1080p60. `0x40=0x40` is the SPD InfoFrame enable
that §2 unknown 8 is about. Note it for §5.

## 3. Cold boot, no Main

`/media/fat/MiSTer` renamed to `MiSTer.off`, board power-cycled. Reboot was
chosen over `killall MiSTer` deliberately — see §9 on the respawn line that
does not exist.

```
uptime       : 0 min
Main process : none
fb knob      : 8888 1 640 480 2560
GPI 0xFF706014 = 0x5CA623A4     (bit 31 clear: fabric in user mode)
GPO 0xFF706010 = 0x00000000
f2sdram 0xFFC25080 = 0x00003FFF
```

The knob reading `8888 1 640 480 2560` is `MiSTer_fb`'s own probe defaults, and
is exactly the reading PLAN §3 step 6a warned is the one that proves nothing.
Here it is the *expected* value and it is load-bearing in the other direction:
it is the before-picture that makes §6's after-picture mean something.

ADV7513 cold: `0x17=0x00 0x3B=0x80 0x3C=0x00 0x40=0x00 0x41=0x50 0x42=0xf0
0x94=0xc0 0xAF=0x14`. `0x41=0x50` is powered **down** and `0xAF=0x14` is DVI
mode — the chip is in its power-on state, untouched since reset. Nothing in the
boot chain configures it; that is the job this tool exists to do.

## 4. `probe`

```
$ itsalive probe --json
{"findings":[{"name":"bitstream","ok":true,"detail":"fabric in user mode (GPI 0x5CA623A4)","exit_code":0},{"name":"adv7513","ok":true,"detail":"ADV7513 at 0x39 on /dev/i2c-1","exit_code":0},{"name":"fb_mode","ok":true,"detail":"/sys/module/MiSTer_fb/parameters/mode present","exit_code":0}],"exit_code":0}
exit=0

$ itsalive probe
ok   bitstream: fabric in user mode (GPI 0x5CA623A4)
ok   adv7513: ADV7513 at 0x39 on /dev/i2c-1
ok   fb_mode: /sys/module/MiSTer_fb/parameters/mode present
exit=0
```

Bus discovery picked `/dev/i2c-1` unaided from three candidates, and did not
raise `AmbiguousBus` — settling §2 unknown 3 and confirming exit 13 stays
theoretical on this board.

## 5. `hdmi` — first light

```
$ itsalive hdmi
itsalive: adv7513: 92 registers written, 0 refused
itsalive: video: 1280x720 @ 74.25 MHz, vic 4, PLL C=6 M=8 K=0xE8F5C28F
itsalive: adv7513: 3 mode registers written, 0 refused
itsalive: video: committed (0x0001 = 0x0008)
EXIT=0
```

**Observed on the monitor: the menu core's background static.** A picture, from
a board with no Main_MiSTer on it.

`C=6 M=8 K=0xE8F5C28F` is the golden vector the unit tests assert against,
reproduced on silicon — the `0.05f`/`0.95f` double-promotion in `video.rs` is
right, and the one-line proof is that the K word is bit-identical.

`0 refused` on both writes: no NAKs across 92 + 3 registers.

Registers after, against the two baselines:

| Reg | Cold | **Ours** | Main @1080p | |
|---|---|---|---|---|
| `0x17` | `0x00` | **`0x62`** | `0x62` | identical to Main |
| `0x3B` | `0x80` | **`0x40`** | `0x40` | identical to Main |
| `0x3C` | `0x00` | **`0x04`** | `0x10` | VIC 4 = 720p60 vs Main's VIC 16 |
| `0x40` | `0x00` | **`0x00`** | `0x40` | SPD InfoFrame — see below |
| `0x41` | `0x50` | **`0x10`** | `0x10` | powered up |
| `0x42` | `0xf0` | **`0xf8`** | `0xf8` | identical to Main |
| `0x94` | `0xc0` | **`0x00`** | `0xc0` | our deliberate divergence |
| `0xAF` | `0x14` | **`0x06`** | `0x06` | HDMI mode, identical to Main |

Six of eight land on Main's exact value. `0x3C` differs because we asked for a
different mode. The two real divergences are §2 unknowns 7 and 8, both of which
this run answers:

- **`0x94 = 0x00` costs nothing** (unknown 7). The sink locked and stayed
  locked. Nothing reads the INT pin, as predicted. Worth keeping as-is.
- **The sink locks without the SPD InfoFrame** (unknown 8). We sat at
  `0x40 = 0x00` for the whole session — through `hdmi`, `fb enable`, `say` and
  two `image` blits — and the monitor never dropped sync. `0x42` reaching
  `0xf8`, Main's own value, says HPD and monitor-sense are both asserted. The
  faithful read-modify-write sequence stays unimplemented, and now on evidence
  rather than on hope.

GPO afterwards: `0x80000008` — bit 31 (EnableIO) still set from the last
transfer, low bits carrying the committed `CONF_CSYNC`.

## 6. `fb enable`

```
knob BEFORE: 8888 1 640 480 2560
$ itsalive fb enable
itsalive: frame buffer: 1280x720, stride 5120 bytes
EXIT=0
knob AFTER:  8888 1 1280 720 5120
fb0 size:    1280,720
fb0 stride:  5120
res_count:   1
dmesg: MiSTer_fb 22000000.MiSTer_fb: width = 1280, height = 720, format=8888
```

The driver re-probed and registered the new geometry: `640 480` → `1280 720`,
stride `2560` → `5120`. That dmesg line is the independent confirmation that
`mode_set` did more than return 0 — the distinction PLAN §3 step 6a exists to
make.

**Observed: the static vanished, replaced by clean black** — not garbage. That
settles §2 unknown 2: the fabric's DDR read ports *are* out of reset after a
U-Boot-loaded core, and `0x00003FFF` at `0xFFC25080` (§3) is why. No bridge
check is needed in `fb enable`.

## 7. `say`, `image`, and the cursor

fbcon binding, read before writing anything:

```
/sys/class/vtconsole/vtcon0/name: (S) dummy device         (bind=0)
/sys/class/vtconsole/vtcon1/name: (M) frame buffer device  (bind=1)
```

§2 unknown 1 answered: **fbcon binds `MiSTer_fb` and follows the sysfs mode
change with no Main running.** The embedded-font fallback in phase 5 is not
needed.

```
$ itsalive say --clear "ITS ALIVE"                                  EXIT=0
$ itsalive say "itsalive on a MiSTer, no Main_MiSTer running"       EXIT=0
```

**Observed: both lines painted, white on black.** `/dev/tty1` is usable —
§2 unknown 4 answered.

Then the installer's actual invocation shape, an image piped in over stdin:

```
$ time (zcat rgbtest.raw.gz | itsalive image -)
itsalive: 1280x720 image at (0, 0) in a 1280x720 frame buffer, stride 5120 bytes
real 0m0.132s   user 0m0.087s   sys 0m0.056s
EXIT=0
```

**Observed: RED on the left third, GREEN in the middle, BLUE on the right.**

That is the result this whole test existed for. The source file is BGRX8888 —
blue byte first — and red came out on the left, so `MiSTer_fb.c`'s
`if(rb) swap(red.offset, blue.offset)` means what ARCHITECTURE §5 says it
means. Had the byte order been wrong, this image would have been blue-first and
the error would have been invisible in every unit test we have.

3686400 bytes in 0.132 s is ~27 MB/s through a pipe, a `read`, and a
write-through mapping. §2 unknown 10 answered: `write(2)` reaches the frame
reader, no flush needed.

**The cursor (§2 unknown 9).** A small black box with a blinking `_` sat in the
upper left where the `say` text had ended, on top of the image. Exactly the
`fbcon_cursor_blink` behaviour predicted. One knob removed it:

```
echo 0 > /sys/class/graphics/fbcon/cursor_blink
```

Confirmed gone on the next blit. **`cursor_blink` was back to `1` after the
reboot in §8**, so it is volatile: the installer must set it each boot, and
nothing needs to undo it. Giving up the console entirely — `con2fbmap`,
`KD_GRAPHICS` — is not required, so `say` and `image` can be mixed freely.

Last, the real payload: a 1280x720 splash, gzipped to 19923 bytes.

```
$ zcat splash-1280x720.raw.gz | itsalive image -
itsalive: 1280x720 image at (0, 0) in a 1280x720 frame buffer, stride 5120 bytes
EXIT=0
```

**Observed: the splash rendered correctly, colours as authored, no cursor.**

**§2 unknown 5, for free:** throughout all of the above the I/O board Power LED
was "blinking slowly on/off as usual" — the menu core drives `LED_POWER` with a
PWM sawtooth while `FB` is off, with no Main attached. That also answers the
open question in Buildroot_MiSTer ADR 0020 §7.

## 8. Restoring Main — the step that decides whether this ships

`MiSTer.off` renamed back to `MiSTer`, board rebooted.

```
Main process : 1328 root /media/fat/MiSTer /media/fat/_Console/NES_20260823.rbf
fb knob      : 8888 1 1920 1080 7680
dmesg        : MiSTer_fb 22000000.MiSTer_fb: width = 1920, height = 1080, format=8888
cursor_blink : 1
```

ADV7513 after Main took over: `0x17=0x62 0x3B=0x40 0x3C=0x10 0x41=0x10
0x42=0xf8 0x94=0xc0` — every value back to the §2 baseline, `0x3C` back to
VIC 16 and `0x94` back to `0xc0`.

**Observed: the Main_MiSTer menu came up as usual, and the NES core loaded and
played a game.**

That last part is a stronger result than PLAN §3 step 8 asked for. Main did not
merely restart against our leftover state: it loaded a *different bitstream*
into the fabric we had been driving, re-ran its own PLL reconfiguration, its own
ADV7513 sequence and its own `UIO_SET_FBUF`, and ran a core. No power cycle
beyond the ordinary reboot, no card re-flash, no manual reset. dmesg carries
nothing angry.

`itsalive` leaves no state that Main cannot walk over. This is the finding that
says the tool is safe to hand to an installer that runs before Main exists.

## 9. Not run — still open

Four items from PLAN §3 were skipped. None blocks Phase 4, and each is cheap on
a future rig session:

1. **`--size` centring on hardware** (step 6c, second half). Only the
   full-screen 1280x720 path was blitted. The centring arithmetic and the
   letterboxing are covered by unit tests and by the golden generators, but no
   photograph yet proves a smaller image lands centred with black around it.
   This is the one gap with a real chance of hiding a bug, because a constant
   offset error is invisible when the image exactly fills the frame.
2. **`say` *after* `image`** (step 6d). The order run here was `say` then
   `image`. Text-over-picture is therefore inferred from the cursor artifact —
   which did land on top of the image, so the mechanism is demonstrated — rather
   than observed directly.
3. **`image` before `fb enable` on a fresh boot** (step 6, trailing paragraph).
   The prediction is exit 0 with nothing on screen at `--size 320x200`, which is
   why the installer must not read `image`'s exit code as "a picture is up".
   Untested; the reasoning in ARCHITECTURE §5 stands unverified.
4. **`hdmi --mode 480p` after `fb disable`** (step 7). Only 720p was exercised
   on hardware. 480p's PLL solution and timings are golden-tested against the C,
   but have never driven a monitor.

One correction to the protocol itself. **PLAN §3 step 2 told us to "check
`inittab` for the respawn line and hold it". There is no respawn line.**
`/etc/inittab:61` is:

```
::sysinit:/media/fat/MiSTer &
```

`sysinit`, not `respawn` — Main is started once at boot and backgrounded, and
`killall MiSTer` on the installed system is sufficient and permanent until the
next boot. This session rebooted without the binary instead, which is stricter
and also proves the cold-boot path, but the instruction as written was chasing
something that does not exist. Fixed in §3.

Nothing here was exercised against the **musl** binary beyond `--help`; the
installer's initramfs is still the untested configuration, and that is Phase 4's
first job.
