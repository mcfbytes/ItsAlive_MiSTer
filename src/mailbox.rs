//! The fabric mailbox — the bit-banged handshake Main_MiSTer calls "SPI".
//!
//! It is not SPI. It is two 32-bit registers in the Cyclone V FPGA manager
//! block: `GPO` (HPS to fabric, write-only) and `GPI` (fabric to HPS, read).
//! One 16-bit word is transferred by placing the word in `GPO[15:0]`, raising
//! a strobe bit, waiting for the fabric to mirror it back as an ack, lowering
//! the strobe and waiting for the ack to clear. See `docs/ARCHITECTURE.md` §2.
//!
//! Transcribed from Main_MiSTer at commit `6cda9cc`: `fpga_io.cpp:510-519`
//! (the `gpo_copy` shadow and the two accessors), `:665-672` (`SSPI_STROBE`,
//! `SSPI_ACK`, `fpga_spi_en`), `:688-721` (`fpga_spi`), `spi.cpp:5-7` (the
//! enable bits), `:45-53` (`EnableIO`/`DisableIO`) and `:106-117`
//! (`spi_uio_cmd_cont`/`spi_uio_cmd`).
//!
//! Two deliberate departures from the C, both forced by this tool having to
//! return where Main_MiSTer takes the machine over:
//!
//! * Where `fpga_spi()` sees GPI bit 31 set it prints, calls
//!   `fpga_wait_to_reset()` and reboots the board (`fpga_io.cpp:699-704`,
//!   `:674-686`). We return [`Error::NoBitstream`] on the spot and never spin.
//! * Where `fpga_spi()` polls for the ack forever, every poll here is bounded
//!   by a [`Deadline`] and gives up with [`Error::Timeout`].
//!
//! Because of that, an error can leave a transfer half finished, which the C
//! never has to think about. Every exit path here lowers the strobe and then
//! drops the enable bit, so the shadow and the wire are left exactly as a
//! completed transfer leaves them and the next [`Mailbox::enable_io`] cannot
//! present a stale strobe edge to the fabric.

use crate::{Error, Result};
use std::time::{Duration, Instant};

/// Base of the Cyclone V FPGA manager block.
///
/// `fpga_base_addr_ac5.h:15` — `SOCFPGA_MGR_ADDRESS`.
pub const FPGA_MANAGER_BASE: usize = 0xFF70_6000;

/// Offset of the GPO register (HPS to fabric) from [`FPGA_MANAGER_BASE`].
///
/// `fpga_io.cpp:514` — `writel(value, SOCFPGA_MGR_ADDRESS + 0x10)`.
pub const GPO_OFFSET: usize = 0x10;

/// Offset of the GPI register (fabric to HPS) from [`FPGA_MANAGER_BASE`].
///
/// `fpga_io.cpp:519` — `readl(SOCFPGA_MGR_ADDRESS + 0x14)`.
pub const GPI_OFFSET: usize = 0x14;

/// The data field of GPO: the 16-bit word being transferred.
///
/// `fpga_io.cpp:690` — the `0xFFFF` in `gpo & ~(0xFFFF | SSPI_STROBE)`.
const DATA_MASK: u32 = 0xFFFF;

/// Strobe, GPO bit 17. `fpga_io.cpp:665` — `#define SSPI_STROBE (1<<17)`.
///
/// Public so that a fake [`Regs`] outside this module can mirror the strobe
/// into the ack the way the fabric does; `docs/ARCHITECTURE.md` §2 already
/// publishes the bit position.
pub const SSPI_STROBE: u32 = 1 << 17;

/// Ack, GPI bit 17 — the same bit position as the strobe.
///
/// `fpga_io.cpp:666` — `#define SSPI_ACK SSPI_STROBE`.
///
/// Public for the same reason as [`SSPI_STROBE`].
pub const SSPI_ACK: u32 = SSPI_STROBE;

/// GPO bit 31, ORed in by every enable change.
///
/// `fpga_io.cpp:670` — `uint32_t gpo = fpga_gpo_read() | 0x80000000;`. It is
/// never cleared again, so it is set in every word written after the first
/// enable call.
const SSPI_EN_BIT31: u32 = 0x8000_0000;

/// GPI bit 31: the fabric is not in user mode, i.e. no bitstream is loaded.
///
/// `fpga_io.cpp:699` — the C reads GPI into a signed `int` and tests
/// `if (gpi < 0)`, which is exactly "bit 31 set".
///
/// Public because the bit is also readable on its own, with no transfer and
/// no side effect: `is_fpga_ready(1)` is `return (fpga_gpi_read() >= 0);`
/// (`fpga_io.cpp:655-662`). The CLI asks that question before it writes 92
/// registers at a chip the fabric is not routing (`docs/ARCHITECTURE.md` §1),
/// and `probe` asks it without disturbing anything at all.
pub const GPI_NOT_READY: u32 = 1 << 31;

