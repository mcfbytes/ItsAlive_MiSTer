//! The only module that talks to the kernel.
//!
//! Five interfaces, one per thing the tool has to touch:
//!
//! * `/dev/mem` — one 4 KiB page mapped at the FPGA manager, which is the
//!   [`crate::mailbox::Regs`] pair GPO/GPI ([`MemRegs`], `shmem.cpp:18-38`).
//! * `/dev/i2c-N` — the ADV7513 at chip address `0x39`, reached with
//!   `ioctl(I2C_SLAVE)` plus the `I2C_SMBUS` ioctl ([`I2c`], `smbus.cpp:26-95`
//!   and `:212-262`).
//! * `/sys/module/MiSTer_fb/parameters/mode` — the kernel driver's geometry
//!   knob, written ([`ModeSink`]) and read back ([`ModeSource`]) through the
//!   same [`ModeFile`] (`video.cpp:3459-3471`, `MiSTer_fb.c:343-371`).
//! * `/dev/tty1` — where `say` puts its text for fbcon to paint ([`Tty`]).
//! * `/dev/fb0` — where `image` puts its pixels ([`FbDevice`]), with plain
//!   `write(2)` and no `mmap`, because the driver offers `.fb_write =
//!   fb_sys_write` (`MiSTer_fb.c:137`).
//!
//! Everything above this module is pure and unit-tested; this module exists so
//! that the real hardware satisfies the same traits the fakes do. None of the
//! code below can be exercised on a build host, so it is written to be read:
//! every FFI declaration cites both the kernel UAPI header and the C in
//! Main_MiSTer at commit `6cda9cc`, and every `unsafe` block states the
//! invariant that makes it sound.
//!
//! # Three things a reviewer should check first
//!
//! 1. **The accessors are volatile.** GPO and GPI are device registers and the
//!    GPO writes are a strobe sequence whose order and count *are* the
//!    protocol. See [`MemRegs`]'s `gpo_write` below.
//! 2. **The ioctl request type differs between our two targets.** `libc`'s
//!    `ioctl` takes a [`libc::Ioctl`], which is `c_ulong` on `*-gnueabihf`
//!    (libc 0.2.189 `unix/linux_like/linux/gnu/mod.rs:18`) and `c_int` on
//!    `*-musleabihf` (`.../musl/mod.rs:51`). The request constants are written
//!    as `libc::Ioctl` so both targets compile.
//! 3. **`mmap`'s offset type differs too, and `0xFF706000` does not fit in the
//!    smaller one.** See the two `map_page` arms in the `linux` module below.
//!
//! # What is gated, and what is not
//!
//! [`MemRegs`] and [`I2c`] are `cfg(target_os = "linux")`, with stand-ins on
//! any other OS whose constructors fail with [`std::io::ErrorKind::Unsupported`]
//! (`/dev/mem`, i2c-dev, sysfs module parameters and `/dev/tty1` are Linux
//! interfaces; there is nothing to fall back *to*). Both of the targets this
//! crate ships to are Linux, so the gate never removes anything on a real
//! target — it only keeps `cargo check` honest somewhere else. The sysfs and
//! tty writers are plain `std::fs` and are not gated at all.

use crate::{Error, Result};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Traits: what the CLI is generic over
// ---------------------------------------------------------------------------

/// A register-mapped I2C device that takes one byte at a time.
///
/// The composer half of [`crate::adv7513`] is a list of `(reg, value)` pairs;
/// this is the only thing it needs from the world. T2.2 drives the whole `up`
/// sequence through this trait with a recording fake and asserts the ordered
/// write list.
pub trait I2cBus {
    /// SMBus *write byte data*: `reg <- value`.
    ///
    /// `i2c_smbus_write_byte_data()` (`smbus.cpp:109-115`), the call
    /// `hdmi_config_init()` makes for every row of its table
    /// (`video.cpp:1608`).
    fn write_reg(&mut self, reg: u8, value: u8) -> Result<()>;
}

/// The kernel's framebuffer-geometry knob.
///
/// One line of text, written whole; see [`crate::fb::mode_param_line`] for the
/// line and `video.cpp:3459-3471` for the `fopen`/`fprintf` it replaces.
pub trait ModeSink {
    /// Write one mode line, newline included, replacing whatever was there.
    fn write_mode_line(&mut self, line: &str) -> Result<()>;
}

/// Somewhere to put text a human is meant to read — `/dev/tty1` in practice.
pub trait TextSink {
    /// Write every byte of `bytes`, or fail.
    fn write_text(&mut self, bytes: &[u8]) -> Result<()>;
}

/// The other direction of the same knob: what the driver says it registered.
///
/// A separate trait from [`ModeSink`] because it has a separate reason to
/// exist. Writing the line is a *request* — `mode_set()` returns 0
/// unconditionally and its body is inside `if(p_fbdev)`
/// (`MiSTer_fb.c:343-361`), so a successful write is no evidence at all on a
/// kernel where `MiSTer_fb` never probed. Reading it back through `mode_get()`
/// (`MiSTer_fb.c:364-371`) is the evidence, and it is what `itsalive image`
/// gets its geometry from rather than trusting a flag from the caller.
///
/// [`ModeFile`] implements both, because both are the same file.
pub trait ModeSource {
    /// Read the whole knob, as the kernel rendered it.
    ///
    /// The bytes are handed on unparsed: [`crate::fb::parse_mode_line`] owns
    /// the grammar, and an empty read is a legitimate answer (`mode_get`
    /// returns 0 bytes when the driver never probed) rather than an error
    /// here.
    fn read_mode_line(&mut self) -> Result<String>;
}

/// Somewhere to put pixels — `/dev/fb0` in practice.
///
/// Byte offsets rather than rows or rectangles, because that is all the
/// driver offers: `MiSTer_fb`'s `fb_ops` has `.fb_write = fb_sys_write`
/// (`MiSTer_fb.c:137`), which is an ordinary `write(2)` honouring the file
/// offset. There is no ioctl to blit with and, for this crate, no `mmap`
/// either — so this trait is a seek and a write, and **nothing in the image
/// path needs `unsafe`**.
pub trait PixelSink {
    /// Write every byte of `bytes` starting at byte `offset`, or fail.
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<()>;

