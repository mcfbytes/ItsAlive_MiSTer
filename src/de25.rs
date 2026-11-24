//! The DE25-Nano's half: the fabric guard, the LW-bridge mailbox and `fbcfg`.
//!
//! `docs/ARCHITECTURE.md` §9 is the contract; every number here is a fact read
//! from a third-party DE25 port's fabric and scripts (its
//! `mister_mailbox_avmm.sv`, `de25_lwh2f_periph.sv`, `golden_top.v`,
//! `de25_fabric_ready.py`, `fb_kernel_scaler.py`) or from Buildroot_MiSTer's
//! DE25 device tree. No code is taken from either.
//!
//! **The hazard that shapes all of it:** on Agilex 5 a read of the lightweight
//! bridge while the fabric is unconfigured is an asynchronous SError, not a
//! failed probe. There is no GPI bit 31 to ask first. So [`fabric_guard`]
//! decides from the kernel's state alone whether the bridge may be touched at
//! all, and only then does [`check_and_program`] read the mailbox's MAGIC.

use crate::mailbox::Regs;
use crate::{Error, Result};

/// The UIO node over the lightweight bridge (`linux,uio-name` in the DTS).
pub const LW_UIO_NAME: &str = "mister_lw_bridge";

/// The mailbox block's offset in the LW window: GPO, GPI, MAGIC.
pub const MAILBOX: usize = 0x1_0100;

/// The `fbcfg` block's offset: a second instance of the same register block,
/// whose GPO is ascal's DDR base.
pub const FBCFG: usize = 0x1_0200;

/// Register offsets inside either block (`mister_mailbox_avmm.sv`'s map).
pub const GPO: usize = 0x00;
/// See [`GPO`].
pub const GPI: usize = 0x04;
/// See [`GPO`]. `{24'h5CA623, CORE_TYPE}`, readable at any time.
pub const MAGIC: usize = 0x1C;

/// Bits 31:8 of MAGIC on both blocks; the same word `fpga_core_id()` checks.
pub const MAGIC_HIGH: u32 = 0x5C_A623;

/// The `fbcfg` block's whole MAGIC: its `CORE_TYPE` is `0xFB`.
pub const FBCFG_MAGIC: u32 = 0x5CA6_23FB;

/// How far into the LW window the mapping must reach: the end of `fbcfg`.
pub const LW_SPAN: usize = FBCFG + 0x20;

/// Ascal's scratch base: the `mister-scaler@9e000000` reservation, 32 MiB.
/// Zero gates ascal off, and every full reconfiguration clears it.
pub const SCALER_BASE: u32 = 0x9E00_0000;

/// The `mister-fb@a0000000` reservation; `/dev/fb0` is its buffer 0, one
/// page in.
pub const FB_REGION: u32 = 0xA000_0000;

/// The fabric's HDMI raster. Ascal's output timing and the pixel clock are
/// the fabric's own (1080p60 at 148.5 MHz) and are not reprogrammable from
/// the HPS, so the frame buffer is scaled onto this, whatever its own size.
pub const RASTER: (u16, u16) = (1920, 1080);

/// The kernel's FPGA manager state.
pub const FPGA0_STATE: &str = "/sys/class/fpga_manager/fpga0/state";

/// The HPS-first markers the third-party port keeps beside its QSPI choice.
pub const HPS_FIRST_MARKERS: [&str; 2] = ["/media/fat/HPS_FIRST_MODE", "/etc/HPS_FIRST_MODE"];

/// May the LW bridge be touched at all? Pure: the caller reads the inputs.
///
/// The third-party port's guard, as facts:
///
/// 1. A `de25_core=` token on the kernel command line is U-Boot saying what it
///    loaded. `none` or empty means it left the fabric unconfigured, and then
///    only an `operating` FPGA manager (a load since boot) makes it safe.
///    Any other value names a core U-Boot loaded: safe.
/// 2. With no token, an HPS-first marker means the fabric starts empty: only
///    `operating` is safe.
/// 3. Otherwise the fabric was configured from QSPI before the HPS ran, and
///    the manager reads `unknown` because it did not do the load.
///
/// One rule is ours: a manager in any state but `operating` or `unknown` is
/// mid-load or failed, and that is never safe, whatever the rules above say.
pub fn fabric_guard(cmdline: &str, fpga0: Option<&str>, hps_first: bool) -> Result<()> {
    let state = fpga0.map(str::trim);
    let operating = state == Some("operating");
    if let Some(s) = state
        && !operating
        && s != "unknown"
    {
        return Err(Error::FabricNotReady(format!("fpga0 is {s:?}")));
    }
    let token = cmdline
        .split_ascii_whitespace()
        .find_map(|t| t.strip_prefix("de25_core="));
    let empty = match token {
        Some(core) => core.is_empty() || core == "none",
        None => hps_first,
    };
    if empty && !operating {
        let why = match token {
            Some(_) => {
                "U-Boot left the fabric unconfigured (de25_core=none) and nothing loaded it since"
            }
            None => "HPS-first boot and fpga0 is not operating",
        };
        return Err(Error::FabricNotReady(why.to_string()));
    }
    Ok(())
}