/// Chip select for the core itself. `spi.cpp:5` — `SSPI_FPGA_EN (1<<18)`.
pub const SSPI_FPGA_EN: u32 = 1 << 18;

/// Chip select for the OSD. `spi.cpp:6` — `SSPI_OSD_EN (1<<19)`.
pub const SSPI_OSD_EN: u32 = 1 << 19;

/// Chip select for the user_io mailbox, the only one this tool uses.
///
/// `spi.cpp:7` — `SSPI_IO_EN (1<<20)`.
pub const SSPI_IO_EN: u32 = 1 << 20;

/// Set the video mode and PLL block. `user_io.h:42` — `UIO_SET_VIDEO 0x20`.
pub const UIO_SET_VIDEO: u16 = 0x20;

/// Control the on-board LEDs. `user_io.h:47` — `UIO_LEDS 0x25`.
pub const UIO_LEDS: u16 = 0x25;

/// Set the HPS frame buffer. `user_io.h:57` — `UIO_SET_FBUF 0x2F`.
pub const UIO_SET_FBUF: u16 = 0x2F;

/// How long an ack poll may take before it is called a failure.
///
/// Main_MiSTer has no timeout at all; this is ours. A healthy fabric acks in
/// microseconds, so 10 ms is generous (`docs/ARCHITECTURE.md` §2).
pub const ACK_TIMEOUT: Duration = Duration::from_millis(10);

/// The two mailbox registers, so the framing can be tested without hardware.
pub trait Regs {
    /// Write the GPO register at [`FPGA_MANAGER_BASE`] + [`GPO_OFFSET`].
    ///
    /// `fpga_io.cpp:511-515` — `fpga_gpo_write()`. Note that the C has a
    /// second, macro form (`fpga_gpo_writeN`, `fpga_io.cpp:517`) that skips
    /// the shadow copy; only the fast block helpers use it
    /// (`fpga_io.cpp:741-742`). Every write on the path transcribed here goes
    /// through the shadow-updating form, which is why [`Mailbox`] updates its
    /// shadow on every write.
    fn gpo_write(&mut self, value: u32);

    /// Read the GPI register at [`FPGA_MANAGER_BASE`] + [`GPI_OFFSET`].
    ///
    /// `fpga_io.cpp:519` — `fpga_gpi_read()`. The C casts to a signed `int`
    /// purely so that bit 31 can be tested as `gpi < 0`; we keep it unsigned
    /// and test the bit.
    fn gpi_read(&self) -> u32;
}

/// A bounded wait, so no ack poll can hang the installer.
///
/// [`Mailbox`] calls [`start`](Deadline::start) once per poll loop and then
/// [`expired`](Deadline::expired) after each unsuccessful GPI read, so a test
/// implementation can force a timeout without any sleeping.
pub trait Deadline {
    /// Begin a fresh timing window.
    fn start(&mut self);

    /// Has the window opened by the last [`start`](Deadline::start) closed?
    fn expired(&self) -> bool;
}

/// The real [`Deadline`]: a monotonic clock and a fixed limit.
#[derive(Debug, Clone)]
pub struct MonotonicDeadline {
    limit: Duration,
    started: Instant,
}

impl MonotonicDeadline {
    /// A deadline `limit` long, measured from each [`Deadline::start`].
    pub fn new(limit: Duration) -> Self {
        Self {
            limit,
            started: Instant::now(),
        }
    }
}

impl Default for MonotonicDeadline {
    fn default() -> Self {
        Self::new(ACK_TIMEOUT)
    }
}

impl Deadline for MonotonicDeadline {
    fn start(&mut self) {
        self.started = Instant::now();
    }

    fn expired(&self) -> bool {
        self.started.elapsed() >= self.limit
    }
}

/// The mailbox: a pair of registers, a deadline and the GPO shadow copy.
///
/// The shadow exists because GPO is write-only in practice — the C keeps
/// `gpo_copy` and never reads the register back (`fpga_io.cpp:510-518`). We
/// start it at zero rather than adopting whatever Main_MiSTer last left
/// there, so a run is reproducible from the first write.
pub struct Mailbox<R: Regs, D> {
    regs: R,
    deadline: D,
    /// Mirror of `gpo_copy` (`fpga_io.cpp:510`).
    shadow: u32,
}