    /// Zero the first `len` bytes.
    ///
    /// Provided rather than required so that the real device and every fake
    /// clear the same way, in the same order, through the same `write_at` a
    /// test is already watching.
    ///
    /// `--clear` matters less than it looks: `itsalive fb enable`'s sysfs
    /// write already blanks the whole reservation on its way past
    /// (`memset(p_fbdev->fb_base, 0, resource_size(p_fbdev->fb_res))`,
    /// `MiSTer_fb.c:350`, ahead of the `sscanf`), so it is a *second* image
    /// that needs this — otherwise the first one shows through around it.
    ///
    /// One page at a time: the buffer is a `const`, so it costs no `.bss` and
    /// a 1280x720 screen is 900 writes of 4 KiB, which against a write-through
    /// mapping is not worth a bigger one.
    fn clear(&mut self, len: u64) -> Result<()> {
        const PAGE: [u8; 4096] = [0; 4096];
        let mut done: u64 = 0;
        while done < len {
            // `try_from` cannot be wrong here and cannot panic if it is: a
            // remainder that does not fit a `usize` is certainly larger than
            // one page, which is what the fallback says.
            let take = usize::try_from(len.saturating_sub(done))
                .unwrap_or(PAGE.len())
                .min(PAGE.len());
            self.write_at(done, PAGE.get(..take).unwrap_or(&PAGE))?;
            done = done.saturating_add(take as u64);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Saying something on a path that is not allowed to die
// ---------------------------------------------------------------------------

/// Put one line on stderr, and survive a stderr that will not take it.
///
/// **This exists because `eprintln!` panics.** `std`'s `print_to` ends with
/// `panic!("failed printing to stderr: {e}")` when the write fails, which it
/// does for `ENOSPC` (an installer logging to a full overlay or SD card),
/// `EPIPE` (stderr into a consumer that has exited) or `EBADF` (fd 2 closed by
/// the caller). `Cargo.toml` sets `panic = "abort"` for the release profile,
/// so that panic is a `SIGABRT` on the board: not one of the
/// `docs/ARCHITECTURE.md` §7 exit codes, and a direct contradiction of §6's
/// "no panics on the hardware paths". Verified on this host with the shipping
/// toolchain: a binary whose body is `eprintln!("hello")` exits 101 under
/// `2>/dev/full` and 0 otherwise.
///
/// Both callers here are paths whose whole purpose is to survive a failure —
/// the NAK log in [`write_table`] and the `munmap` complaint in `MemRegs`'s
/// [`Drop`], which runs at a point where there is nothing left to do about
/// anything. Losing a log line there is the correct trade; aborting is not.
/// It is `pub` because the binary crate has the same problem: the stderr line
/// that accompanies an exit code must not be able to replace it with a
/// SIGABRT. **`println!` panics identically**, so anything T2.2 puts on
/// stdout — `probe`'s findings, `--json` — needs the same treatment there.
pub fn log(args: fmt::Arguments<'_>) {
    log_to(io::stderr().lock(), args);
}

/// [`log`] with the sink named, so a test can hand it one that always fails.
fn log_to(mut sink: impl Write, args: fmt::Arguments<'_>) {
    // The whole point: the error is dropped rather than unwrapped.
    let _ = writeln!(sink, "{args}");
}

// ---------------------------------------------------------------------------
// I2C bus discovery: the pure half
// ---------------------------------------------------------------------------

/// The buses `i2c_open()` scans when it is given no hint.
///
/// `smbus.cpp:221-223` — `bus_first = 0`, `bus_last = 2`, with the comment
/// "only /dev/i2c-0..2 exist".
pub const I2C_BUSES: [u8; 3] = [0, 1, 2];

/// The device node for one bus number.
///
/// `smbus.cpp:226` — `sprintf(str, "/dev/i2c-%d", bus)`.
pub fn bus_path(bus: u8) -> String {
    format!("/dev/i2c-{bus}")
}

/// Run `probe` over [`I2C_BUSES`] and insist that exactly one bus answered.
///
/// **This is a deliberate divergence from Main_MiSTer**, and the reason is in
/// `docs/ARCHITECTURE.md` §4. The C returns the first bus that answers and
/// stops looking (`smbus.cpp:223-262`: every failure is a `continue`, and the
/// first success `return`s the descriptor). That is the right behaviour for a
/// daemon that owns the board. It is the wrong behaviour for a one-shot tool
/// that has to be trustworthy without a screen to check: a fourth adapter in
/// the device tree, or a phantom that ACKs its address on the wrong bus, would
/// silently capture the ADV7513's handle and the failure would look like a
/// dead monitor. So we probe all three buses and then:
///
/// * nothing answered → [`Error::NoAdv7513`] (exit 12),
/// * more than one answered → [`Error::AmbiguousBus`] (exit 13), which carries
///   the bus numbers so the log says which ones,
/// * exactly one answered → that one.
///
/// Probing all three costs two extra `open`/`ioctl`/receive-byte round trips
/// on a board where the first bus is the right one, which is microseconds.
///
/// # "Not a match" and "could not be used" are different answers
///
/// `probe` returns `Ok(None)` for a bus that is simply not a match: there is
/// no `/dev/i2c-N` on this board, or there is and nothing at `0x39` on it
/// acknowledged. That is the C's `continue` (`smbus.cpp:228-239`, where the
/// failed `open`, the failed `ioctl` and the failed presence probe are all
/// `continue`), and it is not an error.
///
/// It returns `Err` for a bus that exists and *could not be used*: `EACCES`
/// because we are not root, `EBUSY` because a kernel driver has claimed
/// `0x39` on that adapter, an `ioctl` the adapter does not implement. Those
/// are [`Error::Io`], exit 14, because `docs/ARCHITECTURE.md` §7 promises
/// that code for "`/dev/i2c-*` … could not be opened" and reserves 12 for
/// "ADV7513 not found on any bus". Collapsing the first into the second —
/// which is what discarding the errno here used to do — tells the installer
/// log that the board has no HDMI transmitter when what it has is a
/// permissions problem, and exit 12 is a code the installer is told to pass
/// over silently. The C can afford the conflation because its caller only
/// wants a descriptor (`video.cpp:1470-1474` prints "ADV7513 not found" and
/// carries on); our caller's whole output is the exit code.
///
/// **An unusable bus never hides a usable one.** The scan always runs to the
/// end, and a remembered error is reported only when *nothing* answered, so a
/// driver sitting on `0x39` on `/dev/i2c-0` cannot stop us finding the chip on
/// `/dev/i2c-2`. That keeps the C's tolerance where it buys something and
/// spends it nowhere else.
pub fn discover_bus<T>(mut probe: impl FnMut(u8) -> Result<Option<T>>) -> Result<T> {
    let mut found: Vec<(u8, T)> = Vec::new();
    let mut unusable: Option<Error> = None;
    for bus in I2C_BUSES {
        match probe(bus) {
            Ok(Some(handle)) => found.push((bus, handle)),
            Ok(None) => {}
            Err(e) => {
                // Keep the first: a later bus can only add noise, and the
                // message already names the bus it came from.
                if unusable.is_none() {
                    unusable = Some(e);
                }
            }
        }
    }

    if found.len() > 1 {
        // The handles of the losers drop here, which closes them.
        return Err(Error::AmbiguousBus(
            found.iter().map(|(bus, _)| *bus).collect(),
        ));
    }
    match found.pop() {
        Some((_, handle)) => Ok(handle),
        // Nothing answered. If a bus was there and refused us, say *that*.
        None => match unusable {
            Some(e) => Err(e),
            None => Err(Error::NoAdv7513),
        },
    }
}

// ---------------------------------------------------------------------------
// Table writing: the NAK policy, in one place
// ---------------------------------------------------------------------------

/// Write a table of `(reg, value)` pairs, logging failures and continuing.
///
/// **This function is where the tolerate-a-failed-write policy lives.** The
/// C's bulk loop is
///
/// ```text
/// for (uint i = 0; i < sizeof(init_data); i += 2)
/// {
///     int res = i2c_smbus_write_byte_data(hdmi_main_fd, init_data[i], init_data[i + 1]);
///     if (res < 0) printf("i2c: write error (%02X %02X): %d\n", init_data[i], init_data[i + 1], res);
/// }
/// ```
///
/// (`video.cpp:1606-1611`, and the same three lines again at `:1455-1459` for
/// the audio table.) It prints and keeps going; nothing aborts the table and
/// nothing is retried.
///
/// **The policy is not limited to the two bulk tables, and neither is this
/// function.** Main log-and-continues at *every* ADV7513 write site:
/// `hdmi_config_set_csc()` (`video.cpp:1399-1403`), the three mode registers
/// `0x17`/`0x3B`/`0x3C` in `hdmi_config_set_mode()` (`video.cpp:1716-1722`),
/// and the `0x41 = 0x10`/`0x50` of `tmds_power()` (`video.cpp:2737-2745`),
/// which is the write `docs/ARCHITECTURE.md` §4 assigns to `hdmi --off`. So
/// **every** i2c write this tool makes goes through here, not only the long
/// ones — `write_table(bus, adv7513::mode_regs(hpol, vpol, vic, pr))` and
/// `write_table(bus, [adv7513::POWER_DOWN])` are the intended calls, and both
/// compile as they stand because the argument is any
/// `IntoIterator<Item = (u8, u8)>`. Reaching past this function to
/// [`I2cBus::write_reg`] and applying `?` to it would turn a single NAK that
/// Main survives into exit 14 for the whole run; the tests below pin the mode
/// and power registers to this path so that drift shows up as a red test and
/// not as a dark screen.
///
/// **A NAK and an ioctl-level failure are tolerated alike, and that is the C's
/// behaviour, not an accident of ours.** `i2c_smbus_access()` ends with
/// `if (err == -1) err = -errno;` (`smbus.cpp:73`), so *every* way the ioctl
/// can fail arrives at that `if (res < 0)` as the same kind of negative
/// number: a chip that does not acknowledge (`-ENXIO`, or `-EREMOTEIO` from
/// some adapters), a descriptor that is not an i2c device (`-EBADF`,
/// `-ENOTTY`), a bad argument (`-EINVAL`). The loop cannot tell them apart and
/// does not try. We follow it, because a partially configured ADV7513 that
/// shows a picture is worth more to an installer than a clean abort — but the
/// log line names the `errno`, so a rig log can tell a NAK from an `EBADF`
/// even though neither stops the table.
///
/// Returns the number of writes that failed, so the caller can say
/// "92 registers, 3 refused" instead of pretending all was well.
pub fn write_table<B, I>(bus: &mut B, writes: I) -> usize
where
    B: I2cBus + ?Sized,
    I: IntoIterator<Item = (u8, u8)>,
{
    let mut failed: usize = 0;
    for (reg, value) in writes {
        if let Err(e) = bus.write_reg(reg, value) {
            // `video.cpp:1609`'s message, with the OS error where the C puts
            // the negative errno.
            log(format_args!(
                "i2c: write error ({reg:02X} {value:02X}): {e}"
            ));
            failed = failed.saturating_add(1);
        }
    }
    failed
}

// ---------------------------------------------------------------------------
// sysfs: the MiSTer_fb geometry knob
// ---------------------------------------------------------------------------

/// The kernel driver's mode parameter (`video.cpp:3465`).
pub const FB_MODE_PATH: &str = "/sys/module/MiSTer_fb/parameters/mode";

/// A [`ModeSink`] over a sysfs module parameter, [`FB_MODE_PATH`] by default.
///
/// The path is held rather than an open descriptor because the C opens the
/// file per write — `fopen(..., "wt")`, one `fprintf`, `fclose`
/// (`video.cpp:3465-3469`) — and because leaving a truncating handle open on a
/// sysfs attribute for the life of the process buys nothing.
#[derive(Debug, Clone)]
pub struct ModeFile {
    path: PathBuf,
}

impl ModeFile {
    /// The real knob, [`FB_MODE_PATH`].
    pub fn new() -> Self {
        Self::at(FB_MODE_PATH)
    }

    /// The same writer pointed somewhere else, which is how it is tested.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The path this writer writes to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Default for ModeFile {
    fn default() -> Self {
        Self::new()
    }
}

impl ModeSink for ModeFile {
    /// `fb_write_module_params()` (`video.cpp:3459-3471`).
    ///
    /// `fopen(path, "wt")` (`video.cpp:3465`) is `O_WRONLY | O_CREAT |
    /// O_TRUNC` — the `t` is a no-op on Linux — which is what
    /// [`File::create`] opens; the single `fprintf` of the whole line is the
    /// one `write(2)` that a sysfs attribute's store method expects, so the
    /// line goes out in one call and is never split across two writes by us. (`write_all` would loop on a
    /// short write; a sysfs attribute either takes the whole buffer or fails,
    /// so the loop can only ever run once here, and it also swallows `EINTR`
    /// the way the C's stdio does.)
    fn write_mode_line(&mut self, line: &str) -> Result<()> {
        let mut file = File::create(&self.path)
            .map_err(|e| Error::io(format!("open {}", self.path.display()), e))?;
        file.write_all(line.as_bytes())
            .map_err(|e| Error::io(format!("write {}", self.path.display()), e))
    }
}

impl ModeSource for ModeFile {
    /// `mode_get()` through sysfs (`MiSTer_fb.c:364-371`).
    ///
    /// One `read(2)` of the whole attribute, which is what a sysfs show
    /// method produces: the kernel renders `.get`'s output into a page buffer,
    /// appends a newline, and serves it. A knob whose driver never probed
    /// returns 0 bytes and is read here as the empty string — a legitimate
    /// answer that [`crate::fb::parse_mode_line`] turns into "run `itsalive fb
    /// enable` first", not an I/O error.
    ///
    /// The bytes are not guaranteed to be UTF-8 by anything but the driver's
    /// own `sprintf` of five `%u`s, so a lossy conversion rather than a
    /// failure: a knob that somehow held other bytes should be reported as
    /// unparseable geometry (exit 2), not as a read error (exit 14).
    fn read_mode_line(&mut self) -> Result<String> {
        let bytes = std::fs::read(&self.path)
            .map_err(|e| Error::io(format!("read {}", self.path.display()), e))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
}

// ---------------------------------------------------------------------------
// tty: where `say` puts its text
// ---------------------------------------------------------------------------

/// The console `say` writes to. fbcon paints it (`docs/ARCHITECTURE.md` §1).
pub const TTY_PATH: &str = "/dev/tty1";

/// A [`TextSink`] over a tty, [`TTY_PATH`] by default.
#[derive(Debug)]
pub struct Tty {
    file: File,
    path: PathBuf,
}

impl Tty {
    /// Open [`TTY_PATH`] for writing.
    pub fn open() -> Result<Self> {
        Self::at(TTY_PATH)
    }

    /// The same writer pointed somewhere else, which is how it is tested.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut opts = OpenOptions::new();
        opts.write(true);

        // `O_NOCTTY`: opening a terminal that has no controlling process makes
        // it *our* controlling terminal if we are a session leader. The
        // installer calls this from its `init` (PLAN §4, ARCHITECTURE §8), so
        // that is not hypothetical, and acquiring a controlling terminal would
        // hand this process the console's signals. We only want to write to
        // it. (std sets `O_CLOEXEC` itself on every file it opens.)
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOCTTY);
        }

        let file = opts
            .open(&path)
            .map_err(|e| Error::io(format!("open {}", path.display()), e))?;
        Ok(Self { file, path })
    }

    /// The path this writer writes to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl TextSink for Tty {
    /// Plain `write(2)`, looping.
    ///
    /// [`Write::write_all`] is that loop: it calls `write(2)`, advances by the
    /// byte count it got back so a short write finishes on the next pass, and
    /// retries on [`std::io::ErrorKind::Interrupted`] — which is `EINTR`, and
    /// is reachable here because a tty write can block on flow control. A
    /// [`File`] is unbuffered, so there is no flush to forget.
    fn write_text(&mut self, bytes: &[u8]) -> Result<()> {
        self.file
            .write_all(bytes)
            .map_err(|e| Error::io(format!("write {}", self.path.display()), e))
    }
}

// ---------------------------------------------------------------------------
// /dev/fb0: where `image` puts its pixels
// ---------------------------------------------------------------------------

/// The framebuffer `MiSTer_fb` registers, and the fabric's frame reader scans.
pub const FB_DEV_PATH: &str = "/dev/fb0";

/// A [`PixelSink`] over a framebuffer device, [`FB_DEV_PATH`] by default.
///
/// **Plain `write(2)`, no `mmap`, and therefore no `unsafe` anywhere in the
/// image path.** The driver's `fb_ops` sets `.fb_write = fb_sys_write`
/// (`MiSTer_fb.c:137`), the generic writer that copies from userspace into
/// `info->screen_base` at `*ppos` and clamps the count against
/// `info->fix.smem_len`, so a [`Seek`] plus a [`Write`] is the whole
/// interface. Mapping the device would buy a `memcpy` in place of a syscall
/// per row and would cost this crate its second `unsafe` module for it.
///
/// Nothing needs flushing afterwards either: the driver's mapping is
/// `memremap(..., MEMREMAP_WT)` (`MiSTer_fb.c:261`), write-through, so the
/// bytes are in DDR by the time the write returns and the fabric's frame
/// reader — which does not walk the CPU's caches — sees them on the next
/// scan. A [`File`] is unbuffered in userspace as well, so there is no
/// `flush` to forget.
#[derive(Debug)]
pub struct FbDevice {
    file: File,
    path: PathBuf,
}

impl FbDevice {
    /// Open [`FB_DEV_PATH`] for writing.
    pub fn open() -> Result<Self> {
        Self::at(FB_DEV_PATH)
    }

