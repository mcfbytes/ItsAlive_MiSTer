//! ADV7513 HDMI transmitter: the register tables.
//!
//! This is the *table half* of the module (`docs/ARCHITECTURE.md` §4 and §6):
//! plain values in, plain values out, no `unsafe`, no I/O, host-testable. The
//! I2C transport that actually pushes these pairs at chip address
//! [`CHIP_ADDR`] lives in `hw.rs`.
//!
//! Everything here is transcribed from Main_MiSTer at commit `6cda9cc`. Main
//! builds three of its four tables at run time from `cfg.*` fields parsed out
//! of `MiSTer.ini`; we ship without an ini, so each of those rows is evaluated
//! here for the value `cfg_parse()` (`cfg.cpp:592-641`) leaves in place when
//! the file is absent, and the comment beside the row names the default that
//! produced it. A reviewer should be able to re-derive every byte below from
//! the cited `file:line` without guessing.
//!
//! Nobody has to take that on trust: `tests/golden/adv7513.cpp` is the same
//! four C functions compiled against upstream's own `mat4x4.h`, and
//! `tests/adv7513_golden.rs` asserts every one of the 92 pairs below, in
//! order, against what that program prints. The unit tests in this file lock
//! the shape of the tables; the golden test is what proves the bytes.
//!
//! The no-ini defaults this module depends on, all from `cfg.cpp`:
//!
//! | `cfg` field | Default | Source |
//! |---|---|---|
//! | `vga_mode_int` | 0 (RGB) | `cfg.cpp:594` `memset(&cfg, 0, ...)`, not overridden at `:627-639` because `vga_mode` is empty |
//! | `direct_video` | 0 | `cfg.cpp:594` |
//! | `hdmi_limited` | 0 (full range) | `cfg.cpp:594` |
//! | `hdr` | 0 (no HDR) | `cfg.cpp:594` |
//! | `hdmi_game_mode` | 0 | `cfg.cpp:594` |
//! | `hdmi_audio_96k` | 0 (48 kHz) | `cfg.cpp:594` |
//! | `video_hue` | 0 | `cfg.cpp:594` |
//! | `dvi_mode` | 2 ("decide from EDID") | `cfg.cpp:603` |
//! | `video_brightness` | 50 | `cfg.cpp:609` |
//! | `video_contrast` | 50 | `cfg.cpp:610` |
//! | `video_saturation` | 100 | `cfg.cpp:611` |
//! | `video_gain_offset` | `"1, 0, 1, 0, 1, 0"` | `cfg.cpp:612` |
//!
//! Two of those deserve a sentence:
//!
//! - `ypbpr` is `(cfg.vga_mode_int == 1) && (cfg.direct_video == 1)`
//!   (`video.cpp:1219`, `:1464`), so it is **false** for us and every
//!   `ypbpr ? a : b` below takes the `b` arm.
//! - `dvi_mode == 2` means "resolve from the sink's EDID", and Main only
//!   resolves it at `video.cpp:1955-1959`, which runs inside `read_edid()` —
//!   called *after* `hdmi_config_init()` (`video.cpp:2694-2695`). So the value
//!   Main writes on its first pass, and the only value we ever write, is the
//!   unresolved `dvi_mode == 2` one. We never read EDID at all
//!   (`docs/ARCHITECTURE.md` §4, "Not done").

/// The ADV7513's main register map answers at this I2C chip address.
///
/// `video.cpp:1470`, `i2c_open(0x39, 0, -1, &adv_bus)`.
pub const CHIP_ADDR: u8 = 0x39;