impl<R: Regs, D: Deadline> Mailbox<R, D> {
    /// A mailbox with a zeroed shadow.
    pub fn new(regs: R, deadline: D) -> Self {
        Self {
            regs,
            deadline,
            shadow: 0,
        }
    }

    /// The registers, for callers that need to look at them.
    pub fn regs(&self) -> &R {
        &self.regs
    }

    /// The current GPO shadow, for logging.
    pub fn shadow(&self) -> u32 {
        self.shadow
    }

    /// `fpga_gpo_write()` (`fpga_io.cpp:511-515`): write GPO *and* update the
    /// shadow.
    fn write_gpo(&mut self, gpo: u32) {
        self.shadow = gpo;
        self.regs.gpo_write(gpo);
    }

    /// `fpga_spi_en()` (`fpga_io.cpp:668-672`).
    ///
    /// Bit 31 is ORed into the shadow *before* the mask is applied, so it is
    /// set by an enable and stays set through the matching disable.
    fn spi_en(&mut self, mask: u32, en: bool) {
        let gpo = self.shadow | SSPI_EN_BIT31;
        self.write_gpo(if en { gpo | mask } else { gpo & !mask });
    }

    /// `EnableIO()` (`spi.cpp:45-48`): raise [`SSPI_IO_EN`].
    pub fn enable_io(&mut self) {
        self.spi_en(SSPI_IO_EN, true);
    }

    /// `DisableIO()` (`spi.cpp:50-53`): drop [`SSPI_IO_EN`].
    pub fn disable_io(&mut self) {
        self.spi_en(SSPI_IO_EN, false);
    }

    /// Transfer one 16-bit word and return the fabric's reply.
    ///
    /// `fpga_spi()` (`fpga_io.cpp:688-721`), reached through `spi_w()`
    /// (`spi.h:25-28`):
    ///
    /// 1. `gpo = (shadow & ~(0xFFFF | SSPI_STROBE)) | word` — keep bit 31 and
    ///    the enable bits, clear the previous word and the strobe.
    /// 2. Write `gpo`, then `gpo | SSPI_STROBE`.
    /// 3. Poll GPI until the ack bit is set.
    /// 4. Write `gpo` again (strobe low), poll GPI until the ack bit clears,
    ///    and return that read's low 16 bits.
    pub fn spi_w(&mut self, word: u16) -> Result<u16> {
        // fpga_io.cpp:690
        let gpo = (self.shadow & !(DATA_MASK | SSPI_STROBE)) | u32::from(word);

        // fpga_io.cpp:692-693
        self.write_gpo(gpo);
        self.write_gpo(gpo | SSPI_STROBE);

        // fpga_io.cpp:696-705: wait for the ack to rise.
        if let Err(e) = self.wait_ack(true) {
            // The C cannot reach this; we can, so lower the strobe and leave
            // the bus exactly as a completed transfer leaves it.
            self.write_gpo(gpo);
            return Err(e);
        }

        // fpga_io.cpp:707
        self.write_gpo(gpo);

        // fpga_io.cpp:709-718: wait for the ack to fall. The strobe is
        // already low here, so an error needs no extra write.
        let gpi = self.wait_ack(false)?;

        // fpga_io.cpp:720 — `return (uint16_t)gpi;`
        Ok(gpi as u16)
    }

    /// Poll GPI until the ack bit reaches `want_set`, or fail.
    ///
    /// `fpga_io.cpp:696-705` and `:709-718`, which are the same loop twice.
    /// Bit 31 is tested first and on every read of both loops, because that
    /// is what the C does: `if (gpi < 0)` (`:699`, `:712`) is the body of the
    /// `do`, so it runs before `while (!(gpi & SSPI_ACK))` (`:705`) or
    /// `while (gpi & SSPI_ACK)` (`:718`) is ever evaluated. The order is not
    /// cosmetic — an unconfigured fabric can present bit 31 and the ack bit
    /// in the same read, and that read must abort, not complete a transfer.
    ///
    /// The deadline check is last for the same reason: it must not be able to
    /// turn a [`Error::NoBitstream`] into a [`Error::Timeout`].
    fn wait_ack(&mut self, want_set: bool) -> Result<u32> {
        self.deadline.start();
        loop {
            let gpi = self.regs.gpi_read();

            // fpga_io.cpp:699-704 and :712-717 — `if (gpi < 0)`: no
            // bitstream. The C reboots the board here; we refuse instead.
            if gpi & GPI_NOT_READY != 0 {
                return Err(Error::NoBitstream);
            }

            // fpga_io.cpp:705 / :718 — `while (!(gpi & SSPI_ACK))` and
            // `while (gpi & SSPI_ACK)`.
            if (gpi & SSPI_ACK != 0) == want_set {
                return Ok(gpi);
            }

            // Ours, not the C's: the poll is bounded.
            if self.deadline.expired() {
                return Err(Error::Timeout);
            }
            std::hint::spin_loop();
        }
    }

