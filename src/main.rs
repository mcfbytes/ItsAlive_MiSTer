//! Command line entry point: `docs/ARCHITECTURE.md` §7, and nothing else.
//!
//! This file is three things stacked on top of each other, in the order a
//! reviewer should read them:
//!
//! 1. **[`parse`]**, which turns `argv` into a [`Command`] and is pure. Every
//!    usage error is decided here, before a single device node is opened, so
//!    the argument tests below can run the whole of it on a build host with no
//!    `/dev/mem` and no i2c adapter in sight.
//! 2. **The sequences** — [`hdmi_on`], [`hdmi_off`], [`fb_enable`],
//!    [`fb_disable`], [`up`] — which are generic over the four traits
//!    [`crate::hw`] publishes ([`Regs`], [`I2cBus`], [`ModeSink`],
//!    [`TextSink`]). They contain the ordering that `docs/ARCHITECTURE.md`
//!    §1's table fixes and nothing about how a file is opened, which is what
//!    lets the end-to-end test drive them with recording fakes and assert one
//!    ordered list of events across all three channels.
//! 3. **[`execute`]**, the only place that names a real device, and [`main`],
//!    which is a [`Result`] to exit-code adapter and no more.
//!
//! # Exit codes, and why nothing here may panic
//!
//! Every path out of [`main`] goes through [`crate::Error::exit_code`]
//! (§7's table). That is only true if nothing on the way can `panic!`:
//! `Cargo.toml` sets `panic = "abort"` for the release profile, so a panic on
//! the board is a `SIGABRT` whose wait status is not one of those codes, and
//! the installer's `|| true` would hide it. So there is no `unwrap`, no
//! `expect` and no indexing that can run off the end outside `#[cfg(test)]`,
//! and — the trap this crate has already been bitten by — **no `println!` or
//! `eprintln!`**, both of which panic when the write fails. Diagnostics go
//! through [`hw::log`] and `probe`'s findings through [`out`], which drop the
//! error instead (see [`hw::log`]'s doc for the whole argument).

use itsalive::hw::{I2cBus, ModeSink};
use itsalive::mailbox::{Deadline, Mailbox, MonotonicDeadline, Regs};
use itsalive::video::Modeline;
use itsalive::{Error, Result, adv7513, fb, hw, mailbox, say, video};
use std::fmt::{self, Write as _};
use std::io::{self, Write as _};
use std::process::ExitCode;

/// What `--help`, and any usage error, puts in front of the caller.
///
/// The subcommand list is `docs/ARCHITECTURE.md` §7's, minus `leds`, which is
/// v1.1 and not implemented: a usage message that advertises a subcommand the
/// binary does not have is worse than no usage message.
const USAGE: &str = "\
itsalive - bring up HDMI and the Linux framebuffer on a MiSTer

usage:
  itsalive probe [--json]              report what is there, exit with the first failure
  itsalive hdmi [--mode MODE] [--off]  configure the ADV7513 and the video PLL
  itsalive fb enable [--mode MODE]     point the fabric's frame reader at /dev/fb0
  itsalive fb disable                  hand the display back to the core
  itsalive say [--clear] TEXT...       write TEXT to /dev/tty1 for fbcon to paint
  itsalive up [--mode MODE]            hdmi, then fb enable: the installer's one call
  itsalive --help                      this text

MODE is 720p (the default) or 480p.

exit codes:
  0   done                      11  mailbox timeout
  2   usage error               12  ADV7513 not found on any i2c bus
  10  no bitstream in the       13  ADV7513 on more than one i2c bus
      fabric (GPI bit 31)       14  /dev/mem, /dev/i2c-*, sysfs or tty unopenable";

fn main() -> ExitCode {
    // `std::env::args()` panics on an argument that is not UTF-8, and a panic
    // here would replace a §7 exit code with a SIGABRT. `args_os` plus a lossy
    // conversion cannot: a mangled flag simply fails to match and becomes a
    // usage error, and mangled `say` text is written as the replacement
    // character, which is what a console would have shown anyway.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            hw::log(format_args!("itsalive: {err}"));
            if matches!(err, Error::Usage(_)) {
                hw::log(format_args!("{USAGE}"));
            }
            // Unreachable fallback: every code in §7's table is 0..=14, so the
            // conversion from `exit_code()`'s `i32` cannot fail. 2 is the
            // "bug in the caller" code, which is what a new out-of-range
            // variant would be.
            ExitCode::from(u8::try_from(err.exit_code()).unwrap_or(2))
        }
    }
}

/// Parse, then do. Split out of [`main`] so that the argument tests can call
/// it without an [`ExitCode`] in the way.
fn run(args: &[String]) -> Result<()> {
    match parse(args)? {
        Command::Help => {
            out(format_args!("{USAGE}"));
            Ok(())
        }
        cmd => execute(cmd),
    }
}

/// Put one line on **stdout**, and survive a stdout that will not take it.
///
/// [`hw::log`]'s argument applies verbatim to `println!`: it ends in
/// `panic!("failed printing to stdout: {e}")`, which `EPIPE` reaches the
/// moment `itsalive probe | head -1` closes the pipe. stdout is `probe`'s
/// output and nothing else's; everything a human reads goes to stderr through
/// [`hw::log`], so that `--json` can be redirected on its own.
fn out(args: fmt::Arguments<'_>) {
    let _ = writeln!(io::stdout().lock(), "{args}");
}

/// Shorthand for the one error variant this file raises on its own.
fn usage(msg: impl Into<String>) -> Error {
    Error::Usage(msg.into())
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// One parsed command line: everything [`execute`] needs and nothing it has
/// to look up again.
///
/// The mode is resolved to a [`Modeline`] here rather than carried as a name,
/// so that an unknown `--mode` is a usage error (exit 2) decided before any
/// device is opened, which is the difference between "bug in the caller" and
/// a half-configured chip.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    /// `probe [--json]`
    Probe { json: bool },
    /// `hdmi [--mode MODE] [--off]`
    Hdmi { mode: Modeline, off: bool },
    /// `fb enable [--mode MODE]`
    FbEnable { mode: Modeline },
    /// `fb disable`
    FbDisable,
    /// `say [--clear] TEXT...`
    Say { clear: bool, text: Vec<String> },
    /// `up [--mode MODE]`
    Up { mode: Modeline },
    /// `--help`
    Help,
}