    /// The same writer pointed somewhere else, which is how it is tested.
    ///
    /// `write(true)` and nothing else: no `create`, because a missing
    /// `/dev/fb0` is a fact about the board and inventing a regular file in
    /// its place would turn exit 14 into a silent success that paints
    /// nothing; and no `truncate`, which on a character device would be
    /// meaningless and on the temporary file a test points this at would
    /// throw away the very bytes the test is checking the offsets against.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(|e| Error::io(format!("open {}", path.display()), e))?;
        Ok(Self { file, path })
    }

    /// The path this writer writes to.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl PixelSink for FbDevice {
    /// `lseek(2)` then `write(2)`, looping.
    ///
    /// [`Write::write_all`] is the loop: it advances by the count each
    /// `write(2)` returns, so a short write finishes on the next pass, and it
    /// retries [`std::io::ErrorKind::Interrupted`] (`EINTR`). A framebuffer
    /// device will not normally short-write, but `fb_sys_write` clamps the
    /// count against `smem_len` and returns what it took, which is exactly a
    /// short write — and the loop then gets `ENOSPC` on the next pass rather
    /// than silently dropping the tail of a row.
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<()> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(format!("seek {} to {offset}", self.path.display()), e))?;
        self.file
            .write_all(bytes)
            .map_err(|e| Error::io(format!("write {}", self.path.display()), e))
    }
}