    /// Send one user_io command: opcode, then payload, then release the bus.
    ///
    /// `spi_uio_cmd()` (`spi.cpp:112-117`) is `spi_uio_cmd_cont()`
    /// (`:106-110`) — `EnableIO()` plus one `spi_w(cmd)` — followed by
    /// `DisableIO()`. `set_video()` is exactly that shape with its payload in
    /// between: `spi_uio_cmd_cont(UIO_SET_VIDEO)` at `video.cpp:2266`, the 26
    /// `spi_w()`s of `:2268-2291`, `DisableIO()` at `:2294`, and the reply to
    /// the opcode word dropped on the floor. The value returned here is that
    /// reply, which is what `spi_uio_cmd()` returns.
    ///
    /// **Not every sender has this shape.** `video_fb_enable()` sends its
    /// payload only when the opcode's reply is non-zero
    /// (`video.cpp:3480-3481`), so `UIO_SET_FBUF` goes through
    /// [`command_if_supported`](Mailbox::command_if_supported) and never
    /// through this method.
    ///
    /// The enable bit is dropped on every path out, including errors.
    pub fn command(&mut self, opcode: u16, payload: &[u16]) -> Result<u16> {
        self.enable_io();
        let reply = self.burst(opcode, payload, false);
        self.disable_io();
        reply
    }

    /// Send one user_io command whose payload the core first has to ask for.
    ///
    /// `video_fb_enable()` (`video.cpp:3474-3543`) opens with
    /// `int res = spi_uio_cmd_cont(UIO_SET_FBUF);` (`:3480`) and then
    /// `if (res)` (`:3481`). The ten payload words (`:3502-3511`) — and the
    /// single `0` of the disable path (`:3527`) — sit *inside* that `if`: a
    /// core that answers `0` is told "Core doesn't support HPS frame buffer"
    /// (`:3535`) and receives no payload at all. `DisableIO()` (`:3539`) runs
    /// either way, as it does here.
    ///
    /// Returns the opcode's reply, so the caller can tell the two cases
    /// apart: non-zero means the payload went out. See
    /// `docs/ARCHITECTURE.md` §5.
    pub fn command_if_supported(&mut self, opcode: u16, payload: &[u16]) -> Result<u16> {
        self.enable_io();
        let reply = self.burst(opcode, payload, true);
        self.disable_io();
        reply
    }

