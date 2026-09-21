# ItsAlive_MiSTer

> *"It's alive! IT'S ALIVE!"* — Dr. Frankenstein, on getting a picture out of a
> board that everyone else had given up for dead.

`itsalive` brings up HDMI and the Linux framebuffer on a MiSTer **without
Main_MiSTer running**. Its first job is the Buildroot_MiSTer SD-card installer:
during the one minute in which the card is being reformatted, the board today
shows nothing on HDMI, and a first-time user reaches for the power switch at the
exact moment that bricks the card
([Buildroot_MiSTer#185](https://github.com/mcfbytes/Buildroot_MiSTer/issues/185)).
With `itsalive`, the installer can put "installing, do not power off" on the
screen using nothing but the stock `menu.rbf` that U-Boot has already loaded
into the fabric.

It is also a rescue tool: any shell on a MiSTer with a core loaded can get a
screen back with two commands, no daemon required.

```sh
itsalive probe                  # is there a core? which i2c bus is the ADV7513 on?
itsalive hdmi                   # program the PLL, timings and the ADV7513 — picture appears
itsalive fb enable              # hand /dev/fb0 to the fabric's frame reader
itsalive say "Installing..."    # put words on that screen
itsalive image splash.raw       # or a picture
```

`itsalive up` is `hdmi` followed by `fb enable`, which is the one call an
installer needs.

## Commands

```
itsalive probe [--json]              report what is there, exit with the first failure
itsalive hdmi [--mode MODE] [--off]  configure the ADV7513 and the video PLL
itsalive fb enable [--mode MODE]     point the fabric's frame reader at /dev/fb0
itsalive fb disable                  hand the display back to the core
itsalive say [--clear] TEXT...       write TEXT to /dev/tty1 for fbcon to paint
itsalive image [--size WxH] [--clear] PATH
                                     blit a raw BGRX8888 image onto /dev/fb0
itsalive up [--mode MODE]            hdmi, then fb enable: the installer's one call
```

`MODE` is `720p` (the default) or `480p`. Everything needs root: `/dev/mem`,
`/dev/i2c-*` and `/dev/fb0`.

| Exit | Meaning |
|---|---|
| 0 | done |
| 2 | usage error |
| 10 | no bitstream in the fabric (GPI bit 31) |
| 11 | mailbox timeout |
| 12 | ADV7513 not found on any i2c bus |
| 13 | ADV7513 on more than one i2c bus |
| 14 | `/dev/mem`, `/dev/i2c-*`, sysfs, tty or `/dev/fb0` unopenable |

### Showing a picture

`image` takes **raw BGRX8888** — four bytes per pixel, blue first, no header —
from a file or from `-` for stdin. There is no scaling and no cropping, so the
resize happens on the build host:

```sh
# on the build host
ffmpeg -i splash.png -vf scale=1280:720 -f rawvideo -pix_fmt bgra splash.raw
gzip -9 splash.raw

# on the board
itsalive up
zcat splash.raw.gz | itsalive image -
```

A 1280x720 frame is 3.6 MB raw, which gzips to around 20 KB for flat artwork and
blits in about 0.13 s. `--size WxH` declares the *source's* dimensions and
defaults to the whole framebuffer; a smaller source is centred, with an odd
remainder going right and down. `--clear` zeroes the framebuffer first, which
only matters for a second image — `fb enable` blanks it on its way past.

Two things worth knowing before wiring this into a script:

- **Run `fb enable` first.** `image` reads the geometry back from the kernel
  rather than taking a flag, and before `fb enable` that geometry is the
  driver's `640x480` probe default while the frame reader is still pointed at
  the core's own buffer. A `--size 320x200` blit will then be accepted, exit 0,
  and put nothing on the screen. **`image` exiting 0 does not mean a picture is
  up** — it means the bytes reached `/dev/fb0`.
- **Turn the console cursor off** if you mix `say` and `image`, or fbcon
  repaints a blinking block over the artwork:
  `echo 0 > /sys/class/graphics/fbcon/cursor_blink`. It does not persist across
  a reboot, so set it each boot and never restore it.

## Building

Rust 1.97 or newer; the toolchain is pinned in `rust-toolchain.toml` and the
linkers in `.cargo/config.toml`. No Docker and no `cross`.

```sh
# the installed system (glibc) — needs gcc-arm-linux-gnueabihf
cargo build --release --target armv7-unknown-linux-gnueabihf

# the installer's initramfs (static musl) — needs no cross gcc, rustup ships the linker
cargo build --release --target armv7-unknown-linux-musleabihf
```

Both targets are built and kept on purpose: the installed MiSTer system is
glibc, the installer's initramfs is static musl, and the two disagree on types
this code depends on (`libc::Ioctl` is `c_ulong` on one and `c_int` on the
other; `libc::off_t` is 32-bit on one and 64-bit on the other). CI lints and
builds both. See `docs/ARCHITECTURE.md` §6.

The golden vectors under `tests/golden/` are produced by compiling
Main_MiSTer's own PLL and ADV7513 functions and running them — never by copying
bytes out of the Rust. `tests/golden/gen.sh` and `tests/golden/adv7513-gen.sh`
regenerate them in place, and CI fails on any diff.

## Status

**Working on hardware.** On 2026-09-21 a DE10-Nano with no Main_MiSTer on the
card went from a dark HDMI output to a 1280x720 picture, then console text,
then a full-screen image, driven only by the `menu.rbf` U-Boot had already
loaded — and Main was restored afterwards, reloaded the fabric with a different
core, and ran a game. All eleven of the questions only hardware could answer are
settled; four protocol steps remain unrun and are listed at the end of the log.

Next is Phase 4: packaging this for the Buildroot_MiSTer installer, where the
binary is static musl in an initramfs rather than the glibc build tested so far.

Read, in order:

1. [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — what the hardware needs, where every
   register sequence comes from, and how the crate is cut.
2. [`docs/PLAN.md`](docs/PLAN.md) — phases, the rig verification protocol, the
   questions hardware answered, and the risks.
3. [`docs/testlogs/2026-09-21-rig-first-light.md`](docs/testlogs/2026-09-21-rig-first-light.md)
   — what the board actually did, register by register.
4. [`TASKS.md`](TASKS.md) — the task list the implementation agents work from.

## What it is not

- Not a replacement for Main_MiSTer, not a core loader, not an OSD. It programs
  the PLL, the timings and the ADV7513, commits the mode, and gets out of the
  way.
- Not a general framebuffer driver. The kernel's `MiSTer_fb` stays as it is.
- Not board-agnostic yet. DE10-Nano (Cyclone V SoC) only; the register map is
  the Cyclone V FPGA manager's.
- Not an image decoder. It blits raw pixels; PNG and JPEG stay on the build
  host, where there is a real image library and no reason to ship one here.

## License

GPL-3.0-or-later. The register sequences are transcribed from
[Main_MiSTer](https://github.com/MiSTer-devel/Main_MiSTer) (GPL-3.0), which makes
this a derived work; see [`LICENSE`](LICENSE).