/// `argv[1..]` to a [`Command`], or [`Error::Usage`] (exit 2).
///
/// Hand-rolled, because `docs/ARCHITECTURE.md` §6 allows exactly one
/// dependency and it is `libc`. The grammar is small enough that a parser
/// combinator would be more code than the five `match` arms below.
///
/// **`--mode` is accepted wherever a mode is meaningful and rejected where it
/// is not**, per subcommand rather than per flag combination. `hdmi --off
/// --mode 720p` therefore parses and ignores the mode: the power-down is one
/// register write that no timing takes part in, and an installer that passes
/// `--mode` uniformly to every `hdmi` call must not be failed for it
/// (§8: nothing there may fail an install). `fb disable --mode 720p` does not
/// parse, because `fb disable` never takes one.
fn parse(args: &[String]) -> Result<Command> {
    let Some((first, rest)) = args.split_first() else {
        return Err(usage("no subcommand"));
    };

    match first.as_str() {
        "--help" | "-h" | "help" => {
            if let Some(extra) = rest.first() {
                return Err(usage(format!("--help takes no arguments, got {extra:?}")));
            }
            Ok(Command::Help)
        }

        "probe" => {
            let mut json = false;
            for arg in rest {
                match arg.as_str() {
                    "--json" => json = true,
                    other => return Err(usage(format!("probe: unexpected argument {other:?}"))),
                }
            }
            Ok(Command::Probe { json })
        }

        "hdmi" => {
            let mut mode = video::MODE_720P;
            let mut off = false;
            let mut it = rest.iter();
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "--mode" => mode = mode_value(&mut it)?,
                    "--off" => off = true,
                    other => return Err(usage(format!("hdmi: unexpected argument {other:?}"))),
                }
            }
            Ok(Command::Hdmi { mode, off })
        }

        "fb" => {
            let Some((what, rest)) = rest.split_first() else {
                return Err(usage("fb: expected `enable` or `disable`"));
            };
            match what.as_str() {
                "enable" => {
                    let mut mode = video::MODE_720P;
                    let mut it = rest.iter();
                    while let Some(arg) = it.next() {
                        match arg.as_str() {
                            "--mode" => mode = mode_value(&mut it)?,
                            other => {
                                return Err(usage(format!(
                                    "fb enable: unexpected argument {other:?}"
                                )));
                            }
                        }
                    }
                    Ok(Command::FbEnable { mode })
                }
                "disable" => {
                    if let Some(extra) = rest.first() {
                        return Err(usage(format!("fb disable: unexpected argument {extra:?}")));
                    }
                    Ok(Command::FbDisable)
                }
                other => Err(usage(format!(
                    "fb: expected `enable` or `disable`, got {other:?}"
                ))),
            }
        }

        "say" => {
            // Options first, then everything else verbatim. Once the text has
            // started a leading `-` is part of what the caller wants on the
            // screen, and `--` ends the options explicitly so that
            // `itsalive say -- --clear` can say "--clear".
            let mut clear = false;
            let mut start = 0usize;
            while let Some(arg) = rest.get(start) {
                match arg.as_str() {
                    "--clear" => {
                        clear = true;
                        start = start.saturating_add(1);
                    }
                    "--" => {
                        start = start.saturating_add(1);
                        break;
                    }
                    other if other.starts_with("--") => {
                        return Err(usage(format!("say: unknown option {other:?}")));
                    }
                    _ => break,
                }
            }
            let text: Vec<String> = rest.get(start..).unwrap_or(&[]).to_vec();
            if text.is_empty() && !clear {
                return Err(usage("say: nothing to say"));
            }
            Ok(Command::Say { clear, text })
        }

        "up" => {
            let mut mode = video::MODE_720P;
            let mut it = rest.iter();
            while let Some(arg) = it.next() {
                match arg.as_str() {
                    "--mode" => mode = mode_value(&mut it)?,
                    other => return Err(usage(format!("up: unexpected argument {other:?}"))),
                }
            }
            Ok(Command::Up { mode })
        }

        other => Err(usage(format!("unknown subcommand {other:?}"))),
    }
}

/// The argument after `--mode`, looked up in [`video::MODES`].
fn mode_value(it: &mut std::slice::Iter<'_, String>) -> Result<Modeline> {
    let Some(name) = it.next() else {
        return Err(usage(format!("--mode needs a value: {}", mode_names())));
    };
    video::mode_by_name(name)
        .ok_or_else(|| usage(format!("unknown mode {name:?}: expected {}", mode_names())))
}

/// `"720p or 480p"`, built from the table so the message cannot drift from it.
fn mode_names() -> String {
    let names: Vec<&str> = video::MODES.iter().map(|(name, _)| *name).collect();
    names.join(" or ")
}

// ---------------------------------------------------------------------------
// The sequences
// ---------------------------------------------------------------------------

/// Is there a bitstream in the fabric?
///
/// `is_fpga_ready(1)` is `return (fpga_gpi_read() >= 0);`
/// (`fpga_io.cpp:655-662`): one read of GPI, no transfer, no side effect, and
/// bit 31 clear means the FPGA is in user mode.
fn fabric_ready<R: Regs>(regs: &R) -> bool {
    regs.gpi_read() & mailbox::GPI_NOT_READY == 0
}

/// Refuse early when the fabric is unconfigured.
///
/// **This is a deliberate addition to Main_MiSTer's order.** The C never asks:
/// it discovers an unconfigured fabric inside `fpga_spi()` (`fpga_io.cpp:699`)
/// and reboots the board. We would discover it in the same place — the first
/// `spi_w` of `UIO_SET_VIDEO` — which is *after* 92 i2c writes have gone out.
/// On a board with no bitstream those 92 writes reach nothing at all, because
/// the HPS i2c peripheral is routed to the chip through the fabric
/// (`docs/ARCHITECTURE.md` §1): every one of them would NAK, print a line, and
/// still end in exit 10. Asking first costs one register read and turns that
/// into an immediate, quiet [`Error::NoBitstream`].
fn require_bitstream<R: Regs, D: Deadline>(mb: &Mailbox<R, D>) -> Result<()> {
    if fabric_ready(mb.regs()) {
        Ok(())
    } else {
        Err(Error::NoBitstream)
    }
}

/// The PLL block for a mode's pixel clock.
///
/// [`video::solve_pll`] returns `None` only for a clock no divider can reach
/// (`docs/ARCHITECTURE.md` §3), which neither preset is — both are golden
/// tested against the C. It is a usage error rather than a hardware one
/// because the only way to arrive here is a mode this tool cannot program,
/// which §7 calls "a bug in the caller".
fn solve(mode: &Modeline) -> Result<video::PllBlock> {
    video::solve_pll(mode.f_pix_mhz).ok_or_else(|| {
        usage(format!(
            "no PLL solution for {} MHz: not a clock this tool can program",
            mode.f_pix_mhz
        ))
    })
}

/// Steps 1 and 2 of `docs/ARCHITECTURE.md` §1's table, in that order.
///
/// The ADV7513's bulk configuration over i2c (`video.cpp:1606-1613`), then
/// `UIO_SET_VIDEO` with the timings and the PLL block (`video.cpp:2266-2294`),
/// then the chip's three mode registers (`video.cpp:2296`, whose body is
/// `:1691-1727`). The mode registers come **after** the mailbox burst because
/// that is where `set_video()` calls `hdmi_config_set_mode()`, and because
/// `0x17`'s sync-invert bits are what correct the polarity the fabric was just
/// told to drive (§3, §4).
///
/// Idempotent (§7): every step is an unconditional write of a fixed value, so
/// a second `hdmi` writes the same 95 registers and the same 27 words again.
fn hdmi_on<R, D, B>(mb: &mut Mailbox<R, D>, bus: &mut B, mode: &Modeline) -> Result<()>
where
    R: Regs,
    D: Deadline,
    B: I2cBus + ?Sized,
{
    require_bitstream(mb)?;
    let pll = solve(mode)?;

    // Step 1: INIT, then AUDIO, then CSC (`adv7513::writes`).
    let total = adv7513::writes().count();
    let refused = hw::write_table(bus, adv7513::writes());
    hw::log(format_args!(
        "itsalive: adv7513: {total} registers written, {refused} refused"
    ));

    // Step 2a: the 26-word burst.
    mb.command(mailbox::UIO_SET_VIDEO, &video::set_video_words(mode, &pll))?;
    hw::log(format_args!(
        "itsalive: video: {}x{} @ {} MHz, vic {}, PLL C={} M={} K={:#X}",
        mode.hact, mode.vact, mode.f_pix_mhz, mode.vic, pll.c, pll.m, pll.k
    ));

    // Step 2b: 0x17, 0x3B, 0x3C.
    let refused = hw::write_table(
        bus,
        adv7513::mode_regs(mode.hpol, mode.vpol, mode.vic, mode.pr),
    );
    hw::log(format_args!(
        "itsalive: adv7513: 3 mode registers written, {refused} refused"
    ));
    Ok(())
}