    /// The words of a command, between `EnableIO()` and `DisableIO()`.
    ///
    /// When `gated`, the payload is sent only if the opcode's reply is
    /// non-zero — the `if (res)` of `video.cpp:3481`.
    fn burst(&mut self, opcode: u16, payload: &[u16], gated: bool) -> Result<u16> {
        let reply = self.spi_w(opcode)?;
        if !gated || reply != 0 {
            for &word in payload {
                self.spi_w(word)?;
            }
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// The GPO word `enable_io()` writes from a zero shadow: bit 31 plus
    /// [`SSPI_IO_EN`].
    const EN: u32 = 0x8010_0000;

    /// A [`Regs`] that records every GPO write in order and replays a script
    /// of GPI values, repeating `tail` once the script runs out.
    struct FakeRegs {
        writes: Vec<u32>,
        gpi: Vec<u32>,
        next: Cell<usize>,
        tail: u32,
    }

    impl FakeRegs {
        /// A fake that replays `gpi` and then returns `tail` forever.
        fn new(gpi: &[u32], tail: u32) -> Self {
            Self {
                writes: Vec::new(),
                gpi: gpi.to_vec(),
                next: Cell::new(0),
                tail,
            }
        }

        /// A fake whose fabric acks each of `words` strobes and then clears
        /// the ack, replying `reply` to the first word and 0 to the rest.
        fn acking(words: usize, reply: u16) -> Self {
            let mut gpi = Vec::with_capacity(words * 2);
            for w in 0..words {
                gpi.push(SSPI_ACK);
                gpi.push(if w == 0 { u32::from(reply) } else { 0 });
            }
            Self::new(&gpi, 0)
        }

        /// How many times GPI has been read.
        fn reads(&self) -> usize {
            self.next.get()
        }
    }

    impl Regs for FakeRegs {
        fn gpo_write(&mut self, value: u32) {
            self.writes.push(value);
        }

        fn gpi_read(&self) -> u32 {
            let i = self.next.get();
            self.next.set(i + 1);
            self.gpi.get(i).copied().unwrap_or(self.tail)
        }
    }

    /// Ceiling on `expired()` calls over the whole life of a
    /// [`FakeDeadline`], whatever its budget says. No honest test here polls
    /// more than a handful of times, so this never fires on working code; it
    /// exists so that a defect which lets a poll run away — a window reopened
    /// per read, a check evaluated in the wrong order — fails a read-count
    /// assertion in milliseconds instead of hanging CI.
    const FUSE: u32 = 10_000;

    /// A [`Deadline`] that expires after a fixed number of polls, so a
    /// timeout costs no wall-clock time. A budget of 0 expires immediately;
    /// a budget of *n* lets the poll loop run *n* further times, so a
    /// never-acking fake is read *n* + 1 times before it gives up.
    ///
    /// `starts` counts the windows opened, and is shared with the test so it
    /// can assert that [`Mailbox::wait_ack`] opens exactly one per poll loop
    /// rather than one per GPI read.
    struct FakeDeadline {
        budget: u32,
        left: Cell<u32>,
        starts: Rc<Cell<u32>>,
        polls: Cell<u32>,
    }

    impl FakeDeadline {
        fn new(budget: u32) -> Self {
            Self {
                budget,
                left: Cell::new(budget),
                starts: Rc::new(Cell::new(0)),
                polls: Cell::new(0),
            }
        }

        /// Never expires within a test — up to [`FUSE`] polls.
        fn patient() -> Self {
            Self::new(u32::MAX)
        }

        /// A handle on the window count, still readable after [`Mailbox`] has
        /// taken ownership of the deadline.
        fn starts(&self) -> Rc<Cell<u32>> {
            Rc::clone(&self.starts)
        }
    }

    impl Deadline for FakeDeadline {
        fn start(&mut self) {
            self.starts.set(self.starts.get() + 1);
            self.left.set(self.budget);
        }

        fn expired(&self) -> bool {
            self.polls.set(self.polls.get() + 1);
            if self.polls.get() > FUSE {
                return true;
            }
            let left = self.left.get();
            if left == 0 {
                return true;
            }
            self.left.set(left - 1);
            false
        }
    }

    fn mailbox(regs: FakeRegs, deadline: FakeDeadline) -> Mailbox<FakeRegs, FakeDeadline> {
        Mailbox::new(regs, deadline)
    }

    /// Every error exit must leave the bus idle: enable dropped, strobe low,
    /// bit 31 still set.
    fn assert_bus_released(writes: &[u32]) {
        let last = *writes.last().expect("at least one GPO write");
        assert_eq!(last & SSPI_IO_EN, 0, "enable still set in {last:#010X}");
        assert_eq!(last & SSPI_STROBE, 0, "strobe still set in {last:#010X}");
        assert_eq!(
            last & SSPI_EN_BIT31,
            SSPI_EN_BIT31,
            "bit 31 cleared in {last:#010X}"
        );
    }

    #[test]
    fn enable_sets_bit31_and_disable_keeps_it() {
        // fpga_io.cpp:670 ORs 0x80000000 in before applying the mask, so from
        // a zero shadow EnableIO writes 0x80100000 and the matching DisableIO
        // writes 0x80000000 — bit 31 stays set once anything has enabled.
        let mut mb = mailbox(FakeRegs::new(&[], 0), FakeDeadline::patient());
        mb.enable_io();
        mb.disable_io();
        assert_eq!(mb.regs().writes, vec![EN, SSPI_EN_BIT31]);
        assert_eq!(mb.shadow(), SSPI_EN_BIT31);
    }

    #[test]
    fn command_writes_the_whole_handshake_in_order() {
        let mut mb = mailbox(FakeRegs::acking(2, 0xABCD), FakeDeadline::patient());
        let reply = mb.command(UIO_SET_VIDEO, &[0x1234]).expect("command");

        assert_eq!(reply, 0xABCD);
        assert_eq!(
            mb.regs().writes,
            vec![
                // EnableIO: spi.cpp:45-48 via fpga_io.cpp:668-672.
                0x8010_0000,
                // Opcode 0x20: data low, strobe high, strobe low.
                0x8010_0020,
                0x8012_0020,
                0x8010_0020,
                // Payload 0x1234: the mask at fpga_io.cpp:690 clears the old
                // data word but keeps bit 31 and the enable.
                0x8010_1234,
                0x8012_1234,
                0x8010_1234,
                // DisableIO: the shadow still carries the last data word
                // (fpga_io.cpp:707 wrote it), so only the enable bit changes.
                0x8000_1234,
            ]
        );
        // Two reads per word: one for the ack rising, one for it falling.
        assert_eq!(mb.regs().reads(), 4);
    }

    #[test]
    fn a_new_word_clears_all_sixteen_data_bits_not_just_the_low_ones() {
        // fpga_io.cpp:690 masks with ~(0xFFFF | SSPI_STROBE): the whole
        // 16-bit data field goes before the next word is ORed in. This pair
        // is the one that proves the width, because the second word does not
        // cover the first — 0x0500 is 720p's hact and 0x02D0 its vact
        // (video.cpp:127). A mask a byte narrower would leave bit 10 set and
        // put 0x07D0, vact 2000, on the wire: a black screen with nothing to
        // see from the host side.
        let mut mb = mailbox(FakeRegs::acking(3, 0), FakeDeadline::patient());
        mb.command(UIO_SET_VIDEO, &[0x0500, 0x02D0])
            .expect("command");

        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_0020,
                0x8012_0020,
                0x8010_0020,
                0x8010_0500,
                0x8012_0500,
                0x8010_0500,
                0x8010_02D0,
                0x8012_02D0,
                0x8010_02D0,
                0x8000_02D0,
            ]
        );
    }