/// `hdmi_config_init()`'s bulk table, `video.cpp:1499-1604`.
///
/// Verbatim and in order: Main writes it top to bottom at `video.cpp:1606-1610`
/// with one SMBus write-byte-data per row, logging and continuing on a NAK.
///
/// First row `(0x98, 0x03)`, last row `(0xFA, 0x7D)`, 51 rows.
///
/// Note that rows `0x17`, `0x3B` and `0x3C` are written again later by
/// [`mode_regs`] with different values; that is Main's behaviour too, and the
/// duplication is across tables, not within one.
pub const INIT: &[(u8, u8)] = &[
    (0x98, 0x03), // video.cpp:1500  ADI required write (C writes octal `03`)
    (0xD6, 0xC0), // video.cpp:1502  0b11000000: [7:6] HPD control = always high
    (0x41, 0x10), // video.cpp:1508  power-down control: powered up (see POWER_UP)
    (0x9A, 0x70), // video.cpp:1509  ADI required write
    (0x9C, 0x30), // video.cpp:1510  ADI required write
    (0x9D, 0x61), // video.cpp:1511  0b01100001: [7:4]=0110 fixed, [3:2]=00 clock not divided, [1:0]=01 fixed
    (0xA2, 0xA4), // video.cpp:1514  ADI required write
    (0xA3, 0xA4), // video.cpp:1515  ADI required write
    (0xE0, 0xD0), // video.cpp:1516  ADI required write
    (0x35, 0x40), // video.cpp:1519
    (0x36, 0xD9), // video.cpp:1520
    (0x37, 0x0A), // video.cpp:1521
    (0x38, 0x00), // video.cpp:1522
    (0x39, 0x2D), // video.cpp:1523
    (0x3A, 0x00), // video.cpp:1524
    (0x16, 0x38), // video.cpp:1526  0b00111000: [7]=0 output format 444, [5:4]=11 8-bit input,
    //                               [3:2]=10 style 1, [1]=0 no DDR, [0]=0 output colour space RGB
    (0x17, 0x62), // video.cpp:1533  0b01100010: 16:9 aspect [1]=1, both sync polarities inverted
    (0x3B, 0x80), // video.cpp:1535  automatic pixel repetition and VIC detection
    (0x3C, 0x00), // video.cpp:1536  VIC, left at 0 until mode_regs() sets it
    (0x48, 0x08), // video.cpp:1538  0b00001000: [6]=0 normal bus order, [4:3]=01 data right justified
    (0x49, 0xA8), // video.cpp:1542  ADI required write
    (0x40, 0x00), // video.cpp:1543
    (0x4A, 0x80), // video.cpp:1544  0b10000000: auto-calculate SPD checksum
    (0x4C, 0x00), // video.cpp:1545  ADI required write
    (0x55, 0x10), // video.cpp:1547  cfg.hdmi_game_mode == 0 -> 0b00010000 (RGB444 in AVI InfoFrame,
    //                               active format valid); the game-mode arm would be 0b00010010
    (0x56, 0x08), // video.cpp:1553  0b00001000 | (cfg.hdr ? .. : 0); cfg.hdr == 0 -> no HDR bits,
    //                               so [3:0]=1000 "active portion aspect = picture aspect"
    (0x57, 0x08), // video.cpp:1556  cfg.hdmi_game_mode == 0 -> no IT-content bit; ypbpr == false and
    //                               cfg.hdmi_limited == 0 and cfg.hdr == 0 -> 0b0001000, i.e.
    //                               [3:2]=10 RGB quantization range = full
    (0x59, 0x00), // video.cpp:1561  cfg.hdmi_game_mode == 0 -> 0x00 (IT content type graphics/none)
    (0x73, 0x01), // video.cpp:1565
    (0x96, 0xFF), // video.cpp:1567  clear all pending interrupts
    (0x94, 0x00), // video.cpp:1568  int0 = hdmi_has_int() ? 0xC0 : 0x00 (video.cpp:1465). We never arm
    //                               or service HPD/monitor-sense interrupts (ARCHITECTURE §4, "Not
    //                               done"), so we take the no-interrupt value unconditionally rather
    //                               than asking the fabric over the mailbox.
    (0xC9, 0x00), // video.cpp:1569  clear EDID request
    (0x99, 0x02), // video.cpp:1571  ADI required write
    (0x9B, 0x18), // video.cpp:1572  ADI required write
    (0x9F, 0x00), // video.cpp:1574  ADI required write
    (0xA1, 0x00), // video.cpp:1576  0b00000000: [6]=0, monitor-sense power-down enabled
    (0xA4, 0x08), // video.cpp:1578  ADI required write
    (0xA5, 0x04), // video.cpp:1579  ADI required write
    (0xA6, 0x00), // video.cpp:1580  ADI required write
    (0xA7, 0x00), // video.cpp:1581  ADI required write
    (0xA8, 0x00), // video.cpp:1582  ADI required write
    (0xA9, 0x00), // video.cpp:1583  ADI required write
    (0xAA, 0x00), // video.cpp:1584  ADI required write
    (0xAB, 0x40), // video.cpp:1585  ADI required write
    (0xB9, 0x00), // video.cpp:1587  ADI required write
    (0xBA, 0x60), // video.cpp:1589  0b01100000: [7:5]=011 input clock delay = no delay
    (0xBB, 0x00), // video.cpp:1599  ADI required write
    (0xDE, 0x9C), // video.cpp:1600  ADI required write
    (0xE2, 0x01), // video.cpp:1601  power down the CEC block
    (0xE4, 0x60), // video.cpp:1602  ADI required write
    (0xFA, 0x7D), // video.cpp:1603  number of times to search for a good phase
];