/// `hdmi --off`: `0x41 = 0x50` and nothing else (§4, `video.cpp:2737-2745`).
///
/// No mailbox, no PLL, no bulk table — so this path never opens `/dev/mem`
/// either. Through [`hw::write_table`] like every other i2c write here, so a
/// NAK is logged and tolerated rather than turned into exit 14.
fn hdmi_off<B: I2cBus + ?Sized>(bus: &mut B) -> Result<()> {
    let (reg, value) = adv7513::POWER_DOWN;
    let refused = hw::write_table(bus, [adv7513::POWER_DOWN]);
    hw::log(format_args!(
        "itsalive: adv7513: power down ({reg:#04X} = {value:#04X}), {refused} refused"
    ));
    Ok(())
}

/// Step 3 of §1's table: `UIO_SET_FBUF`, then the kernel's geometry knob.
///
/// **Fabric first, sysfs second** (§5, `video.cpp:3502-3517`), and the payload
/// is gated on the reply to the opcode: `video_fb_enable()` puts its ten words
/// inside `if (res)` (`video.cpp:3480-3481`), so a core with no HPS frame
/// buffer gets the opcode and nothing else. [`Mailbox::command_if_supported`]
/// is that gate, and it returns the reply so this function can tell the two
/// cases apart.
///
/// When the core answers `0` there is no frame buffer to switch to, so the
/// sysfs line is **not** written: re-registering `/dev/fb0` at a geometry
/// nothing is scanning out would be exactly the "pretending it worked" that
/// leaves a rig log looking green and a monitor looking black. The C prints
/// "Core doesn't support HPS frame buffer" (`video.cpp:3535`) and carries on;
/// so do we, and the exit code stays 0 because §7's table has no code for it —
/// see the note in `docs/ARCHITECTURE.md` §5.
fn fb_enable<R, D, M>(mb: &mut Mailbox<R, D>, sink: &mut M, mode: &Modeline) -> Result<()>
where
    R: Regs,
    D: Deadline,
    M: ModeSink + ?Sized,
{
    require_bitstream(mb)?;

    // T1.4's note: the bridge between a `Modeline` and the framebuffer's own
    // geometry is `for_mode`, and this is the only place it is crossed.
    let geom = fb::FbGeometry::for_mode(mode.hact, mode.vact, mode.pr != 0);

    let reply = mb.command_if_supported(mailbox::UIO_SET_FBUF, &fb::enable_words(&geom))?;
    if reply == 0 {
        hw::log(format_args!(
            "itsalive: core doesn't support HPS frame buffer; /dev/fb0 not switched in"
        ));
        return Ok(());
    }

    let line = fb::mode_param_line(&geom);
    sink.write_mode_line(&line)?;
    hw::log(format_args!(
        "itsalive: frame buffer: {}x{}, stride {} bytes",
        geom.width(),
        geom.height(),
        u32::from(geom.width()) * 4
    ));
    Ok(())
}

/// `fb disable`: the opcode and a single `0` word (`video.cpp:3527`).
///
/// Gated on the same reply as the enable path, because the C's `spi_w(0)` sits
/// inside the same `if (res)`. No sysfs write: `fb_write_module_params()` is
/// only called on the enable side (`video.cpp:3516`), and the knob's geometry
/// is still the right one for whenever the frame buffer comes back.
fn fb_disable<R, D>(mb: &mut Mailbox<R, D>) -> Result<()>
where
    R: Regs,
    D: Deadline,
{
    require_bitstream(mb)?;
    let reply = mb.command_if_supported(mailbox::UIO_SET_FBUF, &fb::disable_words())?;
    if reply == 0 {
        hw::log(format_args!(
            "itsalive: core doesn't support HPS frame buffer; nothing to disable"
        ));
        return Ok(());
    }
    hw::log(format_args!("itsalive: display handed back to the core"));
    Ok(())
}

/// `up`: [`hdmi_on`] then [`fb_enable`], which is §1's three steps in order.
///
/// The installer's one call (§8). It takes one mailbox and one i2c handle and
/// hands the same two to both halves, so the bus is discovered once and
/// `/dev/mem` is mapped once; re-probing between the halves would double the
/// window in which a second writer could appear and would make the failure
/// modes of `up` different from the failure modes of its two parts.
fn up<R, D, B, M>(mb: &mut Mailbox<R, D>, bus: &mut B, sink: &mut M, mode: &Modeline) -> Result<()>
where
    R: Regs,
    D: Deadline,
    B: I2cBus + ?Sized,
    M: ModeSink + ?Sized,
{
    hdmi_on(mb, bus, mode)?;
    fb_enable(mb, sink, mode)
}

// ---------------------------------------------------------------------------
// probe
// ---------------------------------------------------------------------------

/// One line of `probe`'s output: a question it asked and the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Finding {
    /// The short name, stable enough for a rig log to grep for.
    name: &'static str,
    /// Did this check pass?
    ok: bool,
    /// The human-readable answer, or the error's message.
    detail: String,
    /// The §7 exit code this finding would produce, or 0 when it passed.
    code: i32,
}

impl Finding {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            ok: true,
            detail: detail.into(),
            code: 0,
        }
    }

    fn failed(name: &'static str, err: &Error) -> Self {
        Self {
            name,
            ok: false,
            detail: err.to_string(),
            code: err.exit_code(),
        }
    }
}