    #[test]
    fn reply_is_the_low_16_bits_of_the_final_gpi_read() {
        // The opcode's transfer ends on a read with the ack clear, and only
        // its low 16 bits are the reply (fpga_io.cpp:720). Bits 16..=30 of
        // that read — here bit 18 — are discarded.
        let regs = FakeRegs::new(&[SSPI_ACK, 0x0004_ABCD, SSPI_ACK, 0x0000_5555], 0);
        let mut mb = mailbox(regs, FakeDeadline::patient());
        assert_eq!(
            mb.command(UIO_SET_VIDEO, &[0x1234]).expect("command"),
            0xABCD
        );
    }

    #[test]
    fn poll_loops_wait_for_the_ack_to_rise_and_fall() {
        // Three not-yet-acked reads, then the ack; then two still-acked
        // reads, then the ack clears carrying the reply.
        let regs = FakeRegs::new(&[0, 0, 0, SSPI_ACK, SSPI_ACK, SSPI_ACK, 0x0000_0042], 0);
        let mut mb = mailbox(regs, FakeDeadline::patient());
        assert_eq!(mb.spi_w(0x0007).expect("spi_w"), 0x0042);
        assert_eq!(mb.regs().reads(), 7);
        // No enable was raised, so these are the bare three writes of
        // fpga_spi() from a zero shadow.
        assert_eq!(
            mb.regs().writes,
            vec![0x0000_0007, 0x0002_0007, 0x0000_0007]
        );
    }

