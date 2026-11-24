//! Which board this is, from the device tree's root `compatible`.
//!
//! Two boards, two register paths, and nothing else differs at this level:
//!
//! * **DE10-Nano** (Cyclone V): GPO/GPI in the FPGA manager through
//!   `/dev/mem`, the ADV7513 over HPS i2c, the PLL and timings over the
//!   mailbox. Everything `docs/ARCHITECTURE.md` §1-§8 describes.
//! * **DE25-Nano** (Agilex 5): the same mailbox protocol, but on a soft
//!   register block behind the lightweight bridge, reached through a UIO node;
//!   the ADV7513 and the timing belong to the fabric. `docs/ARCHITECTURE.md`
//!   §9 and [`crate::de25`].
//!
//! The answer is read from `/proc/device-tree/compatible`, a list of
//! NUL-terminated strings, most specific first. The match is on the SoC
//! family string, not the board's, because both images in Buildroot_MiSTer
//! carry the SoC family string and the DE25 one names a reference board
//! (`socfpga_agilex5_de25nano.dts:18`).

use crate::{Error, Result};

/// The kernel's copy of the root node's `compatible` property.
pub const COMPATIBLE_PATH: &str = "/proc/device-tree/compatible";

/// Cyclone V, the DE10-Nano's SoC (`socfpga_cyclone5_de10nano.dts`).
const CYCLONE5: &str = "altr,socfpga-cyclone5";

/// Agilex 5, the DE25-Nano's SoC (`socfpga_agilex5_de25nano.dts:18`).
const AGILEX5: &str = "intel,socfpga-agilex5";

/// The two boards this tool knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Board {
    /// Terasic DE10-Nano, Cyclone V.
    De10Nano,
    /// Terasic DE25-Nano, Agilex 5.
    De25Nano,
}

/// The board a `compatible` property names, or `None` for neither.
pub fn from_compatible(raw: &[u8]) -> Option<Board> {
    raw.split(|&b| b == 0).find_map(|s| match s {
        s if s == CYCLONE5.as_bytes() => Some(Board::De10Nano),
        s if s == AGILEX5.as_bytes() => Some(Board::De25Nano),
        _ => None,
    })
}

/// [`from_compatible`] over whatever the kernel said, with the fallback.
///
/// **Unknown means DE10 on an ARMv7 build**, which is what every ARMv7 MiSTer
/// is and what this tool did before it knew there was a second board: a
/// stock kernel, or one with no device tree in procfs, keeps today's path
/// exactly. Any other build refuses instead, because the DE10 path writes a
/// physical address that means something else on any other SoC.
pub fn resolve(compatible: std::io::Result<Vec<u8>>) -> Result<Board> {
    let found = compatible.as_deref().ok().and_then(from_compatible);
    match found {
        Some(board) => Ok(board),
        None if cfg!(target_arch = "arm") => Ok(Board::De10Nano),
        None => Err(Error::Unrecognised(format!(
            "{COMPATIBLE_PATH} names neither {CYCLONE5} nor {AGILEX5}"
        ))),
    }
}

/// The board this process is running on.
pub fn detect() -> Result<Board> {
    resolve(std::fs::read(COMPATIBLE_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn the_de10_device_tree_is_the_de10() {
        let raw = b"terasic,de10-nano\0altr,socfpga-cyclone5\0altr,socfpga\0";
        assert_eq!(from_compatible(raw), Some(Board::De10Nano));
    }

    #[test]
    fn the_de25_device_tree_is_the_de25() {
        let raw = b"intel,socfpga-agilex5-socdk\0intel,socfpga-agilex5\0";
        assert_eq!(from_compatible(raw), Some(Board::De25Nano));
    }

    #[test]
    fn a_prefix_is_not_a_match() {
        // The DE25's first string starts with the SoC family's; only the
        // whole string counts.
        assert_eq!(from_compatible(b"intel,socfpga-agilex5-socdk\0"), None);
        assert_eq!(from_compatible(b"altr,socfpga\0"), None);
        assert_eq!(from_compatible(b""), None);
    }

    #[test]
    fn unknown_or_unreadable_is_the_de10_only_on_armv7() {
        let unknown = resolve(Ok(b"raspberrypi,4-model-b\0brcm,bcm2711\0".to_vec()));
        let missing = resolve(Err(io::Error::from_raw_os_error(libc::ENOENT)));
        for got in [unknown, missing] {
            if cfg!(target_arch = "arm") {
                assert_eq!(got.ok(), Some(Board::De10Nano));
            } else {
                let err = got.expect_err("only an ARMv7 build may assume the DE10");
                assert_eq!(err.exit_code(), 15);
            }
        }
    }

    #[test]
    fn a_known_board_wins_on_every_build() {
        let de25 = resolve(Ok(b"intel,socfpga-agilex5\0".to_vec()));
        assert_eq!(de25.ok(), Some(Board::De25Nano));
    }
}