/// Read the whole of an image source: a file, or standard input for `-`.
///
/// `-` is what makes `zcat splash.raw.gz | itsalive image -` work in the
/// installer, where the payload is compressed and there is no room on the
/// initramfs to land 3.5 MB of raw pixels first.
///
/// The whole source is read before any of it is written, on purpose: the
/// length check in [`crate::fb::BlitPlan::check_source_len`] is the only thing
/// that catches a converter invoked with the wrong pixel format, and it cannot
/// run on a stream that is already half painted on the screen.
pub fn read_source(path: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if path == "-" {
        io::stdin()
            .lock()
            .read_to_end(&mut bytes)
            .map_err(|e| Error::io("read stdin", e))?;
    } else {
        File::open(path)
            .map_err(|e| Error::io(format!("open {path}"), e))?
            .read_to_end(&mut bytes)
            .map_err(|e| Error::io(format!("read {path}"), e))?;
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// The Linux hardware paths
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    use super::{I2cBus, bus_path, discover_bus};
    use crate::mailbox;
    use crate::{Error, Result};
    use libc::{c_int, c_void};
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::mem::offset_of;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::ptr;

    // -----------------------------------------------------------------------
    // /dev/mem
    // -----------------------------------------------------------------------

    /// The physical-memory device (`shmem.cpp:22`).
    pub const DEV_MEM: &str = "/dev/mem";

    /// How much of it we map: one page.
    ///
    /// `docs/ARCHITECTURE.md` §2 — where Main maps a 16 MiB window over the
    /// whole register block (`FPGA_REG_BASE 0xFF000000`, `FPGA_REG_SIZE
    /// 0x01000000`, `fpga_io.cpp:26-27`, mapped at `:534`) because it also
    /// pokes the reset manager and the system manager, we touch exactly two
    /// registers and map exactly the page they are on. `mmap` requires the
    /// offset to be page aligned; [`mailbox::FPGA_MANAGER_BASE`] is
    /// `0xFF706000`, which is aligned to 4 KiB — the page size on ARMv7, the
    /// only architecture this code runs on.
    const MAP_LEN: usize = 4096;

    /// The `Regs` pair, over a mapping of `/dev/mem`.
    ///
    /// This is the real [`mailbox::Regs`]: the same two accessors the
    /// recording fake in `mailbox.rs` implements, pointed at the FPGA
    /// manager's GPO and GPI instead of a `Vec`.
    #[derive(Debug)]
    pub struct MemRegs {
        /// What [`Drop`] hands back to `munmap`.
        base: *mut c_void,
        /// `FPGA_MANAGER_BASE + GPO_OFFSET`, inside the mapping.
        gpo: *mut u32,
        /// `FPGA_MANAGER_BASE + GPI_OFFSET`, inside the mapping.
        gpi: *const u32,
        /// Held only so that dropping `MemRegs` closes `/dev/mem` after the
        /// `munmap`: a struct's fields drop after its own `Drop::drop` has
        /// run. The mapping itself does not need it — POSIX keeps a mapping
        /// valid after its descriptor is closed, which is why the C can leave
        /// `memfd` open for ever without anyone noticing (`shmem.cpp:16`).
        _mem: File,
    }

    impl MemRegs {
        /// Open `/dev/mem` and map the FPGA manager's page.
        ///
        /// `shmem_map()` (`shmem.cpp:18-38`) with our page instead of Main's
        /// 16 MiB window. Every failure is [`Error::Io`], which is exit 14.
        pub fn open() -> Result<Self> {
            // `O_RDWR | O_SYNC | O_CLOEXEC` (`shmem.cpp:22`). std adds
            // `O_CLOEXEC` to everything it opens, so only `O_SYNC` has to be
            // asked for; it is what keeps the mapping uncached, which is not
            // optional for device registers.
            let mem = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_SYNC)
                .open(DEV_MEM)
                .map_err(|e| Error::io(format!("open {DEV_MEM}"), e))?;

            let base = map_page(mem.as_raw_fd(), mailbox::FPGA_MANAGER_BASE);
            // `shmem.cpp:31` — the C tests `res == (void *)-1`, which is
            // `MAP_FAILED`. `mmap` never returns null on failure, so a null
            // check would let a failed mapping through.
            if base == libc::MAP_FAILED {
                return Err(Error::io(
                    format!("mmap {DEV_MEM} at {:#010X}", mailbox::FPGA_MANAGER_BASE),
                    io::Error::last_os_error(),
                ));
            }

            let bytes = base.cast::<u8>();
            // SAFETY: `mmap` returned MAP_LEN = 4096 bytes starting at
            // `base`, and both offsets (0x10 and 0x14) plus four bytes are
            // inside that, so both pointers are in bounds of one allocated
            // object. `base` is page aligned and both offsets are multiples
            // of 4, so both are aligned for `u32`. No reference is ever
            // formed to either address, only volatile accesses through the
            // raw pointers, so the "no other alias" rule that would be
            // violated by a device register changing under us never applies.
            let (gpo, gpi) = unsafe {
                (
                    bytes.add(mailbox::GPO_OFFSET).cast::<u32>(),
                    bytes.add(mailbox::GPI_OFFSET).cast::<u32>().cast_const(),
                )
            };

            Ok(Self {
                base,
                gpo,
                gpi,
                _mem: mem,
            })
        }
    }

    impl mailbox::Regs for MemRegs {
        /// Write GPO.
        ///
        /// **`write_volatile` is the most important call in this module.**
        /// `self.gpo` addresses a device register, not memory. The sequence
        /// `fpga_spi()` performs — word, word-with-strobe, word again
        /// (`fpga_io.cpp:692-707`) — is a protocol in which the *order and
        /// the count* of the writes are the message; two of the three writes
        /// store the same value, and nothing in this program ever reads any
        /// of them back, because GPO is write-only and the shadow copy stands
        /// in for it (`fpga_io.cpp:510-518`). A plain `*p = v` is an ordinary
        /// memory store, and the compiler is entitled to coalesce the
        /// identical ones, sink them past each other, or delete all of them
        /// as dead stores to memory no one reads. `write_volatile` is what
        /// forbids that; without it this module compiles, passes every test
        /// that can run on a host, and silently transfers nothing on the
        /// board.
        fn gpo_write(&mut self, value: u32) {
            // SAFETY: `self.gpo` was derived in `open()` from a live mapping
            // that lives exactly as long as `self`, is 4-byte aligned and in
            // bounds (see the SAFETY note there), and `Drop` is the only
            // thing that invalidates it.
            unsafe { self.gpo.write_volatile(value) }
        }

        /// Read GPI.
        ///
        /// Volatile for the mirror-image reason: the fabric changes this
        /// register behind our back, so the ack poll in
        /// [`mailbox::Mailbox::spi_w`] reads the same address repeatedly and
        /// expects a different answer. A non-volatile read would be hoisted
        /// out of that loop and the poll would spin on a cached value until
        /// its deadline, turning every transfer into [`Error::Timeout`].
        fn gpi_read(&self) -> u32 {
            // SAFETY: as `gpo_write`, and a read needs no exclusivity.
            unsafe { self.gpi.read_volatile() }
        }
    }

    impl Drop for MemRegs {
        fn drop(&mut self) {
            // SAFETY: `self.base` and `MAP_LEN` are exactly the address and
            // length `mmap` returned in `open()`; nothing else unmaps them,
            // `MemRegs` is neither `Copy` nor `Clone` so there is no second
            // owner, and `self.gpo`/`self.gpi` are not touched after this.
            let rc = unsafe { libc::munmap(self.base, MAP_LEN) };
            if rc != 0 {
                // `shmem_unmap()` prints and carries on (`shmem.cpp:42-46`).
                // There is nothing else to do in a destructor, and panicking
                // is banned on a hardware path.
                super::log(format_args!(
                    "itsalive: munmap {DEV_MEM}: {}",
                    io::Error::last_os_error()
                ));
            }
            // `_mem` drops immediately after this, closing `/dev/mem`.
        }
    }

    /// `mmap` one [`MAP_LEN`] page of `fd` at `offset`, or `MAP_FAILED`.
    ///
    /// **The offset type is the one place the two ARM targets need different
    /// code, and it is not cosmetic.** `libc::off_t` is `i64` on
    /// `armv7-unknown-linux-musleabihf` (libc 0.2.189
    /// `unix/linux_like/linux/musl/mod.rs:33` — musl has one 64-bit `off_t`
    /// and no `mmap64`) but `i32` on `armv7-unknown-linux-gnueabihf`
    /// (`.../gnu/b32/mod.rs:60`, the branch taken when neither
    /// `gnu_file_offset_bits64` nor `gnu_time_bits64` is set, which is the
    /// default). `0xFF706000` does not fit in an `i32`: passed to the plain
    /// `mmap` it wraps to a negative offset, and what glibc then does with it
    /// depends on the glibc — it has at times divided the offset by the page
    /// size as *unsigned* on its way to `mmap2` (which happens to recover the
    /// right page number) and at times widened it to `off64_t` by sign
    /// extension first (which does not, and comes back `EINVAL`). None of
    /// that is a thing to gamble a rig session on, and the gamble is
    /// avoidable: with `mmap64` the offset is 64-bit, `0xFF706000` is an
    /// ordinary positive number, and there is nothing to get wrong.
    ///
    /// Main_MiSTer does not have this problem because it is compiled with
    /// `-D_FILE_OFFSET_BITS=64 -D_LARGEFILE64_SOURCE` (`Makefile:52`), which
    /// makes its `off_t` 64-bit and its `mmap` call `mmap64` — so
    /// `shmem_map()`'s `uint32_t address` (`shmem.cpp:18`) widens to a
    /// positive 64-bit offset. We cannot set a C macro from Rust, so we call
    /// `mmap64` by name on glibc and plain `mmap` everywhere else, which is
    /// the same syscall with the same argument the C ends up making.
    #[cfg(target_env = "gnu")]
    fn map_page(fd: c_int, offset: usize) -> *mut c_void {
        // SAFETY: `mmap` with a null address hint cannot disturb any existing
        // mapping; `fd` is a live descriptor for `/dev/mem` owned by the
        // caller; the result is returned unexamined, and the caller checks it
        // against `MAP_FAILED` before deriving any pointer from it.
        unsafe {
            libc::mmap64(
                ptr::null_mut(),
                MAP_LEN,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                offset as libc::off64_t,
            )
        }
    }

    /// `mmap` one [`MAP_LEN`] page of `fd` at `offset`, or `MAP_FAILED`.
    ///
    /// The non-glibc arm: see the glibc one above for why there are two.
    /// musl's `off_t` is 64-bit on every architecture, so the plain call is
    /// already the wide one.
    #[cfg(not(target_env = "gnu"))]
    fn map_page(fd: c_int, offset: usize) -> *mut c_void {
        // SAFETY: as the glibc arm above.
        unsafe {
            libc::mmap(
                ptr::null_mut(),
                MAP_LEN,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                offset as libc::off_t,
            )
        }
    }

    // -----------------------------------------------------------------------
    // i2c-dev
    // -----------------------------------------------------------------------

    // The i2c-dev interface, hand-declared because `libc` does not carry it.
    // Values cross-checked against this machine's
    // `/usr/include/linux/i2c-dev.h` and `/usr/include/linux/i2c.h`, and
    // against the copies in `smbus.cpp:26-44`, which agree.
    //
    // The request numbers are `libc::Ioctl` so that they are `c_ulong` on
    // gnueabihf and `c_int` on musleabihf without a cast at the call site.

    /// Point this descriptor at a slave address.
    /// `linux/i2c-dev.h:28`, `smbus.cpp:26`.
    const I2C_SLAVE: libc::Ioctl = 0x0703;

    /// Do one SMBus transaction. `linux/i2c-dev.h:38`, `smbus.cpp:27`.
    const I2C_SMBUS: libc::Ioctl = 0x0720;

    /// `linux/i2c.h:150`, `smbus.cpp:29`.
    const I2C_SMBUS_READ: u8 = 1;

    /// `linux/i2c.h:151`, `smbus.cpp:30`.
    const I2C_SMBUS_WRITE: u8 = 0;

    /// Receive/send byte, no register. `linux/i2c.h:156`, `smbus.cpp:34`.
    const I2C_SMBUS_BYTE: u32 = 1;

    /// Register plus one byte. `linux/i2c.h:157`, `smbus.cpp:35`.
    const I2C_SMBUS_BYTE_DATA: u32 = 2;

    /// `linux/i2c.h:141`, `smbus.cpp:44`.
    const I2C_SMBUS_BLOCK_MAX: usize = 32;

    /// `union i2c_smbus_data` (`linux/i2c.h:142-146`, `smbus.cpp:46-51`).
    ///
    /// 34 bytes, aligned to 2: the `block` arm is `I2C_SMBUS_BLOCK_MAX + 2`
    /// because element 0 carries the length and one more byte exists for
    /// user-space compatibility (the header says so at `linux/i2c.h:145`).
    #[repr(C)]
    union I2cSmbusData {
        /// The only arm this crate uses: the byte of a *write byte data* and
        /// the byte a *receive byte* brings back.
        byte: u8,
        // `word` and `block` are never touched here, and `dead_code` is
        // wrong about them: their job is the layout. The kernel decides how
        // many bytes of this object to copy from the transaction's `size`
        // (`drivers/i2c/i2c-dev.c`, `i2cdev_ioctl_smbus`), so the object has
        // to be the size the kernel believes it is even for the transactions
        // we never issue.
        #[allow(dead_code)]
        word: u16,
        #[allow(dead_code)]
        block: [u8; I2C_SMBUS_BLOCK_MAX + 2],
    }

    /// `struct i2c_smbus_ioctl_data` (`linux/i2c-dev.h:42-47`,
    /// `smbus.cpp:53-59`), the argument of the `I2C_SMBUS` ioctl.
    ///
    /// The layout is the reason this is written out by hand rather than
    /// packed into a byte array: `read_write` at 0 and `command` at 1 are
    /// followed by two bytes of padding, because `size` is a `__u32` that
    /// must land at offset 4, and `data` follows at offset 8 on both a 32-bit
    /// ARM (pointer alignment 4, so 4 + 4 = 8) and on this 64-bit build host
    /// (pointer alignment 8, so the compiler pads to 8). Getting that wrong
    /// is invisible until the rig, which is why the layout is asserted twice
    /// over: once in `const` below, which every build checks including both
    /// ARM targets, and once in the tests, which only ever see the host's.
    #[repr(C)]
    struct I2cSmbusIoctlData {
        read_write: u8,
        command: u8,
        size: u32,
        data: *mut I2cSmbusData,
    }

    // The two FFI layouts, checked at compile time on whatever is being built.
    // A host unit test cannot prove anything about a 32-bit ARM's padding; a
    // `const` block can, because it is evaluated for the target. If one of
    // these ever fires, the build stops with the offset that moved.
    const _: () = {
        assert!(offset_of!(I2cSmbusIoctlData, read_write) == 0);
        assert!(offset_of!(I2cSmbusIoctlData, command) == 1);
        // 2..4 is padding: `size` is a `__u32` and has to be 4-aligned.
        assert!(offset_of!(I2cSmbusIoctlData, size) == 4);
        assert!(offset_of!(I2cSmbusIoctlData, data) == 8);
        assert!(size_of::<I2cSmbusIoctlData>() == 8 + size_of::<*mut I2cSmbusData>());
        assert!(align_of::<I2cSmbusIoctlData>() == align_of::<*mut I2cSmbusData>());
        assert!(size_of::<I2cSmbusData>() == I2C_SMBUS_BLOCK_MAX + 2);
        assert!(align_of::<I2cSmbusData>() == align_of::<u16>());
    };

    /// What the C compiler makes of the same declaration on the board:
    /// `1 + 1 + 2 padding + 4 + 4`.
    #[cfg(target_pointer_width = "32")]
    const _: () = assert!(size_of::<I2cSmbusIoctlData>() == 12);

    /// Which of `i2c_open()`'s two setup steps failed (`smbus.cpp:226-239`),
    /// so that one message can be built from either caller's rules.
    #[derive(Clone, Copy, Debug)]
    enum OpenStep {
        /// `open("/dev/i2c-N", O_RDWR | O_CLOEXEC)` (`smbus.cpp:228`).
        Open,
        /// `ioctl(fd, I2C_SLAVE, dev_address)` (`smbus.cpp:234`).
        SetSlave,
    }

    /// Does this `open(2)` failure mean "this board has no such i2c bus"?
    ///
    /// `ENOENT` is the ordinary answer for `/dev/i2c-2` on a board with two
    /// adapters, and `ENODEV` is what a device node whose driver is gone
    /// gives. Neither is a failure of ours, so the scan treats them as the
    /// C's `continue` (`smbus.cpp:228-231`). Everything else — `EACCES` (not
    /// root), `EPERM`, `EBUSY`, `ENOMEM` — is a bus that is *there* and that
    /// we could not open, which is exactly the sentence in
    /// `docs/ARCHITECTURE.md` §7 next to exit 14.
    fn bus_is_absent(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ENODEV))
    }

    /// Does this transfer failure mean "nothing at that address answered"?
    ///
    /// A chip that does not acknowledge its address comes back as `ENXIO`
    /// from most adapters and `EREMOTEIO` from others — the DE10-Nano's
    /// controller is the DesignWare one, which reports a `NOACK` abort, and
    /// which of the two errnos that surfaces as has not been read off the rig
    /// yet, so both are accepted — and `ETIMEDOUT` is the answer from an
    /// adapter that gave up on a bus nobody is driving. All three mean "not
    /// here", which is the C's `continue` at `smbus.cpp:250-257`. An `EBADF`,
    /// `ENOTTY` or `EOPNOTSUPP` does not: that is a descriptor or an adapter
    /// that cannot do this at all, and the difference is exit 14 against
    /// exit 12.
    fn no_one_answered(e: &io::Error) -> bool {
        matches!(
            e.raw_os_error(),
            Some(libc::ENXIO | libc::EREMOTEIO | libc::ETIMEDOUT)
        )
    }

    /// The ADV7513 over one `/dev/i2c-N`.
    #[derive(Debug)]
    pub struct I2c {
        file: File,
        bus: u8,
        addr: u8,
    }

    impl I2c {
        /// Open `/dev/i2c-<bus>` and point it at `addr`.
        ///
        /// `smbus.cpp:226-239`: `open(str, O_RDWR | O_CLOEXEC)` — std adds
        /// `O_CLOEXEC` itself — then `ioctl(fd, I2C_SLAVE, dev_address)`.
        /// Unlike [`Self::open_adv7513`] this reports why it failed, because
        /// a caller naming one bus meant that bus.
        pub fn open_bus(bus: u8, addr: u8) -> Result<Self> {
            Self::open_and_select(bus, addr)
                .map_err(|(step, e)| Error::io(Self::step_failed(step, bus, addr), e))
        }

        /// The `open` and the `I2C_SLAVE` ioctl, with the errno and which of
        /// the two produced it, because [`Self::open_bus`] reports every
        /// failure and [`Self::probe`] has to sort them first.
        fn open_and_select(bus: u8, addr: u8) -> std::result::Result<Self, (OpenStep, io::Error)> {
            let path = bus_path(bus);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(|e| (OpenStep::Open, e))?;
            let i2c = Self { file, bus, addr };
            i2c.set_slave().map_err(|e| (OpenStep::SetSlave, e))?;
            Ok(i2c)
        }

        /// What [`Error::Io`]'s `what` says about a failed setup step.
        fn step_failed(step: OpenStep, bus: u8, addr: u8) -> String {
            let path = bus_path(bus);
            match step {
                OpenStep::Open => format!("open {path}"),
                OpenStep::SetSlave => format!("ioctl I2C_SLAVE {addr:#04X} on {path}"),
            }
        }

        /// Find the one bus the ADV7513 answers on and open it.
        ///
        /// The scan is `i2c_open(0x39, 0, -1, &adv_bus)` (`video.cpp:1470`);
        /// the rule applied to its results is ours, and
        /// [`discover_bus`] says why.
        pub fn open_adv7513() -> Result<Self> {
            discover_bus(|bus| Self::probe(bus, crate::adv7513::CHIP_ADDR))
        }

        /// One bus of the scan: a match, a considered "no", or a bus that
        /// exists and cannot be used.
        ///
        /// `smbus.cpp:223-262`. Three things have to work — the `open`, the
        /// `I2C_SLAVE` ioctl and the presence probe — and in the C each
        /// failure is a `continue` that closes the descriptor and moves to
        /// the next bus. Most of them are not errors here either: a board
        /// simply does not have an ADV7513 on every bus, and often does not
        /// have every bus. But the errno says which case this is, and
        /// [`discover_bus`] needs it to choose between exit 12 and exit 14 —
        /// see its doc for why that distinction is worth the two classifier
        /// functions. `Ok(None)` is the C's `continue`; `Err` is a bus that
        /// answered the question "can I use you?" with `EACCES` or `EBUSY`.
        ///
        /// The presence probe is an SMBus **receive byte**. That is what
        /// `i2c_open()` does for a device opened with `is_smbus = 0`
        /// (`smbus.cpp:250-257`), and the ADV7513 is opened that way
        /// (`video.cpp:1470`); the `is_smbus = 1` arm's *write quick*
        /// (`:243-248`) is for real SMBus parts and would put a bare address
        /// with no data on the wire.
        fn probe(bus: u8, addr: u8) -> Result<Option<Self>> {
            let i2c = match Self::open_and_select(bus, addr) {
                Ok(i2c) => i2c,
                // No such `/dev/i2c-N`: nothing to report, nothing wrong.
                Err((OpenStep::Open, ref e)) if bus_is_absent(e) => return Ok(None),
                // Anything else, from either step, is a bus we could not
                // have. `I2C_SLAVE` fails with `EBUSY` when a kernel driver
                // owns the address and `EINVAL` when the address is out of
                // range, which `0x39` is not; neither means "no chip".
                Err((step, e)) => return Err(Error::io(Self::step_failed(step, bus, addr), e)),
            };
            match i2c.receive_byte() {
                Ok(_) => Ok(Some(i2c)),
                Err(ref e) if no_one_answered(e) => Ok(None),
                Err(e) => Err(Error::io(
                    format!("i2c probe {addr:#04X} on {}", bus_path(bus)),
                    e,
                )),
            }
        }

        /// The bus this handle is open on.
        pub fn bus(&self) -> u8 {
            self.bus
        }

        /// The chip address this handle is pointed at.
        pub fn addr(&self) -> u8 {
            self.addr
        }

        /// `ioctl(fd, I2C_SLAVE, addr)` (`smbus.cpp:234`).
        fn set_slave(&self) -> io::Result<()> {
            // SAFETY: `ioctl` is variadic; `I2C_SLAVE` takes its argument by
            // value as an unsigned long (`linux/i2c-dev.h:15-16`), and
            // `c_ulong` is what the C promotes `int dev_address` to on both
            // of our targets. The descriptor is live for the call. No pointer
            // is involved, so there is nothing for the kernel to write.
            let rc = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    I2C_SLAVE,
                    libc::c_ulong::from(self.addr),
                )
            };
            if rc < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// SMBus *receive byte*: read one byte with no register.
        ///
        /// `i2c_smbus_read_byte()` (`smbus.cpp:82-91`).
        fn receive_byte(&self) -> io::Result<u8> {
            // Zero the whole union, not just the arm we read: the 34 bytes
            // are then initialised whatever the kernel chooses to copy back,
            // and reading `byte` afterwards is reading a byte this program
            // wrote.
            let mut data = I2cSmbusData {
                block: [0; I2C_SMBUS_BLOCK_MAX + 2],
            };
            smbus_access(
                self.file.as_raw_fd(),
                I2C_SMBUS_READ,
                0,
                I2C_SMBUS_BYTE,
                &raw mut data,
            )?;
            // SAFETY: every arm of the union is plain bytes with no invalid
            // bit patterns, and all 34 of them were initialised above, so
            // reading the `byte` arm is defined however the kernel filled the
            // object in. `smbus.cpp:90` reads the same arm.
            Ok(unsafe { data.byte })
        }
    }

    impl I2cBus for I2c {
        /// SMBus *write byte data*: `reg <- value`.
        ///
        /// `i2c_smbus_write_byte_data()` (`smbus.cpp:109-115`) — `read_write
        /// = I2C_SMBUS_WRITE`, `command = reg`, `size = I2C_SMBUS_BYTE_DATA`,
        /// `data.byte = value`.
        ///
        /// **A failure here is one register, not the run — but only if the
        /// caller goes through [`crate::hw::write_table`], which is where
        /// that policy lives and which cites the C for it.** This method is
        /// the transport: it reports the `errno` so `write_table` can name it
        /// in the log line, and it is `pub` only because it is the trait
        /// method a recording fake implements. Do not apply `?` to it on the
        /// ADV7513 path. Main tolerates a refused write at every one of its
        /// i2c write sites, the three mode registers and the power register
        /// included (`video.cpp:1716-1722`, `:2737-2745`); a `?` here would
        /// make one NAK exit 14 and leave the screen dark where Main would
        /// have shown a picture.
        fn write_reg(&mut self, reg: u8, value: u8) -> Result<()> {
            let mut data = I2cSmbusData {
                block: [0; I2C_SMBUS_BLOCK_MAX + 2],
            };
            // Writing a `Copy` arm of a union needs no `unsafe`; only reading
            // one does.
            data.byte = value;
            smbus_access(
                self.file.as_raw_fd(),
                I2C_SMBUS_WRITE,
                reg,
                I2C_SMBUS_BYTE_DATA,
                &raw mut data,
            )
            .map_err(|e| {
                Error::io(
                    format!(
                        "i2c write {reg:#04X}={value:#04X} to {:#04X} on {}",
                        self.addr,
                        bus_path(self.bus)
                    ),
                    e,
                )
            })
        }
    }

    /// `i2c_smbus_access()` (`smbus.cpp:61-74`): fill the ioctl argument and
    /// make the call.
    ///
    /// The C returns `-errno`; we return the `errno` as an
    /// [`io::Error`](std::io::Error) and let the caller decide, which is the
    /// same information in the shape the rest of this crate uses.
    fn smbus_access(
        fd: c_int,
        read_write: u8,
        command: u8,
        size: u32,
        data: *mut I2cSmbusData,
    ) -> io::Result<()> {
        let mut args = I2cSmbusIoctlData {
            read_write,
            command,
            size,
            data,
        };

        // SAFETY: `I2C_SMBUS` takes a pointer to one `i2c_smbus_ioctl_data`
        // (`linux/i2c-dev.h:19`), which is what is passed: a live, fully
        // initialised local whose layout is asserted in this module's tests.
        // `args.data` points at the caller's `I2cSmbusData`, which outlives
        // the call because the caller holds it on the stack across it. The
        // kernel may write through that pointer for a read transaction, and
        // may not write anywhere else. The descriptor is live for the call.
        let rc = unsafe { libc::ioctl(fd, I2C_SMBUS, &raw mut args) };
        if rc == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The ioctl argument's layout, which is invisible until the rig if
        /// it is wrong. `read_write` at 0, `command` at 1, two bytes of
        /// padding, `size` at 4, `data` at 8 — on a 32-bit ARM because the
        /// pointer's alignment is 4 and 4 + 4 = 8, and on a 64-bit host
        /// because the pointer's alignment is 8 and the compiler pads to it.
        #[test]
        fn ioctl_data_layout_matches_the_kernel_struct() {
            assert_eq!(offset_of!(I2cSmbusIoctlData, read_write), 0);
            assert_eq!(offset_of!(I2cSmbusIoctlData, command), 1);
            assert_eq!(offset_of!(I2cSmbusIoctlData, size), 4);
            assert_eq!(offset_of!(I2cSmbusIoctlData, data), 8);

            // 12 bytes on a 32-bit target, 16 on a 64-bit one; both are
            // "eight bytes then the pointer, rounded up to the pointer's
            // alignment", which is what the C compiler does to the same
            // declaration.
            let ptr = size_of::<*mut I2cSmbusData>();
            assert_eq!(size_of::<I2cSmbusIoctlData>(), 8 + ptr);
            assert_eq!(align_of::<I2cSmbusIoctlData>(), align_of::<*mut u8>());
        }

        /// `union i2c_smbus_data` is 34 bytes on every target: the block arm
        /// is `I2C_SMBUS_BLOCK_MAX + 2` and the widest scalar arm is a `u16`.
        /// If `block` were ever dropped from the declaration this would be 2,
        /// and a block transaction's reply would run off the end of a local.
        #[test]
        fn smbus_data_union_is_the_kernel_size() {
            assert_eq!(size_of::<I2cSmbusData>(), I2C_SMBUS_BLOCK_MAX + 2);
            assert_eq!(size_of::<I2cSmbusData>(), 34);
            assert_eq!(align_of::<I2cSmbusData>(), align_of::<u16>());
        }

        /// The numbers, against `/usr/include/linux/i2c-dev.h` and
        /// `/usr/include/linux/i2c.h` (and `smbus.cpp:26-44`, which agrees).
        /// Written as `libc::Ioctl` so the comparison is done in whichever
        /// type the target uses.
        #[test]
        fn i2c_constants_match_the_uapi_headers() {
            assert_eq!(I2C_SLAVE, 0x0703 as libc::Ioctl);
            assert_eq!(I2C_SMBUS, 0x0720 as libc::Ioctl);
            assert_eq!(I2C_SMBUS_READ, 1);
            assert_eq!(I2C_SMBUS_WRITE, 0);
            assert_eq!(I2C_SMBUS_BYTE, 1);
            assert_eq!(I2C_SMBUS_BYTE_DATA, 2);
            assert_eq!(I2C_SMBUS_BLOCK_MAX, 32);
        }

        /// The transaction descriptors this module builds, byte for byte,
        /// without an ioctl: the same two shapes `write_reg` and
        /// `receive_byte` hand to the kernel (`smbus.cpp:109-115` and
        /// `:82-91`).
        #[test]
        fn transaction_fields_match_the_c_helpers() {
            let mut data = I2cSmbusData {
                block: [0; I2C_SMBUS_BLOCK_MAX + 2],
            };
            data.byte = 0x5A;
            let write = I2cSmbusIoctlData {
                read_write: I2C_SMBUS_WRITE,
                command: 0x41,
                size: I2C_SMBUS_BYTE_DATA,
                data: &raw mut data,
            };
            assert_eq!(write.read_write, 0);
            assert_eq!(write.command, 0x41);
            assert_eq!(write.size, 2);
            // SAFETY: `data` is live and fully initialised here.
            assert_eq!(unsafe { (*write.data).byte }, 0x5A);

            let read = I2cSmbusIoctlData {
                read_write: I2C_SMBUS_READ,
                command: 0,
                size: I2C_SMBUS_BYTE,
                data: &raw mut data,
            };
            assert_eq!(read.read_write, 1);
            assert_eq!(read.command, 0);
            assert_eq!(read.size, 1);
        }

        /// `ENOENT` on `/dev/i2c-2` is a board with two adapters, not a
        /// failure; `EACCES` on `/dev/i2c-0` is a failure. The exit code
        /// depends on telling them apart.
        #[test]
        fn only_a_missing_node_means_the_bus_is_absent() {
            for errno in [libc::ENOENT, libc::ENODEV] {
                assert!(bus_is_absent(&io::Error::from_raw_os_error(errno)));
            }
            for errno in [libc::EACCES, libc::EPERM, libc::EBUSY, libc::ENOMEM] {
                assert!(!bus_is_absent(&io::Error::from_raw_os_error(errno)));
            }
        }

        /// A chip that does not acknowledge is `ENXIO`, `EREMOTEIO` or
        /// `ETIMEDOUT` depending on the adapter; a descriptor or adapter that
        /// cannot do the transaction at all is not, and must not be read as
        /// "no ADV7513 here".
        #[test]
        fn only_a_nak_means_nobody_answered() {
            for errno in [libc::ENXIO, libc::EREMOTEIO, libc::ETIMEDOUT] {
                assert!(no_one_answered(&io::Error::from_raw_os_error(errno)));
            }
            for errno in [libc::EACCES, libc::EBADF, libc::ENOTTY, libc::EOPNOTSUPP] {
                assert!(!no_one_answered(&io::Error::from_raw_os_error(errno)));
            }
        }

        /// The real `Regs` implements the trait the fake does. This cannot be
        /// exercised on a host — there is no FPGA manager to map — so the
        /// most a host test can do is insist the impl exists.
        #[test]
        fn mem_regs_is_a_regs() {
            const fn assert_regs<R: mailbox::Regs>() {}
            assert_regs::<MemRegs>();
        }
    }
}