/// Ask the three questions `probe` answers, in the order it answers them.
///
/// Returns the findings and the first failure, because §7 says `probe`
/// "prints one line per finding and exits with the first failing code": all
/// three questions are always asked, so the installer sees the whole picture,
/// and the exit code is the first thing that went wrong.
///
/// **Nothing here writes anything.** The bitstream question is one GPI read
/// ([`fabric_ready`]) rather than a mailbox transfer, so `probe` cannot
/// disturb a core, cannot time out, and is safe to run twice; that is also
/// why exit 11 is not among the codes it can produce. The i2c question opens
/// the bus and closes it again, which is the same receive-byte probe
/// `i2c_open()` performs (`smbus.cpp:250-257`). The sysfs question is a
/// `stat`.
fn probe_findings() -> (Vec<Finding>, Option<Error>) {
    let mut findings = Vec::new();
    let mut first: Option<Error> = None;
    let mut record = |finding: Finding, err: Option<Error>| {
        findings.push(finding);
        if let Some(err) = err
            && first.is_none()
        {
            first = Some(err);
        }
    };

    // 1. Is there a bitstream in the fabric? (§2's GPI bit 31.)
    match hw::MemRegs::open() {
        Ok(regs) => {
            let gpi = regs.gpi_read();
            if fabric_ready(&regs) {
                record(
                    Finding::ok(
                        "bitstream",
                        format!("fabric in user mode (GPI {gpi:#010X})"),
                    ),
                    None,
                );
            } else {
                record(
                    Finding::failed("bitstream", &Error::NoBitstream),
                    Some(Error::NoBitstream),
                );
            }
        }
        Err(err) => record(Finding::failed("bitstream", &err), Some(err)),
    }

    // 2. Which i2c bus is the ADV7513 on? (§4's discovery and its two
    //    refusals, exit 12 and exit 13.)
    match hw::I2c::open_adv7513() {
        Ok(i2c) => {
            let detail = format!(
                "ADV7513 at {:#04X} on {}",
                i2c.addr(),
                hw::bus_path(i2c.bus())
            );
            // `i2c` drops at the end of this arm, which closes the bus:
            // `probe` leaves nothing open behind it.
            record(Finding::ok("adv7513", detail), None);
        }
        Err(err) => record(Finding::failed("adv7513", &err), Some(err)),
    }

    // 3. Is the kernel's geometry knob there? Without it `fb enable` reaches
    //    the fabric and then fails at the sysfs write with exit 14, which is
    //    exactly what this reports in advance.
    match std::fs::metadata(hw::FB_MODE_PATH) {
        Ok(_) => record(
            Finding::ok("fb_mode", format!("{} present", hw::FB_MODE_PATH)),
            None,
        ),
        Err(e) => {
            let err = Error::io(format!("stat {}", hw::FB_MODE_PATH), e);
            record(Finding::failed("fb_mode", &err), Some(err));
        }
    }

    (findings, first)
}