    #[test]
    fn bit31_on_the_first_poll_is_no_bitstream_and_never_spins() {
        // fpga_io.cpp:699-704: the C reboots here. One read, then out.
        let mut mb = mailbox(FakeRegs::new(&[GPI_NOT_READY], 0), FakeDeadline::patient());
        let err = mb
            .command(UIO_SET_VIDEO, &[0x1234])
            .expect_err("no bitstream");

        assert!(matches!(err, Error::NoBitstream), "got {err:?}");
        assert_eq!(err.exit_code(), 10);
        assert_eq!(mb.regs().reads(), 1);
        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_0020,
                0x8012_0020,
                0x8010_0020,
                0x8000_0020
            ]
        );
        assert_bus_released(&mb.regs().writes);
    }

    #[test]
    fn bit31_on_the_second_poll_is_no_bitstream_too() {
        // The ack rises, then the fabric drops out while still asserting the
        // ack: the second loop (fpga_io.cpp:709-718) tests bit 31 too, so it
        // aborts rather than waiting for an ack that will never fall. The
        // precedence of the two tests is pinned by the two tests below, not
        // by this one — here the ack is set and the loop wants it clear, so
        // the ack branch would miss in either order.
        let regs = FakeRegs::new(&[SSPI_ACK, GPI_NOT_READY | SSPI_ACK], 0);
        let mut mb = mailbox(regs, FakeDeadline::patient());
        let err = mb
            .command(UIO_SET_VIDEO, &[0x1234])
            .expect_err("no bitstream");

        assert!(matches!(err, Error::NoBitstream), "got {err:?}");
        assert_eq!(mb.regs().reads(), 2);
        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_0020,
                0x8012_0020,
                0x8010_0020,
                0x8000_0020
            ]
        );
        assert_bus_released(&mb.regs().writes);
    }

    #[test]
    fn an_all_ones_gpi_aborts_although_the_ack_bit_is_set() {
        // What a fabric with no bitstream actually presents is every GPI bit
        // driven, so bit 31 and the ack bit (17) arrive in the *same* read.
        // fpga_io.cpp:698-705 reads, tests `if (gpi < 0)` and only then
        // evaluates `while (!(gpi & SSPI_ACK))`, so the abort wins and the C
        // never mistakes that read for an ack. Testing the ack first instead
        // would take this read as a successful rise and carry on into a
        // transfer the fabric is not party to.
        let mut mb = mailbox(FakeRegs::new(&[0xFFFF_FFFF], 0), FakeDeadline::patient());
        let err = mb
            .command(UIO_SET_VIDEO, &[0x1234])
            .expect_err("no bitstream");

        assert!(matches!(err, Error::NoBitstream), "got {err:?}");
        assert_eq!(err.exit_code(), 10);
        assert_eq!(mb.regs().reads(), 1);
        assert_bus_released(&mb.regs().writes);
    }

    #[test]
    fn bit31_beats_an_already_clear_ack_in_the_fall_poll() {
        // The mirror image, for the second loop: the ack rises, then the
        // fabric drops out with the ack already clear, so `gpi < 0` and
        // `!(gpi & SSPI_ACK)` are both true in one read. fpga_io.cpp:711-718
        // tests bit 31 first, so this is an abort — not a transfer that
        // completes and hands the caller a reply of 0 from a dead fabric.
        let regs = FakeRegs::new(&[SSPI_ACK, GPI_NOT_READY], 0);
        let mut mb = mailbox(regs, FakeDeadline::patient());
        let err = mb
            .command(UIO_SET_VIDEO, &[0x1234])
            .expect_err("no bitstream");

        assert!(matches!(err, Error::NoBitstream), "got {err:?}");
        assert_eq!(mb.regs().reads(), 2);
        assert_bus_released(&mb.regs().writes);
    }

    #[test]
    fn a_fabric_that_never_acks_times_out_with_the_bus_released() {
        // A budget of 3 makes the rise poll really loop: three reads inside
        // the window, one more after it closes. The counts are the point —
        // they pin the window to one per poll loop (fpga_io.cpp:696-705 is
        // one `do`), so a deadline restarted on every read, which no zero
        // budget could tell apart, fails here instead of spinning forever on
        // a fabric that has stopped acking.
        let deadline = FakeDeadline::new(3);
        let starts = deadline.starts();
        let mut mb = mailbox(FakeRegs::new(&[], 0), deadline);
        let err = mb.command(UIO_SET_VIDEO, &[0x1234]).expect_err("timeout");

        assert!(matches!(err, Error::Timeout), "got {err:?}");
        assert_eq!(err.exit_code(), 11);
        assert_eq!(mb.regs().reads(), 4);
        assert_eq!(starts.get(), 1, "one window per poll loop, not per read");
        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_0020,
                0x8012_0020,
                0x8010_0020,
                0x8000_0020
            ]
        );
        assert_bus_released(&mb.regs().writes);
    }

    #[test]
    fn an_ack_that_never_clears_times_out_with_the_bus_released() {
        // The second poll loop is bounded as well, and loops for real: the
        // first read satisfies the rise, then a budget of 2 gives two more
        // reads plus the one after the window closes. Two windows, one per
        // `do` (fpga_io.cpp:696-705 and :709-718). The strobe was already
        // lowered by :707, so no extra write appears on this error path.
        let deadline = FakeDeadline::new(2);
        let starts = deadline.starts();
        let mut mb = mailbox(FakeRegs::new(&[], SSPI_ACK), deadline);
        let err = mb.command(UIO_SET_VIDEO, &[]).expect_err("timeout");

        assert!(matches!(err, Error::Timeout), "got {err:?}");
        assert_eq!(mb.regs().reads(), 4);
        assert_eq!(starts.get(), 2, "one window per poll loop, not per read");
        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_0020,
                0x8012_0020,
                0x8010_0020,
                0x8000_0020
            ]
        );
        assert_bus_released(&mb.regs().writes);
    }

    #[test]
    fn a_failed_payload_word_still_releases_the_bus() {
        // The opcode goes through; the fabric stops acking mid-burst.
        let regs = FakeRegs::new(&[SSPI_ACK, 0x0000_ABCD], 0);
        let mut mb = mailbox(regs, FakeDeadline::new(0));
        let err = mb
            .command(UIO_SET_VIDEO, &[0x1111, 0x2222])
            .expect_err("timeout");

        assert!(matches!(err, Error::Timeout), "got {err:?}");
        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_0020,
                0x8012_0020,
                0x8010_0020,
                0x8010_1111,
                0x8012_1111,
                0x8010_1111,
                0x8000_1111,
            ]
        );
        assert_bus_released(&mb.regs().writes);
        // The third word was never attempted.
        assert_eq!(mb.regs().writes.len(), 8);
    }

    #[test]
    fn the_shadow_carries_the_enable_and_last_word_into_the_next_command() {
        // fpga_io.cpp:513 keeps gpo_copy across calls, so the second
        // EnableIO writes the first command's last data word back out.
        let mut mb = mailbox(FakeRegs::acking(4, 0), FakeDeadline::patient());
        mb.command(UIO_SET_FBUF, &[0x00FF]).expect("first command");
        let before = mb.regs().writes.len();
        mb.command(UIO_LEDS, &[]).expect("second command");

        assert_eq!(mb.regs().writes[before], 0x8010_00FF);
        assert_eq!(
            mb.regs().writes[before + 1..],
            [0x8010_0025, 0x8012_0025, 0x8010_0025, 0x8000_0025]
        );
    }

    #[test]
    fn a_gated_command_sends_its_payload_when_the_core_answers() {
        // video.cpp:3480-3481: `int res = spi_uio_cmd_cont(UIO_SET_FBUF);
        // if (res)`. A non-zero reply means the core has the HPS frame
        // buffer, and the payload words of :3502-3511 go out.
        let mut mb = mailbox(FakeRegs::acking(3, 0x0001), FakeDeadline::patient());
        let reply = mb
            .command_if_supported(UIO_SET_FBUF, &[0x8016, 0x1000])
            .expect("command");

        assert_eq!(reply, 0x0001);
        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_002F,
                0x8012_002F,
                0x8010_002F,
                0x8010_8016,
                0x8012_8016,
                0x8010_8016,
                0x8010_1000,
                0x8012_1000,
                0x8010_1000,
                0x8000_1000,
            ]
        );
    }

    #[test]
    fn a_gated_command_sends_no_payload_when_the_core_answers_zero() {
        // The `else` of video.cpp:3533-3537: "Core doesn't support HPS frame
        // buffer", no `spi_w()` at all, and `DisableIO()` at :3539 all the
        // same. The reply reaches the caller so it can say which happened.
        let mut mb = mailbox(FakeRegs::acking(1, 0), FakeDeadline::patient());
        let reply = mb
            .command_if_supported(UIO_SET_FBUF, &[0x8016, 0x1000])
            .expect("command");

        assert_eq!(reply, 0);
        assert_eq!(mb.regs().reads(), 2);
        assert_eq!(
            mb.regs().writes,
            vec![
                0x8010_0000,
                0x8010_002F,
                0x8012_002F,
                0x8010_002F,
                0x8000_002F
            ]
        );
        assert_bus_released(&mb.regs().writes);
    }

    #[test]
    fn an_ungated_command_sends_its_payload_whatever_the_reply_is() {
        // set_video() discards the reply to its opcode (video.cpp:2266) and
        // sends all 26 words regardless, so `command` must not inherit the
        // gate that `command_if_supported` implements.
        let mut mb = mailbox(FakeRegs::acking(2, 0), FakeDeadline::patient());
        let reply = mb.command(UIO_SET_VIDEO, &[0x1234]).expect("command");

        assert_eq!(reply, 0);
        assert!(mb.regs().writes.contains(&0x8012_1234), "payload not sent");
    }

    #[test]
    fn opcodes_match_user_io_h() {
        assert_eq!(UIO_SET_VIDEO, 0x20);
        assert_eq!(UIO_LEDS, 0x25);
        assert_eq!(UIO_SET_FBUF, 0x2F);
    }

    #[test]
    fn bit_positions_match_the_c() {
        assert_eq!(SSPI_STROBE, 0x0002_0000);
        assert_eq!(SSPI_ACK, SSPI_STROBE);
        assert_eq!(SSPI_FPGA_EN, 0x0004_0000);
        assert_eq!(SSPI_OSD_EN, 0x0008_0000);
        assert_eq!(SSPI_IO_EN, 0x0010_0000);
        assert_eq!(GPI_NOT_READY, 0x8000_0000);
        assert_eq!(FPGA_MANAGER_BASE + GPO_OFFSET, 0xFF70_6010);
        assert_eq!(FPGA_MANAGER_BASE + GPI_OFFSET, 0xFF70_6014);
    }

    #[test]
    fn the_monotonic_deadline_expires_on_its_limit() {
        // No sleeping: a zero-length window is closed the moment it opens,
        // and an hour-long one is not.
        let mut zero = MonotonicDeadline::new(Duration::ZERO);
        zero.start();
        assert!(zero.expired());

        let mut hour = MonotonicDeadline::new(Duration::from_secs(3600));
        hour.start();
        assert!(!hour.expired());

        assert_eq!(MonotonicDeadline::default().limit, ACK_TIMEOUT);
    }
}