/// `hdmi_config_audio()`'s table, `video.cpp:1420-1453`.
///
/// Main calls this from the tail of `hdmi_config_init()` (`video.cpp:1612`),
/// so it always follows [`INIT`]. First row `(0xAF, 0x06)`, last row
/// `(0x09, 0x0A)`, 13 rows.
///
/// We drive no audio, but the table is not optional: register `0xAF` bit [1]
/// is what puts the transmitter in HDMI rather than DVI mode, and the CTS/N
/// values keep the packet engine consistent with the 74.25 MHz TMDS clock.
pub const AUDIO: &[(u8, u8)] = &[
    (0xAF, 0x06), // video.cpp:1422  0b00000100 | ((cfg.dvi_mode == 1) ? 0b00 : 0b10);
    //                               cfg.dvi_mode == 2 (cfg.cpp:603, unresolved because we never read
    //                               EDID) -> 0b10, i.e. [1]=1 HDMI mode. [7]=0 HDCP disabled.
    (0x0A, 0x00), // video.cpp:1430  0b00000000: [6:4]=000 audio select = I2S, [3:2]=00 audio mode
    (0x0B, 0x0E), // video.cpp:1433  0b00001110
    (0x0C, 0x04), // video.cpp:1435  0b00000100: [2]=1 I2S0 enable, [1:0]=00 standard I2S format
    (0x0D, 0x10), // video.cpp:1440  0b00010000: [4:0] I2S bit width for right-justified
    (0x14, 0x02), // video.cpp:1441  0b00000010: [3:0]=0010 audio word length = 16 bits
    (0x15, 0x20), // video.cpp:1442  (cfg.hdmi_audio_96k ? 0x80 : 0x00) | 0b0100000;
    //                               cfg.hdmi_audio_96k == 0 -> 0x20, [7:4]=0010 sampling rate 48 kHz,
    //                               [3:1]=000 input ID = 24-bit RGB 444 with separate syncs
    (0x01, 0x00), // video.cpp:1446  N value [19:16]
    (0x02, 0x18), // video.cpp:1447  cfg.hdmi_audio_96k == 0 -> 0x18, i.e. N = 6144 (48 kHz)
    (0x03, 0x00), // video.cpp:1448  N value [7:0]
    (0x07, 0x01), // video.cpp:1450  CTS [19:16]
    (0x08, 0x22), // video.cpp:1451  CTS [15:8]  -- CTS = 74250
    (0x09, 0x0A), // video.cpp:1452  CTS [7:0]
];