/// `probe`'s plain output: one line per finding.
fn render_lines(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| {
            let status = if f.ok { "ok  " } else { "FAIL" };
            format!("{status} {}: {}", f.name, f.detail)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `probe --json`: one object for the rig log.
///
/// Hand-rolled, because `libc` is the only dependency §6 allows and serde is
/// not it. Every string goes through [`json_string`]; the numbers are `i32`
/// and `bool`s are `bool`s, so there is nothing else to escape.
fn render_json(findings: &[Finding], code: i32) -> String {
    let items: Vec<String> = findings
        .iter()
        .map(|f| {
            format!(
                "{{\"name\":{},\"ok\":{},\"detail\":{},\"exit_code\":{}}}",
                json_string(f.name),
                f.ok,
                json_string(&f.detail),
                f.code
            )
        })
        .collect();
    format!(
        "{{\"findings\":[{}],\"exit_code\":{code}}}",
        items.join(",")
    )
}

/// One JSON string literal, quotes included, escaped per RFC 8259 §7.
///
/// The two mandatory escapes are `"` and `\`; every code point below `0x20`
/// must also be escaped, and the five with short forms get them. Everything
/// else, including non-ASCII, is emitted as UTF-8, which is what the RFC's
/// default encoding is. This matters because a finding's detail is an
/// [`Error`] message, which can carry `strerror` output in the caller's
/// locale and a device path chosen by the caller.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                // `write!` to a String cannot fail; the result is dropped
                // rather than unwrapped because `unwrap` is banned here.
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `probe`: ask, print, and exit with the first failing code.
fn cmd_probe(json: bool) -> Result<()> {
    let (findings, first) = probe_findings();
    let code = first.as_ref().map_or(0, Error::exit_code);
    if json {
        out(format_args!("{}", render_json(&findings, code)));
    } else {
        out(format_args!("{}", render_lines(&findings)));
    }
    match first {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// The only place that names a real device
// ---------------------------------------------------------------------------

/// `/dev/mem` mapped at the FPGA manager, behind the real deadline.
fn open_mailbox() -> Result<Mailbox<hw::MemRegs, MonotonicDeadline>> {
    Ok(Mailbox::new(
        hw::MemRegs::open()?,
        MonotonicDeadline::default(),
    ))
}

/// Open what the command needs, in the order §1's table uses it, and run it.
fn execute(cmd: Command) -> Result<()> {
    match cmd {
        Command::Help => {
            out(format_args!("{USAGE}"));
            Ok(())
        }

        Command::Probe { json } => cmd_probe(json),

        // `--off` is one i2c write: no mailbox, so no `/dev/mem`.
        Command::Hdmi { off: true, .. } => {
            let mut bus = hw::I2c::open_adv7513()?;
            hdmi_off(&mut bus)
        }

        Command::Hdmi { mode, off: false } => {
            let mut mb = open_mailbox()?;
            let mut bus = hw::I2c::open_adv7513()?;
            hdmi_on(&mut mb, &mut bus, &mode)
        }

        Command::FbEnable { mode } => {
            let mut mb = open_mailbox()?;
            let mut sink = hw::ModeFile::new();
            fb_enable(&mut mb, &mut sink, &mode)
        }

        Command::FbDisable => {
            let mut mb = open_mailbox()?;
            fb_disable(&mut mb)
        }

        Command::Say { clear, text } => {
            let mut tty = hw::Tty::open()?;
            say::say(&mut tty, clear, &text)
        }

        // One process, one mailbox, one i2c handle (§7's idempotence rule
        // plus §8's "the installer's one call").
        Command::Up { mode } => {
            let mut mb = open_mailbox()?;
            let mut bus = hw::I2c::open_adv7513()?;
            let mut sink = hw::ModeFile::new();
            up(&mut mb, &mut bus, &mut sink, &mode)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsalive::hw::TextSink;
    use itsalive::mailbox::{GPI_NOT_READY, SSPI_ACK, SSPI_IO_EN, SSPI_STROBE};
    use std::cell::RefCell;
    use std::rc::Rc;

    // -----------------------------------------------------------------------
    // One ordered list of events across all three channels
    // -----------------------------------------------------------------------

    /// Everything the tool can do to the world, in the order it did it.
    ///
    /// The point of a single enum and a single shared log is that the
    /// assertion below can see *across* the channels. Three separate recording
    /// fakes, each with its own `Vec`, would pass unchanged if the i2c mode
    /// registers were written before the `UIO_SET_VIDEO` burst instead of
    /// after it, or if the sysfs line went out before the fabric was told
    /// anything — which are precisely the two orderings
    /// `docs/ARCHITECTURE.md` §1 and §5 fix.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Event {
        /// One SMBus write-byte-data: `(reg, value)`.
        I2c(u8, u8),
        /// `SSPI_IO_EN` rose (`EnableIO()`).
        EnableIo,
        /// `SSPI_IO_EN` fell (`DisableIO()`).
        DisableIo,
        /// One 16-bit word went out on the mailbox (the strobe rose over it).
        Word(u16),
        /// One line was written to the sysfs geometry knob.
        ModeLine(String),
        /// Bytes were written to the tty.
        Text(Vec<u8>),
    }

    type Log = Rc<RefCell<Vec<Event>>>;

    fn log_new() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    fn events(log: &Log) -> Vec<Event> {
        log.borrow().clone()
    }

    /// A [`Regs`] that decodes the GPO strobe protocol back into words.
    ///
    /// `gpi_read` mirrors the strobe into the ack bit, which is what a healthy
    /// fabric does (`fpga_io.cpp:696-718` polls for exactly that), and puts
    /// `reply` in the low 16 bits, which is what `spi_w` returns. `ready`
    /// drives GPI bit 31 and `acks` turns the fabric mute so the deadline can
    /// be exercised.
    struct FakeRegs {
        log: Log,
        gpo: u32,
        reply: u16,
        ready: bool,
        acks: bool,
    }

    impl FakeRegs {
        fn new(log: &Log, reply: u16) -> Self {
            Self {
                log: Rc::clone(log),
                gpo: 0,
                reply,
                ready: true,
                acks: true,
            }
        }

        fn push(&self, event: Event) {
            self.log.borrow_mut().push(event);
        }
    }

    impl Regs for FakeRegs {
        fn gpo_write(&mut self, value: u32) {
            let was = self.gpo;
            self.gpo = value;
            let rose = |bit: u32| value & bit != 0 && was & bit == 0;
            let fell = |bit: u32| value & bit == 0 && was & bit != 0;
            if rose(SSPI_IO_EN) {
                self.push(Event::EnableIo);
            }
            if fell(SSPI_IO_EN) {
                self.push(Event::DisableIo);
            }
            // The data field is only meaningful when the strobe goes up over
            // it; the other two writes of `fpga_spi()` carry the same word
            // with the strobe low and are the framing, not the message.
            if rose(SSPI_STROBE) {
                self.push(Event::Word((value & 0xFFFF) as u16));
            }
        }

        fn gpi_read(&self) -> u32 {
            let mut gpi = u32::from(self.reply);
            if self.acks && self.gpo & SSPI_STROBE != 0 {
                gpi |= SSPI_ACK;
            }
            if !self.ready {
                gpi |= GPI_NOT_READY;
            }
            gpi
        }
    }

    /// A [`Deadline`] that has always already expired, so a fabric that never
    /// acks fails on the first poll instead of after 10 ms.
    struct Expired;

    impl Deadline for Expired {
        fn start(&mut self) {}
        fn expired(&self) -> bool {
            true
        }
    }

    struct FakeBus {
        log: Log,
        nak: Vec<u8>,
    }

    impl FakeBus {
        fn new(log: &Log) -> Self {
            Self {
                log: Rc::clone(log),
                nak: Vec::new(),
            }
        }

        fn naking(log: &Log, nak: &[u8]) -> Self {
            Self {
                log: Rc::clone(log),
                nak: nak.to_vec(),
            }
        }
    }

    impl I2cBus for FakeBus {
        fn write_reg(&mut self, reg: u8, value: u8) -> Result<()> {
            self.log.borrow_mut().push(Event::I2c(reg, value));
            if self.nak.contains(&reg) {
                return Err(Error::io(
                    format!("i2c write {reg:#04X}"),
                    std::io::Error::from_raw_os_error(libc::ENXIO),
                ));
            }
            Ok(())
        }
    }

    struct FakeMode {
        log: Log,
        fail: bool,
    }

    impl FakeMode {
        fn new(log: &Log) -> Self {
            Self {
                log: Rc::clone(log),
                fail: false,
            }
        }
    }

    impl ModeSink for FakeMode {
        fn write_mode_line(&mut self, line: &str) -> Result<()> {
            if self.fail {
                return Err(Error::io(
                    "open /sys/module/MiSTer_fb/parameters/mode",
                    std::io::Error::from_raw_os_error(libc::ENOENT),
                ));
            }
            self.log
                .borrow_mut()
                .push(Event::ModeLine(line.to_string()));
            Ok(())
        }
    }

    struct FakeTty {
        log: Log,
    }

    impl TextSink for FakeTty {
        fn write_text(&mut self, bytes: &[u8]) -> Result<()> {
            self.log.borrow_mut().push(Event::Text(bytes.to_vec()));
            Ok(())
        }
    }

    /// The i2c half of an event list, for the assertions that only care about
    /// the chip.
    fn i2c_only(events: &[Event]) -> Vec<(u8, u8)> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::I2c(reg, value) => Some((*reg, *value)),
                _ => None,
            })
            .collect()
    }

    /// The whole of `up` for 720p, as one ordered list.
    fn expected_up_720p() -> Vec<Event> {
        let mode = video::MODE_720P;
        let pll = video::solve_pll(mode.f_pix_mhz).expect("720p has a PLL solution");
        let geom = fb::FbGeometry::for_mode(mode.hact, mode.vact, false);

        let mut expected = Vec::new();

        // 1. The bulk tables, in Main's order: INIT, AUDIO, CSC
        //    (`video.cpp:1606-1613`). Built from the three tables rather than
        //    from `writes()` so that reordering `writes()` fails here too.
        expected.extend(
            adv7513::INIT
                .iter()
                .chain(adv7513::AUDIO)
                .chain(adv7513::CSC)
                .map(|&(reg, value)| Event::I2c(reg, value)),
        );

        // 2a. UIO_SET_VIDEO and its 26 words, between one EnableIO and one
        //     DisableIO (`video.cpp:2266-2294`).
        expected.push(Event::EnableIo);
        expected.push(Event::Word(mailbox::UIO_SET_VIDEO));
        expected.extend(
            video::set_video_words(&mode, &pll)
                .iter()
                .map(|&w| Event::Word(w)),
        );
        expected.push(Event::DisableIo);

        // 2b. The three mode registers, literal (`video.cpp:1710-1712`, and
        //     T1.3's own test asserts the same three).
        expected.extend(
            [(0x17u8, 0x62u8), (0x3B, 0x40), (0x3C, 0x04)]
                .map(|(reg, value)| Event::I2c(reg, value)),
        );

        // 3. UIO_SET_FBUF and its ten words, then the sysfs line — fabric
        //    first, sysfs second (§5).
        expected.push(Event::EnableIo);
        expected.push(Event::Word(mailbox::UIO_SET_FBUF));
        expected.extend(fb::enable_words(&geom).iter().map(|&w| Event::Word(w)));
        expected.push(Event::DisableIo);
        expected.push(Event::ModeLine("8888 1 1280 720 5120\n".to_string()));

        expected
    }

    /// T2.2's "Done when": `up` end to end, one ordered list, exact words and
    /// bytes.
    ///
    /// **Mutation-tested by hand before this was committed.** A test that
    /// asserts an order is worth nothing unless breaking the order breaks it,
    /// so each of these was applied to the code above, run, and reverted; the
    /// number after each is how many tests in this module went red.
    ///
    /// 1. `UIO_SET_VIDEO` sent after the three mode registers instead of
    ///    before them — 3.
    /// 2. The three mode registers deleted — 6.
    /// 3. The sysfs line written before the `UIO_SET_FBUF` burst — 6.
    /// 4. `command` instead of `command_if_supported` for `UIO_SET_FBUF`,
    ///    i.e. the §5 gate removed — 1.
    /// 5. The early [`require_bitstream`] check removed from [`hdmi_on`] — 1.
    /// 6. The bulk tables sent CSC, AUDIO, INIT instead of INIT, AUDIO,
    ///    CSC — 4.
    /// 7. [`up`] running `fb enable` before `hdmi` — 5.
    #[test]
    fn up_writes_every_stage_in_the_order_the_spec_fixes() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut bus = FakeBus::new(&log);
        let mut sink = FakeMode::new(&log);

        up(&mut mb, &mut bus, &mut sink, &video::MODE_720P).unwrap();

        assert_eq!(events(&log), expected_up_720p());
    }

    /// The same run, checked against literals rather than against the
    /// composers, so that a composer and this test cannot drift together.
    #[test]
    fn the_stages_of_up_are_where_they_should_be() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut bus = FakeBus::new(&log);
        let mut sink = FakeMode::new(&log);
        up(&mut mb, &mut bus, &mut sink, &video::MODE_720P).unwrap();
        let events = events(&log);

        // The first thing on the wire is the first row of `init_data[]`
        // (`video.cpp:1499`), and the last i2c write of the run is the VIC.
        let i2c = i2c_only(&events);
        assert_eq!(i2c.first().copied(), Some((0x98, 0x03)));
        assert_eq!(
            i2c.get(adv7513::INIT.len().saturating_sub(1)).copied(),
            Some((0xFA, 0x7D)),
            "the INIT table ends where the AUDIO table begins"
        );
        assert_eq!(
            i2c.get(adv7513::INIT.len()).copied(),
            adv7513::AUDIO.first().copied()
        );
        assert_eq!(
            i2c.get(i2c.len().saturating_sub(3)..),
            Some(&[(0x17, 0x62), (0x3B, 0x40), (0x3C, 0x04)][..])
        );

        // The mailbox bursts: opcode, then the first payload word of each.
        let words: Vec<u16> = events
            .iter()
            .filter_map(|e| match e {
                Event::Word(w) => Some(*w),
                _ => None,
            })
            .collect();
        assert_eq!(words.first().copied(), Some(0x20), "UIO_SET_VIDEO");
        assert_eq!(words.get(1).copied(), Some(1280), "hact, word 1");
        assert_eq!(words.len(), 1 + 26 + 1 + 10);
        assert_eq!(words.get(27).copied(), Some(0x2F), "UIO_SET_FBUF");
        assert_eq!(words.get(28).copied(), Some(0x8016), "FB_EN | RxB | 8888");

        // And the last event of all is the sysfs line, not the first.
        assert_eq!(
            events.last(),
            Some(&Event::ModeLine("8888 1 1280 720 5120\n".to_string()))
        );
    }

    /// Every word of both bursts is bracketed by the enable: the bus is
    /// claimed before the opcode and released after the payload, on every
    /// path (`spi.cpp:106-117`).
    #[test]
    fn no_word_is_sent_outside_an_enabled_window() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut bus = FakeBus::new(&log);
        let mut sink = FakeMode::new(&log);
        up(&mut mb, &mut bus, &mut sink, &video::MODE_720P).unwrap();

        let mut enabled = false;
        for event in events(&log) {
            match event {
                Event::EnableIo => {
                    assert!(!enabled, "EnableIO twice with no DisableIO between");
                    enabled = true;
                }
                Event::DisableIo => {
                    assert!(enabled, "DisableIO with no EnableIO");
                    enabled = false;
                }
                Event::Word(w) => assert!(enabled, "word {w:#06X} sent with the bus released"),
                _ => {}
            }
        }
        assert!(!enabled, "the bus was left enabled");
    }

    /// 480p differs in every one of the numbers the spec says it should, and
    /// in nothing else.
    #[test]
    fn up_480p_carries_the_480p_numbers() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut bus = FakeBus::new(&log);
        let mut sink = FakeMode::new(&log);
        up(&mut mb, &mut bus, &mut sink, &video::MODE_480P).unwrap();
        let events = events(&log);

        let i2c = i2c_only(&events);
        assert_eq!(
            i2c.get(i2c.len().saturating_sub(3)..),
            Some(&[(0x17, 0x62), (0x3B, 0x40), (0x3C, 0x01)][..]),
            "VIC 1 for 640x480"
        );
        assert_eq!(
            events.last(),
            Some(&Event::ModeLine("8888 1 640 480 2560\n".to_string()))
        );
    }

    /// `hdmi --off` is one register and nothing else: no mailbox word, no
    /// bulk table, no sysfs line (§4).
    #[test]
    fn hdmi_off_writes_only_the_power_register() {
        let log = log_new();
        let mut bus = FakeBus::new(&log);
        hdmi_off(&mut bus).unwrap();
        assert_eq!(events(&log), vec![Event::I2c(0x41, 0x50)]);
        assert_eq!(adv7513::POWER_DOWN, (0x41, 0x50));
    }

    /// `hdmi` on its own stops after the three mode registers: no
    /// `UIO_SET_FBUF`, no sysfs line. That is what makes `fb enable` a
    /// separate subcommand.
    #[test]
    fn hdmi_does_not_touch_the_frame_buffer() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut bus = FakeBus::new(&log);
        hdmi_on(&mut mb, &mut bus, &video::MODE_720P).unwrap();
        let events = events(&log);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Word(0x2F) | Event::ModeLine(_))),
            "hdmi sent UIO_SET_FBUF or wrote sysfs"
        );
        assert_eq!(
            i2c_only(&events).len(),
            adv7513::writes().count().saturating_add(3)
        );
    }

    /// A core that answers `0` to `UIO_SET_FBUF` gets the opcode and nothing
    /// else, and the sysfs knob is left alone (§5, `video.cpp:3480-3535`).
    #[test]
    fn an_unsupported_frame_buffer_sends_no_payload_and_writes_no_sysfs_line() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 0), MonotonicDeadline::default());
        let mut sink = FakeMode::new(&log);

        fb_enable(&mut mb, &mut sink, &video::MODE_720P).unwrap();

        assert_eq!(
            events(&log),
            vec![Event::EnableIo, Event::Word(0x2F), Event::DisableIo]
        );
    }

    /// `fb disable` is the opcode and a single `0`, gated on the same reply
    /// (`video.cpp:3527`), and it writes no sysfs line either.
    #[test]
    fn fb_disable_sends_one_zero_word() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        fb_disable(&mut mb).unwrap();
        assert_eq!(
            events(&log),
            vec![
                Event::EnableIo,
                Event::Word(0x2F),
                Event::Word(0),
                Event::DisableIo
            ]
        );
    }

    /// `fb enable` twice in a row is the same burst twice: idempotent, per §7.
    #[test]
    fn fb_enable_is_idempotent() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut sink = FakeMode::new(&log);
        fb_enable(&mut mb, &mut sink, &video::MODE_720P).unwrap();
        let once = events(&log);
        log.borrow_mut().clear();
        fb_enable(&mut mb, &mut sink, &video::MODE_720P).unwrap();
        assert_eq!(events(&log), once);
    }

    /// `up` twice is `up` once, twice: nothing latches, nothing accumulates.
    #[test]
    fn up_is_idempotent() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut bus = FakeBus::new(&log);
        let mut sink = FakeMode::new(&log);
        up(&mut mb, &mut bus, &mut sink, &video::MODE_720P).unwrap();
        log.borrow_mut().clear();
        up(&mut mb, &mut bus, &mut sink, &video::MODE_720P).unwrap();
        assert_eq!(events(&log), expected_up_720p());
    }

    /// A NAK on one register does not stop the run: that is
    /// [`hw::write_table`]'s policy, and `up` must not have reached past it
    /// to `write_reg` with a `?` (`video.cpp:1606-1611`, `:1716-1722`).
    #[test]
    fn a_refused_register_does_not_abort_the_run() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        // 0x98 is the first row of INIT; 0x3C is the VIC, one of the three
        // mode registers.
        let mut bus = FakeBus::naking(&log, &[0x98, 0x3C]);
        let mut sink = FakeMode::new(&log);

        up(&mut mb, &mut bus, &mut sink, &video::MODE_720P).unwrap();

        assert_eq!(events(&log), expected_up_720p());
    }

    /// No bitstream: exit 10, and **nothing written at all**. The early GPI
    /// read is what buys that; without it the 92 i2c writes would go out
    /// first and fail one by one (§1).
    #[test]
    fn no_bitstream_is_exit_10_before_anything_is_written() {
        let log = log_new();
        let mut regs = FakeRegs::new(&log, 1);
        regs.ready = false;
        let mut mb = Mailbox::new(regs, MonotonicDeadline::default());
        let mut bus = FakeBus::new(&log);
        let mut sink = FakeMode::new(&log);

        let err = up(&mut mb, &mut bus, &mut sink, &video::MODE_720P).unwrap_err();
        assert_eq!(err.exit_code(), 10);
        assert!(matches!(err, Error::NoBitstream));
        assert!(events(&log).is_empty(), "wrote something with no bitstream");
    }

    /// A fabric that never acks is exit 11, and the enable bit is dropped on
    /// the way out (`mailbox::Mailbox::command` does it on every path).
    #[test]
    fn a_fabric_that_never_acks_is_exit_11() {
        let log = log_new();
        let mut regs = FakeRegs::new(&log, 1);
        regs.acks = false;
        let mut mb = Mailbox::new(regs, Expired);
        let mut bus = FakeBus::new(&log);

        let err = hdmi_on(&mut mb, &mut bus, &video::MODE_720P).unwrap_err();
        assert_eq!(err.exit_code(), 11);
        assert!(matches!(err, Error::Timeout));
        assert_eq!(
            events(&log).last(),
            Some(&Event::DisableIo),
            "the bus was left claimed after a timeout"
        );
    }

    /// A sysfs knob that cannot be written is exit 14, after the fabric burst
    /// has already gone out — the order §5 fixes means the failure can only
    /// be discovered there.
    #[test]
    fn an_unwritable_sysfs_knob_is_exit_14() {
        let log = log_new();
        let mut mb = Mailbox::new(FakeRegs::new(&log, 1), MonotonicDeadline::default());
        let mut sink = FakeMode::new(&log);
        sink.fail = true;
        let err = fb_enable(&mut mb, &mut sink, &video::MODE_720P).unwrap_err();
        assert_eq!(err.exit_code(), 14);
    }

    /// `say` goes to the text sink and nowhere near the fabric.
    #[test]
    fn say_writes_the_line_to_the_text_sink() {
        let log = log_new();
        let mut tty = FakeTty {
            log: Rc::clone(&log),
        };
        say::say(
            &mut tty,
            true,
            &["Installing".to_string(), "MiSTer...".to_string()],
        )
        .unwrap();
        assert_eq!(
            events(&log),
            vec![Event::Text(b"\x1b[2J\x1b[HInstalling MiSTer...\n".to_vec())]
        );
    }

    // -----------------------------------------------------------------------
    // Arguments and exit codes
    // -----------------------------------------------------------------------

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }

    fn parse_ok(args: &[&str]) -> Command {
        match parse(&argv(args)) {
            Ok(cmd) => cmd,
            Err(e) => panic!("{args:?} should parse, got {e}"),
        }
    }

    fn parse_usage(args: &[&str]) {
        match parse(&argv(args)) {
            Ok(cmd) => panic!("{args:?} should be a usage error, parsed as {cmd:?}"),
            Err(e) => {
                assert_eq!(e.exit_code(), 2, "{args:?}");
                assert!(matches!(e, Error::Usage(_)), "{args:?}");
            }
        }
    }

    /// T2.2's "Done when": no arguments is exit 2, and the message the caller
    /// gets is the usage text.
    #[test]
    fn no_arguments_is_a_usage_error() {
        parse_usage(&[]);
        assert!(USAGE.contains("itsalive probe"));
        assert!(USAGE.contains("itsalive up"));
    }

    /// The usage text lists every subcommand that exists and nothing else.
    /// `leds` is v1.1 (`docs/ARCHITECTURE.md` §7) and must not be advertised.
    #[test]
    fn the_usage_text_matches_the_implemented_commands() {
        for name in ["probe", "hdmi", "fb enable", "fb disable", "say", "up"] {
            assert!(USAGE.contains(name), "usage does not mention {name}");
        }
        assert!(!USAGE.contains("leds"));
        for (name, _) in video::MODES {
            assert!(USAGE.contains(name), "usage does not mention mode {name}");
        }
    }

    #[test]
    fn probe_parses_with_and_without_json() {
        assert_eq!(parse_ok(&["probe"]), Command::Probe { json: false });
        assert_eq!(
            parse_ok(&["probe", "--json"]),
            Command::Probe { json: true }
        );
        parse_usage(&["probe", "--jsonn"]);
        parse_usage(&["probe", "extra"]);
        parse_usage(&["probe", "--mode", "720p"]);
    }

    #[test]
    fn hdmi_parses_its_two_flags() {
        assert_eq!(
            parse_ok(&["hdmi"]),
            Command::Hdmi {
                mode: video::MODE_720P,
                off: false
            }
        );
        assert_eq!(
            parse_ok(&["hdmi", "--mode", "480p"]),
            Command::Hdmi {
                mode: video::MODE_480P,
                off: false
            }
        );
        assert_eq!(
            parse_ok(&["hdmi", "--off"]),
            Command::Hdmi {
                mode: video::MODE_720P,
                off: true
            }
        );
        // The mode is inert for a power-down but is not an error: an
        // installer may pass it to every `hdmi` call.
        assert_eq!(
            parse_ok(&["hdmi", "--off", "--mode", "480p"]),
            Command::Hdmi {
                mode: video::MODE_480P,
                off: true
            }
        );
        parse_usage(&["hdmi", "--mode"]);
        parse_usage(&["hdmi", "--mode", "1080p"]);
        parse_usage(&["hdmi", "--mode", "720P"]);
        parse_usage(&["hdmi", "--on"]);
        parse_usage(&["hdmi", "off"]);
    }

    #[test]
    fn fb_needs_enable_or_disable() {
        assert_eq!(
            parse_ok(&["fb", "enable"]),
            Command::FbEnable {
                mode: video::MODE_720P
            }
        );
        assert_eq!(
            parse_ok(&["fb", "enable", "--mode", "480p"]),
            Command::FbEnable {
                mode: video::MODE_480P
            }
        );
        assert_eq!(parse_ok(&["fb", "disable"]), Command::FbDisable);
        parse_usage(&["fb"]);
        parse_usage(&["fb", "on"]);
        parse_usage(&["fb", "enable", "--off"]);
        parse_usage(&["fb", "enable", "--mode"]);
        // `fb disable` takes nothing, mode included: there is no timing in a
        // handover back to the core.
        parse_usage(&["fb", "disable", "--mode", "720p"]);
    }

    #[test]
    fn say_takes_its_flag_then_everything_else() {
        assert_eq!(
            parse_ok(&["say", "hello", "world"]),
            Command::Say {
                clear: false,
                text: argv(&["hello", "world"])
            }
        );
        assert_eq!(
            parse_ok(&["say", "--clear", "hello"]),
            Command::Say {
                clear: true,
                text: argv(&["hello"])
            }
        );
        // `--clear` on its own clears the screen and says nothing.
        assert_eq!(
            parse_ok(&["say", "--clear"]),
            Command::Say {
                clear: true,
                text: vec![]
            }
        );
        // Once the text has started, a leading dash is text.
        assert_eq!(
            parse_ok(&["say", "step", "--clear"]),
            Command::Say {
                clear: false,
                text: argv(&["step", "--clear"])
            }
        );
        // `--` ends the options.
        assert_eq!(
            parse_ok(&["say", "--", "--clear"]),
            Command::Say {
                clear: false,
                text: argv(&["--clear"])
            }
        );
        assert_eq!(
            parse_ok(&["say", "-50%"]),
            Command::Say {
                clear: false,
                text: argv(&["-50%"])
            }
        );
        parse_usage(&["say"]);
        parse_usage(&["say", "--quiet", "hello"]);
    }

    #[test]
    fn up_takes_only_a_mode() {
        assert_eq!(
            parse_ok(&["up"]),
            Command::Up {
                mode: video::MODE_720P
            }
        );
        assert_eq!(
            parse_ok(&["up", "--mode", "480p"]),
            Command::Up {
                mode: video::MODE_480P
            }
        );
        parse_usage(&["up", "--off"]);
        parse_usage(&["up", "720p"]);
        parse_usage(&["up", "--mode", ""]);
    }

    #[test]
    fn help_parses_and_unknown_subcommands_do_not() {
        assert_eq!(parse_ok(&["--help"]), Command::Help);
        assert_eq!(parse_ok(&["-h"]), Command::Help);
        assert_eq!(parse_ok(&["help"]), Command::Help);
        parse_usage(&["--help", "hdmi"]);
        parse_usage(&["leds", "0xFF"]);
        parse_usage(&["HDMI"]);
        parse_usage(&["--version"]);
        parse_usage(&[""]);
    }

    /// `--help` is the one command that writes to stdout and exits 0 without
    /// opening anything, so it is safe to run here.
    #[test]
    fn help_runs_without_touching_hardware() {
        run(&argv(&["--help"])).unwrap();
    }

    /// Every variant of §7's table maps to its code, including the ones this
    /// file never constructs itself.
    #[test]
    fn every_error_maps_to_its_documented_exit_code() {
        let cases: Vec<(Error, i32)> = vec![
            (Error::Usage("no subcommand".into()), 2),
            (Error::NoBitstream, 10),
            (Error::Timeout, 11),
            (Error::NoAdv7513, 12),
            (Error::AmbiguousBus(vec![0, 2]), 13),
            (
                Error::io(
                    "open /dev/mem",
                    std::io::Error::from_raw_os_error(libc::EACCES),
                ),
                14,
            ),
        ];
        for (err, code) in cases {
            assert_eq!(err.exit_code(), code, "{err}");
            // Every code fits in the byte a process exit status carries.
            assert!(u8::try_from(err.exit_code()).is_ok());
        }
    }

    // -----------------------------------------------------------------------
    // probe's rendering
    // -----------------------------------------------------------------------

    fn sample_findings() -> Vec<Finding> {
        vec![
            Finding::ok("bitstream", "fabric in user mode (GPI 0x00000000)"),
            Finding::failed("adv7513", &Error::NoAdv7513),
            Finding::ok("fb_mode", "/sys/module/MiSTer_fb/parameters/mode present"),
        ]
    }

    #[test]
    fn probe_prints_one_line_per_finding() {
        let lines = render_lines(&sample_findings());
        let lines: Vec<&str> = lines.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("ok   bitstream: "));
        assert!(lines[1].starts_with("FAIL adv7513: "));
        assert!(lines[1].contains("no ADV7513 found"));
    }

    #[test]
    fn probe_json_is_one_object_with_the_first_failing_code() {
        let json = render_json(&sample_findings(), 12);
        assert_eq!(
            json,
            "{\"findings\":[\
{\"name\":\"bitstream\",\"ok\":true,\"detail\":\"fabric in user mode (GPI 0x00000000)\",\"exit_code\":0},\
{\"name\":\"adv7513\",\"ok\":false,\"detail\":\"no ADV7513 found at 0x39 on any i2c bus\",\"exit_code\":12},\
{\"name\":\"fb_mode\",\"ok\":true,\"detail\":\"/sys/module/MiSTer_fb/parameters/mode present\",\"exit_code\":0}\
],\"exit_code\":12}"
        );
    }

    /// The detail of a finding is an [`Error`] message, which can carry a
    /// path the caller chose and `strerror` text in the caller's locale. A
    /// quote or a backslash in either must not end the string early.
    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_string("plain"), "\"plain\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb\tc\rd"), "\"a\\nb\\tc\\rd\"");
        assert_eq!(json_string("\u{8}\u{c}"), "\"\\b\\f\"");
        assert_eq!(json_string("\u{1}\u{1f}"), "\"\\u0001\\u001f\"");
        // Non-ASCII is legal in a JSON string and is left as UTF-8.
        assert_eq!(json_string("Eingabe/Ausgabe"), "\"Eingabe/Ausgabe\"");
        assert_eq!(json_string("ä"), "\"ä\"");
    }

    /// A finding built from an error whose message contains a quote survives
    /// the round trip into the JSON object.
    #[test]
    fn a_hostile_detail_cannot_break_the_json() {
        let err = Error::Usage("say \"hi\"\\".to_string());
        let json = render_json(&[Finding::failed("bitstream", &err)], 2);
        assert!(json.contains("\\\"hi\\\"\\\\"), "{json}");
        assert_eq!(json.matches('{').count(), 2);
    }

    #[test]
    fn a_clean_probe_has_no_failures_and_no_code() {
        let findings = vec![Finding::ok("bitstream", "fabric in user mode")];
        assert_eq!(
            render_json(&findings, 0),
            "{\"findings\":[{\"name\":\"bitstream\",\"ok\":true,\"detail\":\"fabric in user mode\",\"exit_code\":0}],\"exit_code\":0}"
        );
    }
}
