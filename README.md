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

```
itsalive probe               # is there a core? which i2c bus is the ADV7513 on?
itsalive hdmi                # program the PLL, timings and the ADV7513: picture appears
itsalive fb enable           # hand /dev/fb0 to the fabric's frame reader
itsalive say "Installing"    # put words on that screen
itsalive image splash.raw    # or a picture: raw BGRX8888, centred, no scaling
```

## Status

**Working on hardware.** On 2026-09-21 a DE10-Nano with no Main_MiSTer on the
card went from a dark HDMI output to a 1280x720 picture, then console text,
then a full-screen image, driven only by the `menu.rbf` U-Boot had already
loaded — and Main was restored afterwards, reloaded the fabric with a different
core, and ran a game. All eleven of the questions only hardware could answer
are settled. Four protocol steps remain unrun, listed at the end of that log.

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
  exactly the three things a picture needs and then gets out of the way.
- Not a general framebuffer driver. The kernel's `MiSTer_fb` stays as it is.
- Not board-agnostic yet. DE10-Nano (Cyclone V SoC) only; the register map is
  the Cyclone V FPGA manager's.

## License

GPL-3.0-or-later. The register sequences are transcribed from
[Main_MiSTer](https://github.com/MiSTer-devel/Main_MiSTer) (GPL-3.0), which makes
this a derived work; see [`LICENSE`](LICENSE).