/// `hdmi_config_set_csc()`'s table, `video.cpp:1365-1397`, evaluated for the
/// no-ini defaults.
///
/// Main calls this immediately after [`AUDIO`] (`video.cpp:1613`). 28 rows,
/// first `(0x18, 0xA8)`, last `(0xC3, 0xFF)`.
///
/// The whole `!ypbpr` branch at `video.cpp:1226-1356` is a chain of 4x4 matrix
/// multiplies over the identity matrix `hdmi_full_coeffs` (`video.cpp:1189`),
/// and at the defaults above every factor is itself the identity:
///
/// - hue: `cfg.video_hue == 0` -> `cos_hue == 1`, `sin_hue == 0`, and the nine
///   `mat_hue` terms at `video.cpp:1281-1291` collapse to the identity.
/// - saturation: `cfg.video_saturation == 100` -> `s == 1`, so
///   `sr == sg == sb == 0` (`video.cpp:1297-1299`) and `mat_saturation` is the
///   identity.
/// - brightness/contrast: `50`/`50` -> `b == 0`, `c == 1`, `t == 0`
///   (`video.cpp:1235-1236, 1311-1313`), identity again.
/// - gain/offset: `"1, 0, 1, 0, 1, 0"` -> unit gains, zero offsets.
/// - `csc.compress(2.0f)` (`video.cpp:1342`, `mat4x4.h:64-90`) only rescales
///   when some component exceeds 2.0; the largest here is 1.0, so it is a
///   no-op.
/// - `cfg.hdmi_limited == 0` -> neither limited-range matrix is applied
///   (`video.cpp:1345-1348`).
///
/// So `csc_int16[] = int16_t(comp * 2048)` (`video.cpp:1354`) is
/// `[2048, 0, 0, 0,  0, 2048, 0, 0,  0, 0, 2048, 0]`: an unscaled pass-through
/// with 2048 = 1.0 in the chip's `0xA0` "[-2 .. 2]" mode.
pub const CSC: &[(u8, u8)] = &[
    // Coefficients, channel A. csc_int16[0] == 2048 -> high byte 0x08.
    (0x18, 0xA8), // video.cpp:1366  0b10100000 | ((csc_int16[0] >> 8) & 0b00011111) = 0xA0 | 0x08;
    //                               the 0xA0 selects the chip's [-2 .. 2] coefficient range
    (0x19, 0x00), // video.cpp:1367  csc_int16[0] & 0xFF
    (0x1A, 0x00), // video.cpp:1368  csc_int16[1] >> 8,   csc_int16[1] == 0
    (0x1B, 0x00), // video.cpp:1369  csc_int16[1] & 0xFF
    (0x1C, 0x00), // video.cpp:1370  csc_int16[2] >> 8,   csc_int16[2] == 0
    (0x1D, 0x00), // video.cpp:1371  csc_int16[2] & 0xFF
    (0x1E, 0x00), // video.cpp:1372  csc_int16[3] >> 8,   csc_int16[3] == 0 (channel A offset)
    (0x1F, 0x00), // video.cpp:1373  csc_int16[3] & 0xFF
    // Coefficients, channel B. csc_int16[5] == 2048 -> high byte 0x08.
    (0x20, 0x00), // video.cpp:1375  csc_int16[4] >> 8,   csc_int16[4] == 0
    (0x21, 0x00), // video.cpp:1376  csc_int16[4] & 0xFF
    (0x22, 0x08), // video.cpp:1377  csc_int16[5] >> 8
    (0x23, 0x00), // video.cpp:1378  csc_int16[5] & 0xFF
    (0x24, 0x00), // video.cpp:1379  csc_int16[6] >> 8,   csc_int16[6] == 0
    (0x25, 0x00), // video.cpp:1380  csc_int16[6] & 0xFF
    (0x26, 0x00), // video.cpp:1381  csc_int16[7] >> 8,   csc_int16[7] == 0 (channel B offset)
    (0x27, 0x00), // video.cpp:1382  csc_int16[7] & 0xFF
    // Coefficients, channel C. csc_int16[10] == 2048 -> high byte 0x08.
    (0x28, 0x00), // video.cpp:1384  csc_int16[8] >> 8,   csc_int16[8] == 0
    (0x29, 0x00), // video.cpp:1385  csc_int16[8] & 0xFF
    (0x2A, 0x00), // video.cpp:1386  csc_int16[9] >> 8,   csc_int16[9] == 0
    (0x2B, 0x00), // video.cpp:1387  csc_int16[9] & 0xFF
    (0x2C, 0x08), // video.cpp:1388  csc_int16[10] >> 8
    (0x2D, 0x00), // video.cpp:1389  csc_int16[10] & 0xFF
    (0x2E, 0x00), // video.cpp:1390  csc_int16[11] >> 8,  csc_int16[11] == 0 (channel C offset)
    (0x2F, 0x00), // video.cpp:1391  csc_int16[11] & 0xFF
    // Output clamps. clipMin/clipMax are computed at video.cpp:1361-1362.
    (0xC0, 0x00), // video.cpp:1393  clipMin >> 8;   cfg.hdmi_limited == 0 -> clipMin = 0x000
    (0xC1, 0x00), // video.cpp:1394  clipMin & 0xFF
    (0xC2, 0x0F), // video.cpp:1395  clipMax >> 8;   cfg.hdmi_limited == 0 -> clipMax = 0xFFF, the
    //                               12-bit maximum (0xEB0 would be the limited-range clamp)
    (0xC3, 0xFF), // video.cpp:1396  clipMax & 0xFF
];