/// [`fabric_guard`] over this machine's kernel state.
pub fn guard_from_system() -> Result<()> {
    let cmdline =
        std::fs::read_to_string("/proc/cmdline").map_err(|e| Error::io("read /proc/cmdline", e))?;
    let fpga0 = std::fs::read_to_string(FPGA0_STATE).ok();
    let hps_first = HPS_FIRST_MARKERS
        .iter()
        .any(|p| std::path::Path::new(p).exists());
    fabric_guard(&cmdline, fpga0.as_deref(), hps_first)
}

/// A mapping of the LW window, word-addressed by byte offset.
///
/// Callers pass only this module's constants, all below [`LW_SPAN`], which the
/// real mapping checks once when it is opened.
pub trait LwWindow {
    /// One aligned 32-bit read at `offset`.
    fn read32(&self, offset: usize) -> u32;
    /// One aligned 32-bit write at `offset`.
    fn write32(&mut self, offset: usize, value: u32);
}

/// Verify both MAGICs, then point ascal at its scratch.
///
/// The mailbox MAGIC is the first access of all: it proves the window decodes
/// and that this is a MiSTer framework, before anything is written. The
/// `fbcfg` write is read back because a wrong bitstream can decode the
/// address and drop the write.
pub fn check_and_program<W: LwWindow + ?Sized>(lw: &mut W) -> Result<()> {
    check_magic(lw)?;
    program_fbcfg(lw)
}

/// Both MAGICs, read-only: the mailbox's first, then `fbcfg`'s. `probe` stops
/// here.
pub fn check_magic<W: LwWindow + ?Sized>(lw: &W) -> Result<()> {
    let magic = lw.read32(MAILBOX + MAGIC);
    if magic >> 8 != MAGIC_HIGH {
        return Err(Error::Unrecognised(format!(
            "LW mailbox MAGIC {magic:#010X}, expected {MAGIC_HIGH:#08X}xx: not a MiSTer fabric"
        )));
    }
    let magic = lw.read32(FBCFG + MAGIC);
    if magic != FBCFG_MAGIC {
        return Err(Error::Unrecognised(format!(
            "fbcfg MAGIC {magic:#010X}, expected {FBCFG_MAGIC:#010X}"
        )));
    }
    Ok(())
}

/// Write ascal's scratch base into `fbcfg`, and read it back.
pub fn program_fbcfg<W: LwWindow + ?Sized>(lw: &mut W) -> Result<()> {
    lw.write32(FBCFG + GPO, SCALER_BASE);
    let back = lw.read32(FBCFG + GPO);
    if back != SCALER_BASE {
        return Err(Error::Unrecognised(format!(
            "fbcfg read back {back:#010X} after writing {SCALER_BASE:#010X}"
        )));
    }
    Ok(())
}

/// The mailbox's GPO/GPI pair on the LW window: [`Regs`] for the DE25.
///
/// Same protocol word for word as the DE10's FPGA-manager pair, including the
/// core-magic gate on GPI while GPO bit 31 is clear.
#[derive(Debug)]
pub struct LwRegs<W: LwWindow>(pub W);

impl<W: LwWindow> Regs for LwRegs<W> {
    fn gpo_write(&mut self, value: u32) {
        self.0.write32(MAILBOX + GPO, value);
    }