#[cfg(target_os = "linux")]
pub use linux::{DEV_MEM, I2c, MemRegs};

/// Stand-ins for the Linux-only types on any other OS.
///
/// `/dev/mem`, i2c-dev, sysfs module parameters and `/dev/tty1` are Linux
/// interfaces; off Linux there is nothing to fall back *to*, so the types
/// exist with the same signatures and their constructors fail with
/// [`std::io::ErrorKind::Unsupported`]. The structs are uninhabited, so no
/// value of either can be produced and the trait bodies below are unreachable
/// rather than wrong. Both targets this crate ships to are Linux; this arm is
/// only here so the crate type-checks elsewhere.
#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
mod unsupported {
    use super::I2cBus;
    use crate::mailbox;
    use crate::{Error, Result};
    use std::io;

    /// The physical-memory device, for the message.
    pub const DEV_MEM: &str = "/dev/mem";

    fn unsupported(what: &str) -> Error {
        Error::io(
            what.to_string(),
            io::Error::new(
                io::ErrorKind::Unsupported,
                "this interface exists only on Linux",
            ),
        )
    }

    /// See the module doc: uninhabited, so [`Self::open`] is the only way in
    /// and it never returns one.
    pub struct MemRegs {
        never: std::convert::Infallible,
    }

    impl MemRegs {
        /// Always [`Error::Io`] with [`io::ErrorKind::Unsupported`].
        pub fn open() -> Result<Self> {
            Err(unsupported("mmap /dev/mem"))
        }
    }