/// Power the TMDS output up: `tmds_power(1)` writes `0x41 = 0x10`.
///
/// `video.cpp:2742` (`uint8_t val = on ? 0x10 : 0x50;`). The same pair is row
/// 3 of [`INIT`] (`video.cpp:1508`), so the bulk table already leaves the chip
/// powered up and this constant exists for `hdmi --off` symmetry and for a
/// re-power without a full reconfigure.
pub const POWER_UP: (u8, u8) = (0x41, 0x10);

/// Power the TMDS output down: `tmds_power(0)` writes `0x41 = 0x50`.
///
/// `video.cpp:2742`. This is the only write `hdmi --off` performs
/// (`docs/ARCHITECTURE.md` §4).
pub const POWER_DOWN: (u8, u8) = (0x41, 0x50);

/// `pr_flags` for "manual pixel repetition", `video.cpp:1700`.
///
/// `hdmi_config_set_mode()` picks between three values (`video.cpp:1698-1700`):
/// `0` when `cfg.direct_video && is_menu()`, [`PR_FLAGS_MANUAL_2X`] when the
/// mode's `pr` field is non-zero, and this one otherwise. `cfg.direct_video`
/// defaults to 0 (`cfg.cpp:594`) and we never program the menu core's direct
/// video path, so the first branch cannot be reached here; the other two are
/// both live and the caller's `pr` chooses between them.
const PR_FLAGS_MANUAL: u8 = 0b0100_0000;

/// `pr_flags` for "manual pixel repetition with 2x clock", `video.cpp:1699`.
///
/// Both modes this crate ships have `pr == 0` (`vmodes[0]` at `video.cpp:127`
/// and `vmodes[6]` at `video.cpp:133`), so nothing selects this value today.
/// It exists so that [`mode_regs`] is total over the `pr` field a `Modeline`
/// carries: `vmodes[14]` (`video.cpp:141`) is a `pr == 1` mode, and sending
/// `0x3B = 0x40` for one would tell the chip 1x repetition on a 2x-clock
/// mode — wrong TMDS timing, and a blank screen with nothing else to see.
const PR_FLAGS_MANUAL_2X: u8 = 0b0100_1000;