    fn gpi_read(&self) -> u32 {
        self.0.read32(MAILBOX + GPI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A register file: what was written, readable back, plus the two MAGICs.
    #[derive(Default)]
    struct FakeLw {
        regs: BTreeMap<usize, u32>,
        /// Every access in order: `(offset, Some(value))` for a write.
        trace: std::cell::RefCell<Vec<(usize, Option<u32>)>>,
        drop_writes: bool,
    }

    impl FakeLw {
        fn mister() -> Self {
            let mut lw = Self::default();
            lw.regs.insert(MAILBOX + MAGIC, 0x5CA6_23A4);
            lw.regs.insert(FBCFG + MAGIC, FBCFG_MAGIC);
            lw
        }
    }

    impl LwWindow for FakeLw {
        fn read32(&self, offset: usize) -> u32 {
            self.trace.borrow_mut().push((offset, None));
            self.regs.get(&offset).copied().unwrap_or(0)
        }
        fn write32(&mut self, offset: usize, value: u32) {
            self.trace.borrow_mut().push((offset, Some(value)));
            if !self.drop_writes {
                self.regs.insert(offset, value);
            }
        }
    }

    #[test]
    fn a_qspi_configured_fabric_passes() {
        // No breadcrumb, no marker, and the manager did not do the load.
        assert!(
            fabric_guard(
                "console=ttyS0 root=/dev/mmcblk0p2",
                Some("unknown\n"),
                false
            )
            .is_ok()
        );
        assert!(fabric_guard("root=/dev/mmcblk0p2", None, false).is_ok());
    }

    #[test]
    fn a_breadcrumb_naming_a_core_passes() {
        assert!(fabric_guard("de25_core=menu quiet", Some("unknown"), false).is_ok());
        // The breadcrumb outranks the marker: it is U-Boot's own word.
        assert!(fabric_guard("de25_core=menu", Some("unknown"), true).is_ok());
    }

    #[test]
    fn an_empty_fabric_is_refused_until_something_loads_it() {
        for cmdline in ["de25_core=none", "quiet de25_core=", "x de25_core=none y"] {
            let err = fabric_guard(cmdline, Some("unknown"), false).expect_err(cmdline);
            assert!(matches!(err, Error::FabricNotReady(_)), "{cmdline}");
            assert_eq!(err.exit_code(), 10);
            assert!(
                fabric_guard(cmdline, None, false).is_err(),
                "{cmdline}, no fpga0"
            );
            assert!(
                fabric_guard(cmdline, Some("operating\n"), false).is_ok(),
                "{cmdline}"
            );
        }
    }

    #[test]
    fn hps_first_needs_an_operating_manager() {
        assert!(fabric_guard("root=/dev/mmcblk0p2", Some("unknown"), true).is_err());
        assert!(fabric_guard("root=/dev/mmcblk0p2", None, true).is_err());
        assert!(fabric_guard("root=/dev/mmcblk0p2", Some("operating"), true).is_ok());
    }

    #[test]
    fn a_manager_mid_load_or_failed_is_always_refused() {
        for state in ["write", "write init", "write error", "power off", "reset"] {
            for cmdline in ["de25_core=menu", "root=/dev/mmcblk0p2"] {
                let err = fabric_guard(cmdline, Some(state), false).expect_err(state);
                assert!(matches!(err, Error::FabricNotReady(_)), "{state}");
            }
        }
    }

    #[test]
    fn the_token_must_be_whole() {
        // `xde25_core=none` is not the breadcrumb.
        assert!(fabric_guard("xde25_core=none", Some("unknown"), false).is_ok());
    }

    #[test]
    fn magic_is_read_first_and_fbcfg_gets_the_scaler_base() {
        let mut lw = FakeLw::mister();
        check_and_program(&mut lw).expect("a MiSTer fabric");
        let trace = lw.trace.borrow().clone();
        assert_eq!(
            trace,
            vec![
                (MAILBOX + MAGIC, None),
                (FBCFG + MAGIC, None),
                (FBCFG + GPO, Some(0x9E00_0000)),
                (FBCFG + GPO, None),
            ]
        );
    }

    #[test]
    fn a_wrong_mailbox_magic_stops_before_anything_else() {
        for magic in [0, 0xFFFF_FFFF, 0x5CA6_2300 ^ 0x0100_0000] {
            let mut lw = FakeLw::mister();
            lw.regs.insert(MAILBOX + MAGIC, magic);
            let err = check_and_program(&mut lw).expect_err("wrong magic");
            assert_eq!(err.exit_code(), 15);
            assert_eq!(lw.trace.borrow().as_slice(), &[(MAILBOX + MAGIC, None)]);
        }
    }

    #[test]
    fn any_core_type_passes_the_mailbox_magic() {
        let mut lw = FakeLw::mister();
        lw.regs.insert(MAILBOX + MAGIC, 0x5CA6_23A5);
        assert!(check_and_program(&mut lw).is_ok());
    }

    #[test]
    fn a_wrong_fbcfg_magic_writes_nothing() {
        let mut lw = FakeLw::mister();
        lw.regs.insert(FBCFG + MAGIC, 0x5CA6_23A4);
        let err = check_and_program(&mut lw).expect_err("wrong fbcfg magic");
        assert_eq!(err.exit_code(), 15);
        assert!(lw.trace.borrow().iter().all(|(_, w)| w.is_none()));
    }

    #[test]
    fn a_dropped_fbcfg_write_is_caught() {
        let mut lw = FakeLw::mister();
        lw.drop_writes = true;
        assert_eq!(
            check_and_program(&mut lw).expect_err("dropped").exit_code(),
            15
        );
    }

    #[test]
    fn the_mailbox_pair_sits_on_the_lw_offsets() {
        let mut regs = LwRegs(FakeLw::mister());
        regs.gpo_write(0x8010_0000);
        let _ = regs.gpi_read();
        assert_eq!(
            regs.0.trace.borrow().as_slice(),
            &[(0x1_0100, Some(0x8010_0000)), (0x1_0104, None)]
        );
    }

    #[test]
    fn every_offset_is_inside_the_span() {
        for off in [
            MAILBOX + GPO,
            MAILBOX + GPI,
            MAILBOX + MAGIC,
            FBCFG + GPO,
            FBCFG + MAGIC,
        ] {
            assert!(off + 4 <= LW_SPAN && off.is_multiple_of(4), "{off:#X}");
        }
    }
}