    impl mailbox::Regs for MemRegs {
        fn gpo_write(&mut self, _value: u32) {
            // Unreachable: `MemRegs` is uninhabited off Linux.
        }

        fn gpi_read(&self) -> u32 {
            0
        }
    }

    /// See the module doc: uninhabited off Linux.
    pub struct I2c {
        never: std::convert::Infallible,
    }

    impl I2c {
        /// Always [`Error::Io`] with [`io::ErrorKind::Unsupported`].
        pub fn open_bus(bus: u8, _addr: u8) -> Result<Self> {
            Err(unsupported(&super::bus_path(bus)))
        }

        /// Always [`Error::Io`] with [`io::ErrorKind::Unsupported`].
        pub fn open_adv7513() -> Result<Self> {
            Err(unsupported("/dev/i2c-*"))
        }

        /// Unreachable: no value of this type exists.
        pub fn bus(&self) -> u8 {
            0
        }

        /// Unreachable: no value of this type exists.
        pub fn addr(&self) -> u8 {
            0
        }
    }

    impl I2cBus for I2c {
        fn write_reg(&mut self, _reg: u8, _value: u8) -> Result<()> {
            Err(unsupported("/dev/i2c-*"))
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub use unsupported::{DEV_MEM, I2c, MemRegs};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::ErrorKind;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A recording [`I2cBus`] that refuses the registers it is told to.
    struct FakeBus {
        writes: Vec<(u8, u8)>,
        nak: Vec<u8>,
    }

    impl FakeBus {
        fn new(nak: &[u8]) -> Self {
            Self {
                writes: Vec::new(),
                nak: nak.to_vec(),
            }
        }
    }

    impl I2cBus for FakeBus {
        fn write_reg(&mut self, reg: u8, value: u8) -> Result<()> {
            self.writes.push((reg, value));
            if self.nak.contains(&reg) {
                // What a NAK looks like coming back out of the ioctl: the
                // adapter reports ENXIO and `i2c_smbus_access` turns it into
                // `-errno` (`smbus.cpp:73`).
                return Err(Error::io(
                    format!("i2c write {reg:#04X}"),
                    std::io::Error::from_raw_os_error(libc::ENXIO),
                ));
            }
            Ok(())
        }
    }

    /// A unique scratch path, since there is no temp-file crate to lean on
    /// and `/tmp` is shared.
    fn scratch(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("itsalive-hw-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn bus_paths_are_the_c_sprintf() {
        // smbus.cpp:226 — `sprintf(str, "/dev/i2c-%d", bus)`.
        assert_eq!(bus_path(0), "/dev/i2c-0");
        assert_eq!(bus_path(1), "/dev/i2c-1");
        assert_eq!(bus_path(2), "/dev/i2c-2");
        assert_eq!(I2C_BUSES, [0, 1, 2]);
    }

    #[test]
    fn discovery_probes_every_bus_in_order() {
        let mut seen = Vec::new();
        let _ = discover_bus(|bus| {
            seen.push(bus);
            Ok(None::<u8>)
        });
        // The C stops at the first answer; we do not, which is the whole
        // point of the divergence.
        assert_eq!(seen, vec![0, 1, 2]);
    }

    #[test]
    fn one_answer_is_the_bus() {
        let got = discover_bus(|bus| Ok((bus == 1).then_some(bus)));
        assert_eq!(got.ok(), Some(1));
    }

    #[test]
    fn no_answer_is_no_adv7513() {
        let err = discover_bus(|_| Ok(None::<u8>)).unwrap_err();
        assert!(matches!(err, Error::NoAdv7513));
        assert_eq!(err.exit_code(), 12);
    }

    #[test]
    fn two_answers_are_ambiguous_and_name_the_buses() {
        let err = discover_bus(|bus| Ok((bus != 1).then_some(bus))).unwrap_err();
        match &err {
            Error::AmbiguousBus(buses) => assert_eq!(buses, &[0, 2]),
            other => panic!("expected AmbiguousBus, got {other:?}"),
        }
        assert_eq!(err.exit_code(), 13);
    }

    #[test]
    fn three_answers_are_ambiguous_too() {
        let err = discover_bus(|bus| Ok(Some(bus))).unwrap_err();
        match err {
            Error::AmbiguousBus(buses) => assert_eq!(buses, vec![0, 1, 2]),
            other => panic!("expected AmbiguousBus, got {other:?}"),
        }
    }

    /// A bus that is there and will not let us in is exit 14, not exit 12.
    ///
    /// `docs/ARCHITECTURE.md` §7 gives 12 to "ADV7513 not found on any bus",
    /// which the installer passes over silently, and 14 to "`/dev/i2c-*` …
    /// could not be opened". Running the tool as a non-root user is the
    /// everyday way to produce this, and it must not read as "your board has
    /// no HDMI transmitter".
    #[test]
    fn a_bus_that_refuses_us_is_exit_14() {
        let err = discover_bus(|bus| {
            if bus == 0 {
                Err(Error::io(
                    "open /dev/i2c-0",
                    std::io::Error::from_raw_os_error(libc::EACCES),
                ))
            } else {
                Ok(None::<u8>)
            }
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 14);
        match err {
            Error::Io { source, .. } => assert_eq!(source.raw_os_error(), Some(libc::EACCES)),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    /// The scan runs to the end, so a bus we cannot use cannot cost us the
    /// one the chip is on. That is the C's `continue` (`smbus.cpp:228-239`)
    /// kept where it earns its keep.
    #[test]
    fn an_unusable_bus_does_not_hide_the_one_that_answers() {
        let got = discover_bus(|bus| match bus {
            0 => Err(Error::io(
                "open /dev/i2c-0",
                std::io::Error::from_raw_os_error(libc::EBUSY),
            )),
            2 => Ok(Some(2)),
            _ => Ok(None),
        });
        assert_eq!(got.ok(), Some(2));
    }

    /// Two answers are still ambiguous even when a third bus was unusable:
    /// the error we could not act on does not displace the one we can.
    #[test]
    fn ambiguity_outranks_an_unusable_bus() {
        let err = discover_bus(|bus| match bus {
            0 => Err(Error::io(
                "open /dev/i2c-0",
                std::io::Error::from_raw_os_error(libc::EACCES),
            )),
            _ => Ok(Some(bus)),
        })
        .unwrap_err();
        assert_eq!(err.exit_code(), 13);
        match err {
            Error::AmbiguousBus(buses) => assert_eq!(buses, vec![1, 2]),
            other => panic!("expected AmbiguousBus, got {other:?}"),
        }
    }

    /// One message, from the first bus that gave one, so the line names a bus
    /// rather than summarising three.
    #[test]
    fn the_first_unusable_bus_is_the_one_reported() {
        let err = discover_bus(|bus| {
            Err::<Option<u8>, _>(Error::io(
                format!("open /dev/i2c-{bus}"),
                std::io::Error::from_raw_os_error(libc::EACCES),
            ))
        })
        .unwrap_err();
        // Not `err.to_string()`: `strerror` is locale-dependent, and this is
        // about which bus is named, not about how the C library spells
        // EACCES.
        match err {
            Error::Io { what, source } => {
                assert_eq!(what, "open /dev/i2c-0");
                assert_eq!(source.raw_os_error(), Some(libc::EACCES));
            }
            other => panic!("expected Io, got {other:?}"),
        }
    }

    /// [`log`] must return even when the line cannot be written. `eprintln!`
    /// panics there, and with `panic = "abort"` that is a SIGABRT whose exit
    /// status is not one of the `docs/ARCHITECTURE.md` §7 codes.
    #[test]
    fn logging_survives_a_sink_that_refuses_the_line() {
        struct Full;
        impl std::io::Write for Full {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::from_raw_os_error(libc::ENOSPC))
            }
        }
        // The assertion is that this returns at all.
        log_to(Full, format_args!("i2c: write error (41 50): {}", 6));
    }

    /// And that it is otherwise `eprintln!`: one line, newline included.
    #[test]
    fn logging_writes_one_line() {
        let mut sink: Vec<u8> = Vec::new();
        log_to(
            &mut sink,
            format_args!("i2c: write error ({:02X} {:02X}): x", 0x41, 0x50),
        );
        assert_eq!(sink, b"i2c: write error (41 50): x\n");
    }

    /// A refused write must not end the table: that is `video.cpp:1608-1609`.
    #[test]
    fn a_nak_does_not_stop_the_table() {
        let table = [(0x98, 0x03), (0x41, 0x10), (0xFA, 0x7D)];
        let mut bus = FakeBus::new(&[0x41]);
        let failed = write_table(&mut bus, table);
        assert_eq!(failed, 1);
        assert_eq!(bus.writes, table);
    }

    #[test]
    fn a_table_that_all_works_reports_no_failures() {
        let mut bus = FakeBus::new(&[]);
        assert_eq!(write_table(&mut bus, crate::adv7513::writes()), 0);
        assert_eq!(bus.writes.len(), crate::adv7513::writes().count());
    }

    /// The tables go out in Main's order through the same call the hardware
    /// uses, so T2.2's end-to-end assertion has something to stand on.
    #[test]
    fn write_table_preserves_order() {
        let mut bus = FakeBus::new(&[]);
        write_table(&mut bus, crate::adv7513::writes());
        assert_eq!(bus.writes.first().copied(), Some((0x98, 0x03)));
        assert_eq!(
            bus.writes.last().copied(),
            crate::adv7513::CSC.last().copied()
        );
    }

    /// The three mode registers and the power register go through the same
    /// tolerant path as the bulk tables, because Main tolerates a NAK on them
    /// too (`video.cpp:1716-1722` and `:2737-2745`). This is the shape T2.2
    /// is meant to copy: `write_table`, not `write_reg` with a `?`, which
    /// would turn one NAK into exit 14 and a dark screen.
    #[test]
    fn the_mode_and_power_registers_go_through_the_tolerant_path() {
        let mut bus = FakeBus::new(&[0x3C, 0x41]);
        // 720p: hpol = 0, vpol = 0, VIC 4, no pixel repetition.
        assert_eq!(
            write_table(&mut bus, crate::adv7513::mode_regs(0, 0, 4, 0)),
            1
        );
        assert_eq!(write_table(&mut bus, [crate::adv7513::POWER_DOWN]), 1);
        // Every write was attempted: the refused 0x3C did not stop 0x41.
        assert_eq!(
            bus.writes,
            vec![(0x17, 0x62), (0x3B, 0x40), (0x3C, 0x04), (0x41, 0x50)]
        );
    }

    #[test]
    fn mode_file_writes_the_line_verbatim() {
        let path = scratch("mode");
        let mut sink = ModeFile::at(&path);
        let line = crate::fb::mode_param_line(&crate::fb::FbGeometry::for_mode(1280, 720, false));
        sink.write_mode_line(&line).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "8888 1 1280 720 5120\n");
        fs::remove_file(&path).unwrap();
    }

    /// The knob is rewritten, not appended to: `fopen(..., "wt")` truncates
    /// (`video.cpp:3465`), and a second `up` in the same boot must not leave
    /// two lines behind.
    #[test]
    fn mode_file_truncates_on_rewrite() {
        let path = scratch("mode-twice");
        let mut sink = ModeFile::at(&path);
        sink.write_mode_line("8888 1 1280 720 5120\n").unwrap();
        sink.write_mode_line("8888 1 640 480 2560\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "8888 1 640 480 2560\n");
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn mode_file_defaults_to_the_kernel_knob() {
        assert_eq!(ModeFile::new().path(), Path::new(FB_MODE_PATH));
        assert_eq!(
            ModeFile::default().path(),
            Path::new("/sys/module/MiSTer_fb/parameters/mode")
        );
    }

    #[test]
    fn an_unwritable_mode_path_is_exit_14() {
        let err = ModeFile::at("/proc/itsalive/does/not/exist")
            .write_mode_line("8888 1 1280 720 5120\n")
            .unwrap_err();
        assert_eq!(err.exit_code(), 14);
    }

    #[test]
    fn tty_writes_every_byte() {
        let path = scratch("tty");
        fs::write(&path, b"").unwrap();
        let mut sink = Tty::at(&path).unwrap();
        sink.write_text(b"itsalive: step 1\n").unwrap();
        sink.write_text(b"itsalive: step 2\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "itsalive: step 1\nitsalive: step 2\n"
        );
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_missing_tty_is_exit_14() {
        let path = scratch("tty-missing");
        let err = Tty::at(&path).unwrap_err();
        assert_eq!(err.exit_code(), 14);
        match err {
            Error::Io { source, .. } => assert_eq!(source.kind(), ErrorKind::NotFound),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    /// The three traits stay object-safe, so T2.2 can hold them as `dyn` if
    /// that turns out to be simpler than another generic parameter.
    #[test]
    fn the_traits_are_object_safe() {
        let mut bus = FakeBus::new(&[]);
        let erased: &mut dyn I2cBus = &mut bus;
        erased.write_reg(0x41, 0x10).unwrap();
        let _: Option<&dyn ModeSink> = None;
        let _: Option<&dyn TextSink> = None;
        assert_eq!(bus.writes, vec![(0x41, 0x10)]);
    }

    /// `write_table` takes `?Sized`, so a `dyn I2cBus` can be driven through
    /// it without another monomorphisation.
    #[test]
    fn write_table_accepts_a_trait_object() {
        let mut bus = FakeBus::new(&[]);
        let erased: &mut dyn I2cBus = &mut bus;
        assert_eq!(write_table(erased, [(0x17, 0x62), (0x3B, 0x40)]), 0);
        assert_eq!(bus.writes, vec![(0x17, 0x62), (0x3B, 0x40)]);
    }

    // -----------------------------------------------------------------------
    // Reading the knob back, and /dev/fb0
    // -----------------------------------------------------------------------

    /// The same file both ways: what [`ModeSink`] wrote is what [`ModeSource`]
    /// reads.
    #[test]
    fn the_mode_knob_reads_back_what_was_written() {
        let path = scratch("mode-readback");
        let mut knob = ModeFile::at(&path);
        knob.write_mode_line("8888 1 1280 720 5120\n").unwrap();
        assert_eq!(knob.read_mode_line().unwrap(), "8888 1 1280 720 5120\n");
        knob.write_mode_line("8888 1 640 480 2560\n").unwrap();
        assert_eq!(knob.read_mode_line().unwrap(), "8888 1 640 480 2560\n");
        fs::remove_file(&path).unwrap();
    }

    /// A knob whose driver never probed: `mode_get` returns 0 bytes
    /// (`MiSTer_fb.c:364-371`), which is an empty read and not an error. The
    /// grammar, and the message about `fb enable`, belong to
    /// [`crate::fb::parse_mode_line`].
    #[test]
    fn an_empty_knob_reads_as_the_empty_string() {
        let path = scratch("mode-empty");
        fs::write(&path, b"").unwrap();
        assert_eq!(ModeFile::at(&path).read_mode_line().unwrap(), "");
        assert!(crate::fb::parse_mode_line("").is_err());
        fs::remove_file(&path).unwrap();
    }

    /// A knob that cannot be read at all is exit 14, which is the other half
    /// of the split: unreadable is I/O, unparseable is usage.
    #[test]
    fn an_unreadable_mode_knob_is_exit_14() {
        let err = ModeFile::at("/proc/itsalive/does/not/exist")
            .read_mode_line()
            .unwrap_err();
        assert_eq!(err.exit_code(), 14);
    }

    /// [`PixelSink::write_at`] seeks first, so two rows written to unrelated
    /// offsets land where they were addressed and the gap between them is
    /// untouched.
    #[test]
    fn the_pixel_sink_writes_at_the_offset_it_is_given() {
        let path = scratch("fb");
        fs::write(&path, vec![0xAAu8; 32]).unwrap();
        let mut fbdev = FbDevice::at(&path).unwrap();
        fbdev.write_at(4, b"BGRX").unwrap();
        fbdev.write_at(20, b"bgrx").unwrap();

        let mut want = vec![0xAAu8; 32];
        want.splice(4..8, *b"BGRX");
        want.splice(20..24, *b"bgrx");
        assert_eq!(fs::read(&path).unwrap(), want);
        assert_eq!(fbdev.path(), path);
        fs::remove_file(&path).unwrap();
    }

    /// Opening the device must not truncate it: a framebuffer is a device
    /// whose contents are the screen, and a truncating open would blank it
    /// before the plan had decided anything.
    #[test]
    fn opening_the_pixel_sink_keeps_what_is_there() {
        let path = scratch("fb-notrunc");
        fs::write(&path, vec![0x5Au8; 16]).unwrap();
        let _fbdev = FbDevice::at(&path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), vec![0x5Au8; 16]);
        fs::remove_file(&path).unwrap();
    }

    /// `--clear` zeroes exactly `len` bytes and not one more, in page-sized
    /// writes from the top. The length is `stride * height`, which is the
    /// driver's own `smem_len` (`MiSTer_fb.c:239`).
    #[test]
    fn clearing_zeroes_exactly_the_length_asked_for() {
        /// A [`PixelSink`] that records `(offset, len)` and applies the bytes.
        struct Recording {
            buf: Vec<u8>,
            writes: Vec<(u64, usize)>,
        }

        impl PixelSink for Recording {
            fn write_at(&mut self, offset: u64, bytes: &[u8]) -> Result<()> {
                self.writes.push((offset, bytes.len()));
                let at = usize::try_from(offset).unwrap();
                self.buf
                    .splice(at..at.saturating_add(bytes.len()), bytes.iter().copied());
                Ok(())
            }
        }

        // Under one page: one write, and the byte past the end survives.
        let mut sink = Recording {
            buf: vec![0xFFu8; 100],
            writes: Vec::new(),
        };
        sink.clear(64).unwrap();
        assert_eq!(sink.writes, vec![(0, 64)]);
        assert_eq!(sink.buf.get(..64), Some(&[0u8; 64][..]));
        assert_eq!(sink.buf.get(64), Some(&0xFF), "one byte past `len`");

        // Over one page: whole pages from the top, then the remainder.
        let mut sink = Recording {
            buf: vec![0xFFu8; 10_000],
            writes: Vec::new(),
        };
        sink.clear(9000).unwrap();
        assert_eq!(sink.writes, vec![(0, 4096), (4096, 4096), (8192, 808)]);
        assert_eq!(sink.buf.get(..9000), Some(&vec![0u8; 9000][..]));
        assert_eq!(sink.buf.get(9000), Some(&0xFF));

        // Nothing to clear is no writes at all.
        let mut sink = Recording {
            buf: vec![0xFFu8; 8],
            writes: Vec::new(),
        };
        sink.clear(0).unwrap();
        assert!(sink.writes.is_empty());
    }

    /// A missing `/dev/fb0` is exit 14, not a created file.
    #[test]
    fn a_missing_framebuffer_is_exit_14() {
        let path = scratch("fb-missing");
        let err = FbDevice::at(&path).unwrap_err();
        assert_eq!(err.exit_code(), 14);
        match err {
            Error::Io { source, .. } => assert_eq!(source.kind(), ErrorKind::NotFound),
            other => panic!("expected Io, got {other:?}"),
        }
        assert!(!path.exists(), "`at` must not create the device");
    }

    #[test]
    fn the_framebuffer_defaults_to_dev_fb0() {
        assert_eq!(FB_DEV_PATH, "/dev/fb0");
    }

    /// `read_source` reads a whole file, and reports a missing one as exit 14.
    #[test]
    fn a_source_file_is_read_whole() {
        let path = scratch("image-src");
        let pixels: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        fs::write(&path, &pixels).unwrap();
        let got = read_source(path.to_str().unwrap()).unwrap();
        assert_eq!(got, pixels);
        fs::remove_file(&path).unwrap();

        let missing = scratch("image-src-missing");
        let err = read_source(missing.to_str().unwrap()).unwrap_err();
        assert_eq!(err.exit_code(), 14);
    }

    /// The two new traits are object-safe, like the other four, so the CLI can
    /// hold them as `dyn` if that is ever simpler than another generic.
    #[test]
    fn the_new_traits_are_object_safe_too() {
        let _: Option<&dyn ModeSource> = None;
        let _: Option<&mut dyn PixelSink> = None;
    }
}