/// The three registers `hdmi_config_set_mode()` writes after `UIO_SET_VIDEO`,
/// `video.cpp:1702-1712`.
///
/// `hpol` and `vpol` are the mode's sync polarities, `vic` its CEA VIC and
/// `pr` its pixel-repetition flag. Both preset modes leave `hpol` and `vpol`
/// at their zero initialisation
/// (`docs/ARCHITECTURE.md` §3: `vmodes[]` rows carry no polarity, only custom
/// modes set it), which is exactly why this write matters: the polarity the
/// monitor sees comes from the `0x17` sync-invert bits, not from the wire.
///
/// `0x17 = 0b00000010 | sync_invert` (`video.cpp:1710`), where `sync_invert`
/// gets `1 << 5` when `hpol == 0` and `1 << 6` when `vpol == 0`
/// (`video.cpp:1702-1704`). Bit [1] is the 16:9 aspect-ratio flag and Main
/// sets it unconditionally here.
///
/// Main wraps those three writes in a cache we do not have: it keeps the last
/// `sync_invert`, `pr_flags` and `vic_mode` in three file statics and returns
/// before writing anything when all three are unchanged (`video.cpp:1706`,
/// stored at `:1724-1726`, cleared by `hdmi_invalidate_mode_cache()` at
/// `:1684-1689`). We drop it because we set the mode once per invocation and
/// the three writes are idempotent, so there is no repeat to suppress.
/// `tests/golden/adv7513.cpp` keeps the cache, and the golden test asserts a
/// repeated call there writes nothing, so the difference stays on the record.
///
/// Takes scalars rather than a `&Modeline` only because `crate::video` is
/// still a stub on this branch; they are exactly the four
/// `vmode_custom_param_t` fields the C reads, so the wrapper a later task adds
/// cannot silently drop one. See this module's tests for the expected values.
pub fn mode_regs(hpol: u8, vpol: u8, vic: u8, pr: u8) -> [(u8, u8); 3] {
    // video.cpp:1698-1700. The `cfg.direct_video && is_menu()` arm is not
    // reachable here; see PR_FLAGS_MANUAL.
    let pr_flags = if pr != 0 {
        PR_FLAGS_MANUAL_2X
    } else {
        PR_FLAGS_MANUAL
    };

    // video.cpp:1702-1704
    let mut sync_invert: u8 = 0;
    if hpol == 0 {
        sync_invert |= 1 << 5;
    }
    if vpol == 0 {
        sync_invert |= 1 << 6;
    }

    [
        (0x17, 0b0000_0010 | sync_invert), // video.cpp:1710
        (0x3B, pr_flags),                  // video.cpp:1711
        (0x3C, vic),                       // video.cpp:1712
    ]
}

/// Every bulk write `hdmi_config_init()` performs, in the order it performs
/// them: [`INIT`], then [`AUDIO`], then [`CSC`].
///
/// The order is `video.cpp:1606-1613` — the `init_data[]` loop, then the
/// `hdmi_config_audio()` call, then the `hdmi_config_set_csc()` call. Chip
/// registers are order-sensitive (`0x41` powers the output up part-way through
/// [`INIT`], and the CSC coefficients must land before the first active line),
/// so callers must not reorder or deduplicate this.
pub fn writes() -> impl Iterator<Item = (u8, u8)> {
    INIT.iter().chain(AUDIO).chain(CSC).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Registers that appear twice in one table, with the values they carried.
    fn duplicate_regs(table: &[(u8, u8)]) -> Vec<(u8, u8, u8)> {
        let mut dups = Vec::new();
        for (i, &(reg, val)) in table.iter().enumerate() {
            for &(other_reg, other_val) in &table[i + 1..] {
                if other_reg == reg {
                    dups.push((reg, val, other_val));
                }
            }
        }
        dups
    }

    #[test]
    fn init_table_matches_source() {
        // video.cpp:1499-1604: 51 (address, value) rows.
        assert_eq!(INIT.len(), 51);
        assert_eq!(INIT[0], (0x98, 0x03)); // video.cpp:1500, C octal `03`
        assert_eq!(INIT[INIT.len() - 1], (0xFA, 0x7D)); // video.cpp:1603
    }

    #[test]
    fn audio_table_matches_source() {
        // video.cpp:1420-1453: 13 rows.
        assert_eq!(AUDIO.len(), 13);
        assert_eq!(AUDIO[0], (0xAF, 0x06)); // video.cpp:1422, cfg.dvi_mode == 2 -> HDMI mode
        assert_eq!(AUDIO[AUDIO.len() - 1], (0x09, 0x0A)); // video.cpp:1452
    }

    #[test]
    fn csc_table_matches_source() {
        // video.cpp:1365-1397: 28 rows.
        assert_eq!(CSC.len(), 28);
        assert_eq!(CSC[0], (0x18, 0xA8)); // video.cpp:1366
        assert_eq!(CSC[CSC.len() - 1], (0xC3, 0xFF)); // video.cpp:1396
    }

    #[test]
    fn csc_is_an_unscaled_pass_through() {
        // The evaluated matrix is csc_int16 = [2048,0,0,0, 0,2048,0,0, 0,0,2048,0]
        // (video.cpp:1354 over the identity), so exactly three coefficient
        // registers are non-zero: the diagonal high bytes.
        assert_eq!(CSC[0], (0x18, 0xA0 | 0x08)); // channel A gain, high byte + range select
        assert_eq!(CSC[10], (0x22, 0x08)); // channel B gain, high byte
        assert_eq!(CSC[20], (0x2C, 0x08)); // channel C gain, high byte

        let non_zero: Vec<u8> = CSC
            .iter()
            .filter(|&&(reg, val)| (0x18..=0x2F).contains(&reg) && val != 0)
            .map(|&(reg, _)| reg)
            .collect();
        assert_eq!(non_zero, vec![0x18, 0x22, 0x2C]);

        // cfg.hdmi_limited == 0 -> clamps wide open, video.cpp:1361-1362.
        assert_eq!(
            &CSC[24..],
            &[(0xC0, 0x00), (0xC1, 0x00), (0xC2, 0x0F), (0xC3, 0xFF)]
        );
    }

    #[test]
    fn init_powers_the_chip_up_exactly_once() {
        // video.cpp:1508. If this row were ever duplicated or dropped the
        // output would be dark with no other symptom.
        let power_rows: Vec<(u8, u8)> = INIT
            .iter()
            .copied()
            .filter(|&(reg, _)| reg == 0x41)
            .collect();
        assert_eq!(power_rows, vec![(0x41, 0x10)]);
        assert_eq!(power_rows[0], POWER_UP);
    }

    #[test]
    fn init_arms_no_interrupts() {
        // video.cpp:1568 writes `int0`, which is
        // `hdmi_has_int() ? 0xC0 : 0x00` (video.cpp:1465): 0xC0 on a core that
        // routes the ADV7513 interrupt pin, 0x00 on one that does not. We
        // never make that query and never service HPD or monitor sense
        // (docs/ARCHITECTURE.md section 4, "Not done"), so we write the
        // no-interrupt mask unconditionally.
        //
        // This is the one byte in all 92 where we knowingly differ from Main.
        // The assertion is here so that "fixing" it to 0xC0 has to be a
        // deliberate change: arming INT1 without also servicing it, and
        // without the mailbox query that decides whether the pin even exists,
        // would leave the chip flagging interrupts nobody clears.
        assert!(INIT.contains(&(0x94, 0x00)));
        assert!(!INIT.contains(&(0x94, 0xC0)));
    }

    #[test]
    fn power_constants_match_tmds_power() {
        // video.cpp:2742: `uint8_t val = on ? 0x10 : 0x50;` on register 0x41.
        assert_eq!(POWER_UP, (0x41, 0x10));
        assert_eq!(POWER_DOWN, (0x41, 0x50));
        assert_eq!(POWER_UP.0, POWER_DOWN.0);
    }

    #[test]
    fn no_register_is_written_twice_within_a_table() {
        // None of the three C arrays repeats an address, so a repeat here
        // would be a transcription slip, and the second value would silently
        // win on the wire.
        assert_eq!(duplicate_regs(INIT), vec![]);
        assert_eq!(duplicate_regs(AUDIO), vec![]);
        assert_eq!(duplicate_regs(CSC), vec![]);
    }

    #[test]
    fn mode_regs_for_720p() {
        // vmodes[0], video.cpp:127: VIC 4, pr 0. hpol/vpol stay 0 for presets
        // (ARCHITECTURE §3), so both invert bits are set:
        // 0b00000010 | (1 << 5) | (1 << 6) == 0x62.
        assert_eq!(
            mode_regs(0, 0, 4, 0),
            [(0x17, 0x62), (0x3B, 0x40), (0x3C, 4)]
        );
    }

    #[test]
    fn mode_regs_for_480p() {
        // vmodes[6], video.cpp:133: VIC 1, pr 0.
        let regs = mode_regs(0, 0, 1, 0);
        assert_eq!(regs[2], (0x3C, 1));
        // Only the VIC differs from 720p; 0x17 and 0x3B are polarity- and
        // pixel-repetition-driven, and neither changes between the presets.
        assert_eq!(regs[0], (0x17, 0x62));
        assert_eq!(regs[1], (0x3B, 0x40));
    }

    #[test]
    fn mode_regs_sync_invert_bits_follow_polarity() {
        // video.cpp:1702-1704, each branch exercised on its own.
        assert_eq!(mode_regs(1, 1, 4, 0)[0], (0x17, 0b0000_0010));
        assert_eq!(mode_regs(0, 1, 4, 0)[0], (0x17, 0b0010_0010));
        assert_eq!(mode_regs(1, 0, 4, 0)[0], (0x17, 0b0100_0010));
        assert_eq!(mode_regs(0, 0, 4, 0)[0], (0x17, 0b0110_0010));
    }

    #[test]
    fn mode_regs_pixel_repetition_is_the_manual_branch() {
        // video.cpp:1700: cfg.direct_video == 0 (cfg.cpp:594) and pr == 0 for
        // both presets, so pr_flags is 0b01000000 and never 0.
        assert_eq!(PR_FLAGS_MANUAL, 0b0100_0000);
        for vic in [1u8, 4u8] {
            assert_eq!(mode_regs(0, 0, vic, 0)[1], (0x3B, 0b0100_0000));
        }
    }

    #[test]
    fn mode_regs_takes_the_2x_clock_branch_for_a_pixel_repeat_mode() {
        // video.cpp:1699: `else if (vm->param.pr != 0) pr_flags = 0b01001000`.
        // No mode this crate ships sets pr, but vmodes[14] (video.cpp:141)
        // does, and a wrapper over a Modeline must not be able to drop the
        // field: 0x40 on a 2x-clock mode is wrong TMDS timing and a dark
        // screen.
        assert_eq!(PR_FLAGS_MANUAL_2X, 0b0100_1000);
        assert_eq!(mode_regs(0, 0, 4, 1)[1], (0x3B, 0x48));

        // pr changes nothing else in the triple.
        let with_pr = mode_regs(0, 0, 4, 1);
        let without = mode_regs(0, 0, 4, 0);
        assert_eq!(with_pr[0], without[0]);
        assert_eq!(with_pr[2], without[2]);

        // Any non-zero pr takes it; the C tests `!= 0`, not `== 1`.
        assert_eq!(mode_regs(0, 0, 4, 2)[1], (0x3B, 0x48));
    }

    #[test]
    fn writes_yields_the_three_tables_in_mains_order() {
        // video.cpp:1606-1613.
        let all: Vec<(u8, u8)> = writes().collect();
        assert_eq!(all.len(), INIT.len() + AUDIO.len() + CSC.len());
        assert_eq!(all.len(), 51 + 13 + 28);

        assert_eq!(&all[..INIT.len()], INIT);
        assert_eq!(&all[INIT.len()..INIT.len() + AUDIO.len()], AUDIO);
        assert_eq!(&all[INIT.len() + AUDIO.len()..], CSC);

        // Spot-check the seams so a reordering is caught even if the lengths
        // happened to stay the same.
        assert_eq!(all[0], (0x98, 0x03));
        assert_eq!(all[50], (0xFA, 0x7D));
        assert_eq!(all[51], (0xAF, 0x06));
        assert_eq!(all[63], (0x09, 0x0A));
        assert_eq!(all[64], (0x18, 0xA8));
        assert_eq!(all[91], (0xC3, 0xFF));
    }

    #[test]
    fn mode_regs_override_the_bulk_tables_values() {
        // INIT leaves 0x3B in automatic pixel repetition / VIC detection
        // (video.cpp:1535) and 0x3C at 0 (video.cpp:1536); the mode write is
        // what switches to manual and supplies the VIC. This is Main's own
        // sequencing, not a transcription artefact, so the "duplicate"
        // addresses across tables are intentional.
        assert!(INIT.contains(&(0x3B, 0x80)));
        assert!(INIT.contains(&(0x3C, 0x00)));
        assert_eq!(mode_regs(0, 0, 4, 0)[1], (0x3B, 0x40));
        assert_eq!(mode_regs(0, 0, 4, 0)[2], (0x3C, 0x04));

        // 0x17 carries the same 0x62 in both places (video.cpp:1533 and the
        // evaluated video.cpp:1710), so the mode write is idempotent for it.
        assert!(INIT.contains(&(0x17, 0x62)));
        assert_eq!(mode_regs(0, 0, 4, 0)[0], (0x17, 0x62));
    }

    #[test]
    fn chip_address_is_the_main_register_map() {
        assert_eq!(CHIP_ADDR, 0x39); // video.cpp:1470
    }
}
