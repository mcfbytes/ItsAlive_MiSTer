//! Video modes, the PLL solver and the `UIO_SET_VIDEO` word composer.
//!
//! Everything here is pure arithmetic over plain values: no `unsafe`, no I/O,
//! no allocation. See `docs/ARCHITECTURE.md` §3.
//!
//! All constants and control flow are transcribed from Main_MiSTer at upstream
//! commit `6cda9cc`, `video.cpp`, and each carries its `file:line`.
//!
//! # Why the float literals look strange
//!
//! `findPLLpar()` and `setPLL()` mix `double` and `float` literals, and the
//! search they drive is a sequence of exact comparisons against those
//! literals. `0.05f` promotes to the `double` 0.05000000074505805969…, which
//! is *larger* than `0.05`; `0.95f` promotes to 0.94999998807907104492…,
//! which is *smaller* than `0.95`. Rounding them to the obvious decimal moves
//! both ends of the accepted band and can pick a different C for pixel clocks
//! that land near the edge. So the literals below are written as
//! `0.05_f32 as f64`, which is exactly what the C compiler does, and the cast
//! is left visible so a reviewer can see it was meant.
//!
//! `50.f` and `1500.f` are exactly representable in `f32`, so they promote to
//! plain `50.0` and `1500.0` and are written that way.

/// Number of payload words in a `UIO_SET_VIDEO` burst, opcode excluded.
///
/// Eight timing words, then `item[9..=20]`: the six odd entries contribute one
/// word each and the six even entries two each, so 8 + 6 + 12 = 26
/// (`video.cpp:2268-2291`).
pub const SET_VIDEO_WORDS: usize = 26;

/// One row of Main's `vmodes[]` (`video.cpp:117-123`, `:125-140`).
///
/// `vmodes[]` rows carry timings, `Fpix`, the VIC and a pixel-repeat flag
/// only. `hpol`/`vpol` are **not** in the table: they are set solely by
/// custom-mode parsing (`video.cpp:2338-2341`, `:2378-2379`), so for a preset
/// they keep the zero initialisation of `vmode_custom_t`. Both fields are
/// carried here anyway, because `set_video_words` and the ADV7513 mode
/// registers both read them, and both are zero for our two presets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Modeline {
    /// Horizontal active pixels, `vpar[0]` / `item[1]`.
    pub hact: u16,
    /// Horizontal front porch, `vpar[1]` / `item[2]`.
    pub hfp: u16,
    /// Horizontal sync width, `vpar[2]` / `item[3]`.
    pub hs: u16,
    /// Horizontal back porch, `vpar[3]` / `item[4]`.
    pub hbp: u16,
    /// Vertical active lines, `vpar[4]` / `item[5]`.
    pub vact: u16,
    /// Vertical front porch, `vpar[5]` / `item[6]`.
    pub vfp: u16,
    /// Vertical sync width, `vpar[6]` / `item[7]`.
    pub vs: u16,
    /// Vertical back porch, `vpar[7]` / `item[8]`.
    pub vbp: u16,
    /// Pixel clock in MHz, `vmode_t::Fpix`. Feeds [`solve_pll`].
    pub f_pix_mhz: f64,
    /// CEA VIC, `vmode_t::vic_mode`. Goes to ADV7513 register `0x3C`.
    pub vic: u8,
    /// Pixel repetition flag, `vmode_t::pr`. Shifted into bit 15 of word 1.
    pub pr: u8,
    /// Hsync polarity, `vmode_custom_param_t::hpol` (`item[21]`). Zero for
    /// every preset; see the type comment.
    pub hpol: u8,
    /// Vsync polarity, `vmode_custom_param_t::vpol` (`item[22]`). Zero for
    /// every preset; see the type comment.
    pub vpol: u8,
}

/// 1280x720@60, `vmodes[0]` (`video.cpp:127`).
///
/// `{ { 1280, 110, 40, 220, 720, 5, 5, 20 }, 74.25, 4, 0 }`. The default.
pub const MODE_720P: Modeline = Modeline {
    hact: 1280,
    hfp: 110,
    hs: 40,
    hbp: 220,
    vact: 720,
    vfp: 5,
    vs: 5,
    vbp: 20,
    f_pix_mhz: 74.25,
    vic: 4,
    pr: 0,
    hpol: 0,
    vpol: 0,
};

/// 640x480@60, `vmodes[6]` (`video.cpp:133`).
///
/// `{ { 640, 16, 96, 48, 480, 10, 2, 33 }, 25.175, 1, 0 }`. Selected with
/// `--mode 480p`, for sinks that dislike 720p.
pub const MODE_480P: Modeline = Modeline {
    hact: 640,
    hfp: 16,
    hs: 96,
    hbp: 48,
    vact: 480,
    vfp: 10,
    vs: 2,
    vbp: 33,
    f_pix_mhz: 25.175,
    vic: 1,
    pr: 0,
    hpol: 0,
    vpol: 0,
};

/// The modes `--mode` accepts, in the order `--help` should list them.
///
/// Two rows only: `docs/ARCHITECTURE.md` §3 says no other modes in v1.
pub const MODES: &[(&str, Modeline)] = &[("720p", MODE_720P), ("480p", MODE_480P)];

/// Look a mode up by its `--mode` name, e.g. `"720p"`.
///
/// Returns `None` for anything not in [`MODES`]; the caller turns that into a
/// usage error so this module stays free of the crate's error type.
pub fn mode_by_name(name: &str) -> Option<Modeline> {
    MODES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, mode)| *mode)
}

/// `item[9..=20]`: the twelve-word PLL reconfiguration block.
///
/// `item[0]` of this struct is the C's `item[9]`, and so on to `item[11]` for
/// the C's `item[20]`. Only [`PllBlock::item`] goes on the wire; `c`, `m`, `k`
/// and `f_pix_mhz` are `setPLL()`'s locals, kept so the CLI can log the same
/// line Main prints (`video.cpp:299`) and so the golden test can check the
/// search result and not only its encoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PllBlock {
    /// `item[9]` through `item[20]`, in order.
    pub item: [u32; 12],
    /// The C divider, `setPLL()`'s `c`. Encoded into `item[14]`.
    pub c: u32,
    /// The M multiplier, `setPLL()`'s `m`. Encoded into `item[10]`.
    pub m: u32,
    /// The fractional multiplier, `setPLL()`'s `k`. Sent as `item[20]`.
    pub k: u32,
    /// The pixel clock the solution actually produces, `v->Fpix`. Not sent;
    /// Main only prints it.
    pub f_pix_mhz: f64,
}

impl PllBlock {
    /// The value the C calls `item[i]`, for `i` in `9..=20`.
    ///
    /// The `set_video()` loop switches on the C index's parity, so the
    /// composer works in C indices and converts here.
    ///
    /// Private on purpose. `i - 9` underflows for `i < 9`: a panic in a debug
    /// build, an out-of-bounds index in a release one, and with
    /// `panic = "abort"` a SIGABRT whose exit code is not one
    /// `docs/ARCHITECTURE.md` §7 lists. The only call sites are
    /// [`set_video_words`]'s literal `9..21` and this module's tests, so the
    /// index is checkable by reading this file; callers outside it get
    /// [`PllBlock::item`], which cannot be handed a C index at all.
    fn c_item(&self, i: usize) -> u32 {
        self.item[i - 9]
    }
}

/// `0.05f` as the C's comparison sees it: a `float` literal promoted to
/// `double`, i.e. 0.05000000074505805969238281250, not 0.05
/// (`video.cpp:238`, `:282`).
const KO_LOW: f64 = 0.05_f32 as f64;

/// `0.95f` promoted to `double`: 0.94999998807907104492187500, not 0.95
/// (`video.cpp:238`, `:286`).
const KO_HIGH: f64 = 0.95_f32 as f64;

/// `1500.f` (`video.cpp:241`). Exactly representable, so it is just 1500.0.
const FVCO_MAX: f64 = 1500.0;

/// The PLL reference clock, MHz. `fvco *= 50.f` and `fvco / 50`
/// (`video.cpp:232-236`); `50.f` is exact, so this is plain 50.0.
const F_REF_MHZ: f64 = 50.0;

/// `2^32`, the scale `ko` is turned into `K` with (`video.cpp:293`).
const K_SCALE: f64 = 4294967296.0;

/// The largest C the opening `while ((Fout*c) < 400) c++;` search may reach.
///
/// **Not in the C.** Both copies of that loop (`video.cpp:227` and `:275`)
/// have no exit of their own: for an `Fout` too small for `Fout * c` ever to
/// reach 400, `c` runs past `UINT32_MAX` and wraps, and the loop never ends.
/// Upstream is safe because every call site hands it a `vmodes[]` row's
/// `Fpix`; [`solve_pll`] is `pub`, so the precondition is enforced here rather
/// than assumed (`docs/ARCHITECTURE.md` §2: bounded, always — a hang is worse
/// than an error exit).
///
/// 510 is the ceiling the rest of the transcription already implies:
/// `getPLLdiv()` packs a divider into two 8-bit half-counts
/// (`video.cpp:220-221`), and 511 is the first divider whose upper half-count,
/// `(div / 2) + 1` = 256, no longer fits in its byte. `400 / 510` is 0.784
/// MHz, so no pixel clock this refuses is one the C would have encoded
/// usefully anyway.
///
/// The *other* `c++` (`video.cpp:247`) needs no bound: it runs only while
/// `fvco <= 1500`, and `fvco` grows by about `Fout` each time, so from ~400
/// MHz it takes at most `1100 / 0.784` ≈ 1404 turns.
const C_MAX: u32 = 510;

/// `getPLLdiv()` (`video.cpp:218-222`).
///
/// Packs a divider into the counter form the Cyclone V PLL reconfiguration
/// block wants: the low byte and the next byte hold the two half-counts, and
/// an odd divider sets bit 17 and makes the upper half-count one larger so the
/// two sum back to `div`. Only the encoding is our business here; what the
/// fabric does with the bits is the FPGA's.
fn get_pll_div(div: u32) -> u32 {
    // video.cpp:220
    if div & 1 != 0 {
        return 0x20000 | (((div / 2) + 1) << 8) | (div / 2);
    }
    // video.cpp:221
    ((div / 2) << 8) | (div / 2)
}

/// `findPLLpar()` (`video.cpp:224-260`).
///
/// Searches upward from the smallest C that puts `Fvco = Fout * C` at or above
/// 400 MHz for a C whose fractional part `ko` is not in the PLL's forbidden
/// bands. Returns `Some((c, m, ko))`, or `None` for the C's `return 0` when
/// `Fvco` runs past 1500 MHz without a hit — and also when `f_out` is so small
/// that the opening search would not terminate ([`C_MAX`]), which the caller
/// handles the same way it handles `return 0`, by failing rather than by
/// spinning.
///
/// Transcribed statement for statement. Two things a reader trips over:
///
/// - `fvco` is *recomputed* as `(ko + m) * 50` before the range test
///   (`video.cpp:235-236`), so the 1500 MHz bound is tested against the
///   reconstructed Fvco, not against `Fout * c`. That is deliberate upstream
///   and is kept.
/// - `fvco / 50` is evaluated twice (`video.cpp:232-233`), once for the
///   truncating cast to M and once for the subtraction. Same value; written
///   the same way so the two lines stay side by side with the C.
fn find_pll_par(f_out: f64) -> Option<(u32, u32, f64)> {
    // video.cpp:226-227. The C_MAX bail is not in the C; see C_MAX.
    let mut c: u32 = 1;
    while (f_out * f64::from(c)) < 400.0 {
        c += 1;
        if c > C_MAX {
            return None;
        }
    }

    // video.cpp:229
    loop {
        // video.cpp:231-233. The C's `(uint32_t)` is a truncating cast; Rust's
        // `as u32` truncates identically for the finite non-negative values
        // this can produce, and saturates instead of being undefined if the
        // arithmetic ever went out of range.
        let mut fvco = f_out * f64::from(c);
        let m = (fvco / F_REF_MHZ) as u32;
        let ko = (fvco / F_REF_MHZ) - f64::from(m);

        // video.cpp:235-236
        fvco = ko + f64::from(m);
        fvco *= F_REF_MHZ;

        // video.cpp:238
        if ko != 0.0 && (ko <= KO_LOW || ko >= KO_HIGH) {
            // video.cpp:241-245
            if fvco > FVCO_MAX {
                return None;
            }
            // video.cpp:247
            c += 1;
        } else {
            // video.cpp:251-254
            return Some((c, m, ko));
        }
    }
}

/// `setPLL()` (`video.cpp:262-315`): solve for the PLL block that produces
/// `f_out_mhz` from the 50 MHz reference.
///
/// When [`find_pll_par`] gives up, the fallback (`video.cpp:273-291`) redoes
/// the first candidate and snaps `ko` to zero at whichever end it fell off,
/// carrying into M at the top end. That path costs exactness but always
/// terminates, which is why it exists.
///
/// Returns `None` for a pixel clock the C's own search cannot handle: zero,
/// negative, infinite, NaN, or so small that the opening `while (Fout*c) < 400`
/// would run `c` past `UINT32_MAX` and wrap ([`C_MAX`]). Upstream has no such
/// guard and simply spins or overflows there, because a `vmodes[]` row always
/// carries a real clock; this one is `pub`, and
/// `docs/ARCHITECTURE.md` §2 says a search that cannot finish is an error and
/// never a hang. Every clock the C solves, this solves identically — the
/// golden vectors are the proof.
///
/// Note that the C's `return 0` from `findPLLpar()` is *not* one of those
/// failures: it is the ordinary "no exact parameters" case and takes the
/// fallback below, exactly as upstream.
pub fn solve_pll(f_out_mhz: f64) -> Option<PllBlock> {
    // Not in the C. `(fvco / 50) as u32` would saturate for an infinity and
    // give 0 for a NaN, and the fallback's `m++` would then run off the top of
    // a u32; upstream never sees either because `Fout` is a table literal.
    if !f_out_mhz.is_finite() || f_out_mhz <= 0.0 {
        return None;
    }

    // video.cpp:272
    let (c, m, ko) = match find_pll_par(f_out_mhz) {
        Some(found) => found,
        None => {
            // video.cpp:274-275, with the same C_MAX bail as find_pll_par:
            // this is the second copy of the unbounded loop.
            let mut c: u32 = 1;
            while (f_out_mhz * f64::from(c)) < 400.0 {
                c += 1;
                if c > C_MAX {
                    return None;
                }
            }

            // video.cpp:277-279
            let fvco = f_out_mhz * f64::from(c);
            let mut m = (fvco / F_REF_MHZ) as u32;
            let mut ko = (fvco / F_REF_MHZ) - f64::from(m);

            // Make sure K is in allowed range. video.cpp:281-289
            if ko <= KO_LOW {
                ko = 0.0;
            } else if ko >= KO_HIGH {
                // video.cpp:288 is a bare `m++`. Checked, not wrapping: for
                // any clock the guard at the top of this function admits, M is
                // far below u32::MAX and this never fires.
                m = m.checked_add(1)?;
                ko = 0.0;
            }

            (c, m, ko)
        }
    };

    // video.cpp:293. K is 1, not 0, when the fraction vanished: the reconfig
    // block reads 0 as "no fractional path at all".
    let k: u32 = if ko != 0.0 { (ko * K_SCALE) as u32 } else { 1 };

    // video.cpp:295-297
    let mut fvco = ko + f64::from(m);
    fvco *= F_REF_MHZ;
    let f_pix = fvco / f64::from(c);

    // video.cpp:301-312. item[9] is index 0 here.
    let item = [
        4,              // item[9]
        get_pll_div(m), // item[10]
        3,              // item[11]
        0x10000,        // item[12]
        5,              // item[13]
        get_pll_div(c), // item[14]
        9,              // item[15]
        2,              // item[16]
        8,              // item[17]
        7,              // item[18]
        7,              // item[19]
        k,              // item[20]
    ];

    Some(PllBlock {
        item,
        c,
        m,
        k,
        // video.cpp:314
        f_pix_mhz: f_pix,
    })
}

/// Variable refresh rate, bit 14 of word 1 (`video.cpp:2270`, `use_vrr`).
///
/// Always 0: VRR needs `MiSTer.ini` knobs and a core that asked for it, and we
/// send a fixed preset to the menu core.
const VRR: u8 = 0;

/// Build one timing word, `video.cpp:2270-2275`.
///
/// `i` is the C's `item[]` index, 1 through 8. Written as a function taking
/// `pr`/`vrr`/`pol` rather than folding the constant zeros in, so the shape of
/// the upstream expression survives.
fn timing_word(i: usize, value: u16, pr: u8, vrr: u8, hpol: u8, vpol: u8) -> u16 {
    match i {
        // video.cpp:2270
        1 => ((pr as u16) << 15) | ((vrr as u16) << 14) | value,
        // hsync polarity, video.cpp:2272. The C writes `!!v_cur.param.hpol`.
        3 => (((hpol != 0) as u16) << 15) | value,
        // vsync polarity, video.cpp:2274
        7 => (((vpol != 0) as u16) << 15) | value,
        // video.cpp:2275
        _ => value,
    }
}

/// Compose the 26 payload words of a `UIO_SET_VIDEO` burst, opcode excluded
/// (`video.cpp:2266-2291`).
///
/// Words 1..=8 are the timings with the flag bits ORed in; words 9..=20 of
/// `item[]` follow, odd C indices as one word with `0x4000` set and even C
/// indices as two words, low half then high half.
///
/// Main sends the timings from `v_fix`, not `v_cur` (`video.cpp:2270-2275`).
/// `v_fix` starts as a copy of `v_cur` (`video.cpp:2205`) and is only altered
/// by the `cfg.direct_video` branch (`:2206-2216`) and the VRR branch
/// (`:2226-2260`). Both are off for us, so `v_fix == v_cur` and the mode's own
/// timings go out unchanged.
///
/// # Polarity is zero on the wire for presets
///
/// `MODE_720P` and `MODE_480P` carry `hpol == vpol == 0`, because Main's
/// `vmodes[]` rows have no polarity fields at all and only custom-mode parsing
/// ever sets them (`video.cpp:2338-2341`, `:2378-2379`). So bit 15 of words 3
/// and 7 is clear here, and the polarity the monitor actually sees is fixed
/// afterwards by the ADV7513's `0x17` sync-invert bits (`video.cpp:1691-1727`,
/// `docs/ARCHITECTURE.md` §4). That looks like a bug on the wire and is not
/// one: do not "fix" it by setting the bits.
///
/// # The `0x8000` bit
///
/// Main sometimes ORs `0x8000` into the word for `item[9]`, gated on
/// `Fpix && cfg.vsync_adjust == 2 && !is_menu()` (`video.cpp:2285`). We are
/// always the menu case, so it is never set.
pub fn set_video_words(mode: &Modeline, pll: &PllBlock) -> [u16; SET_VIDEO_WORDS] {
    let mut out = [0u16; SET_VIDEO_WORDS];
    let mut n = 0usize;

    // video.cpp:2268-2277. item[1..=8].
    let timings = [
        mode.hact, mode.hfp, mode.hs, mode.hbp, mode.vact, mode.vfp, mode.vs, mode.vbp,
    ];
    for (offset, &value) in timings.iter().enumerate() {
        out[n] = timing_word(offset + 1, value, mode.pr, VRR, mode.hpol, mode.vpol);
        n += 1;
    }

    // video.cpp:2282-2291. item[9..21).
    for i in 9..21usize {
        let value = pll.c_item(i);
        if i & 1 != 0 {
            // video.cpp:2285, without the 0x8000 term: see the doc comment.
            out[n] = (value as u16) | 0x4000;
            n += 1;
        } else {
            // video.cpp:2288-2289. spi_w() takes a uint16_t, so the first
            // write is an implicit truncation to the low half.
            out[n] = value as u16;
            out[n + 1] = (value >> 16) as u16;
            n += 2;
        }
    }

    debug_assert_eq!(n, SET_VIDEO_WORDS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_literals_are_the_promoted_float_ones() {
        // If these ever became 0.05_f64 / 0.95_f64 the accepted band for ko
        // would move and some pixel clocks would pick a different C.
        const { assert!(KO_LOW > 0.05_f64, "0.05f promotes to more than 0.05") };
        const { assert!(KO_HIGH < 0.95_f64, "0.95f promotes to less than 0.95") };
        assert_eq!(KO_LOW.to_bits(), 0x3FA9_9999_A000_0000);
        assert_eq!(KO_HIGH.to_bits(), 0x3FEE_6666_6000_0000);
    }

    #[test]
    fn get_pll_div_matches_the_c() {
        // video.cpp:218-222, worked by hand for the two presets' dividers.
        assert_eq!(get_pll_div(8), 0x0404); // even: (4 << 8) | 4
        assert_eq!(get_pll_div(6), 0x0303);
        assert_eq!(get_pll_div(16), 0x0808);
        assert_eq!(get_pll_div(4), 0x0202);
        // odd: 0x20000 | (((div / 2) + 1) << 8) | (div / 2)
        assert_eq!(get_pll_div(3), 0x2_0201);
        assert_eq!(get_pll_div(7), 0x2_0403);
        assert_eq!(get_pll_div(9), 0x2_0504);
        assert_eq!(get_pll_div(15), 0x2_0807);
    }

    #[test]
    fn solve_pll_720p() {
        // Cross-checked against tests/golden/pll.json, which is generated by
        // compiling the C. Repeated here so a broken solver fails a unit test
        // too, not only the integration test.
        let pll = solve_pll(MODE_720P.f_pix_mhz).expect("720p has a real clock");
        assert_eq!(pll.c, 6);
        assert_eq!(pll.m, 8);
        assert_eq!(pll.k, 3_908_420_239);
        assert_eq!(pll.f_pix_mhz, 74.25);
        assert_eq!(
            pll.item,
            [
                4,
                0x0404,
                3,
                0x10000,
                5,
                0x0303,
                9,
                2,
                8,
                7,
                7,
                3_908_420_239
            ]
        );
    }

    #[test]
    fn solve_pll_480p() {
        let pll = solve_pll(MODE_480P.f_pix_mhz).expect("480p has a real clock");
        assert_eq!(pll.c, 16);
        assert_eq!(pll.m, 8);
        assert_eq!(pll.k, 240_518_168);
        assert_eq!(
            pll.item,
            [4, 0x0404, 3, 0x10000, 5, 0x0808, 9, 2, 8, 7, 7, 240_518_168]
        );
    }

    #[test]
    fn solve_pll_refuses_what_the_c_would_spin_on() {
        // Upstream's `while ((Fout*c) < 400) c++;` never ends for these, and
        // in a release build (overflow-checks off) `c` just wraps. Bounded,
        // always: docs/ARCHITECTURE.md §2.
        assert_eq!(solve_pll(0.0), None);
        assert_eq!(solve_pll(-74.25), None);
        assert_eq!(solve_pll(f64::NAN), None);
        assert_eq!(solve_pll(f64::INFINITY), None);
        assert_eq!(solve_pll(f64::NEG_INFINITY), None);
        // Positive but far too small for Fout * C_MAX to reach 400 MHz.
        assert_eq!(solve_pll(1e-9), None);
    }

    #[test]
    fn the_c_max_bound_refuses_nothing_a_real_clock_needs() {
        // 1 MHz is three orders of magnitude below any mode in vmodes[] and
        // still solves: C = 400 (1.0 * 400 is the first Fvco >= 400), M = 8,
        // ko = 0 so K is 1, and getPLLdiv(400) = (200 << 8) | 200.
        let pll = solve_pll(1.0).expect("1 MHz is inside the bound");
        assert_eq!((pll.c, pll.m, pll.k), (400, 8, 1));
        assert_eq!(pll.item[5], 0xC8C8);
        assert_eq!(pll.f_pix_mhz, 1.0);
    }

    #[test]
    fn c_item_indexes_in_c_coordinates() {
        let pll = solve_pll(MODE_720P.f_pix_mhz).expect("720p has a real clock");
        assert_eq!(pll.c_item(9), 4);
        assert_eq!(pll.c_item(10), 0x0404);
        assert_eq!(pll.c_item(14), 0x0303);
        assert_eq!(pll.c_item(20), pll.k);
    }

    #[test]
    fn set_video_burst_720p_word_by_word() {
        let pll = solve_pll(MODE_720P.f_pix_mhz).expect("720p has a real clock");
        let w = set_video_words(&MODE_720P, &pll);

        // k = 3908420239 = 0xE8F5C28F.
        let k_lo = 0xC28F;
        let k_hi = 0xE8F5;

        #[rustfmt::skip]
        let expected: [u16; SET_VIDEO_WORDS] = [
            // timings, video.cpp:2268-2277. No polarity bits: presets carry none.
            1280, 110, 40, 220, 720, 5, 5, 20,
            0x4004,                 // item[9]  = 4        | 0x4000
            0x0404, 0x0000,         // item[10] = 0x404
            0x4003,                 // item[11] = 3        | 0x4000
            0x0000, 0x0001,         // item[12] = 0x10000
            0x4005,                 // item[13] = 5        | 0x4000
            0x0303, 0x0000,         // item[14] = 0x303
            0x4009,                 // item[15] = 9        | 0x4000
            0x0002, 0x0000,         // item[16] = 2
            0x4008,                 // item[17] = 8        | 0x4000
            0x0007, 0x0000,         // item[18] = 7
            0x4007,                 // item[19] = 7        | 0x4000
            k_lo, k_hi,             // item[20] = K
        ];
        assert_eq!(w, expected);

        // Spelled out again as properties, so a copy-paste slip in the table
        // above cannot pass silently.
        assert_eq!(w.len(), 26);
        for (n, i) in (9..21usize).filter(|i| i & 1 != 0).enumerate() {
            // Odd C indices land at 8, 11, 14, 17, 20, 23.
            let at = 8 + n * 3;
            assert_eq!(
                w[at] & 0x4000,
                0x4000,
                "word for item[{i}] (burst index {at}) must have 0x4000 set"
            );
        }
        // The 0x8000 bit is gated on !is_menu(); we are always the menu.
        assert_eq!(w[8] & 0x8000, 0, "0x8000 must never be set on item[9]");
        // Sync polarity is zero on the wire for presets; the ADV7513's 0x17
        // fixes what the monitor sees.
        assert_eq!(w[2] & 0x8000, 0, "hpol bit must be clear for a preset");
        assert_eq!(w[6] & 0x8000, 0, "vpol bit must be clear for a preset");
        // pr and vrr both zero, so word 1 is bare hact.
        assert_eq!(w[0], 1280);
    }

    #[test]
    fn set_video_burst_480p_word_by_word() {
        let pll = solve_pll(MODE_480P.f_pix_mhz).expect("480p has a real clock");
        let w = set_video_words(&MODE_480P, &pll);

        // k = 240518168 = 0x0E560418.
        #[rustfmt::skip]
        let expected: [u16; SET_VIDEO_WORDS] = [
            640, 16, 96, 48, 480, 10, 2, 33,
            0x4004,
            0x0404, 0x0000,
            0x4003,
            0x0000, 0x0001,
            0x4005,
            0x0808, 0x0000,
            0x4009,
            0x0002, 0x0000,
            0x4008,
            0x0007, 0x0000,
            0x4007,
            0x0418, 0x0E56,
        ];
        assert_eq!(w, expected);
        assert_eq!(w[8] & 0x8000, 0);
    }

    #[test]
    fn polarity_and_pr_bits_go_where_the_c_puts_them() {
        // No preset sets these, but the composer must still place them, since
        // a later custom-mode path would depend on it. Synthesised mode, not
        // transcribed from vmodes[].
        let mode = Modeline {
            pr: 1,
            hpol: 1,
            vpol: 1,
            ..MODE_720P
        };
        let pll = solve_pll(mode.f_pix_mhz).expect("720p has a real clock");
        let w = set_video_words(&mode, &pll);
        assert_eq!(w[0], 0x8000 | 1280); // pr in bit 15 of word 1
        assert_eq!(w[2], 0x8000 | 40); // hpol in bit 15 of word 3
        assert_eq!(w[6], 0x8000 | 5); // vpol in bit 15 of word 7
        assert_eq!(w[1], 110); // and nowhere else
        assert_eq!(w[4], 720);
    }

    #[test]
    fn mode_lookup() {
        assert_eq!(mode_by_name("720p"), Some(MODE_720P));
        assert_eq!(mode_by_name("480p"), Some(MODE_480P));
        assert_eq!(mode_by_name("1080p"), None);
        assert_eq!(mode_by_name(""), None);
        assert_eq!(mode_by_name("720P"), None, "the C's names are lowercase");
    }

    #[test]
    fn preset_rows_match_vmodes() {
        // video.cpp:127 and :133, re-read off the table.
        assert_eq!(
            (
                MODE_720P.hact,
                MODE_720P.hfp,
                MODE_720P.hs,
                MODE_720P.hbp,
                MODE_720P.vact,
                MODE_720P.vfp,
                MODE_720P.vs,
                MODE_720P.vbp
            ),
            (1280, 110, 40, 220, 720, 5, 5, 20)
        );
        assert_eq!(MODE_720P.vic, 4);
        assert_eq!(MODE_720P.pr, 0);
        assert_eq!(
            (
                MODE_480P.hact,
                MODE_480P.hfp,
                MODE_480P.hs,
                MODE_480P.hbp,
                MODE_480P.vact,
                MODE_480P.vfp,
                MODE_480P.vs,
                MODE_480P.vbp
            ),
            (640, 16, 96, 48, 480, 10, 2, 33)
        );
        assert_eq!(MODE_480P.vic, 1);
        assert_eq!(MODE_480P.pr, 0);
        for (_, mode) in MODES {
            assert_eq!(mode.hpol, 0, "vmodes[] rows carry no polarity");
            assert_eq!(mode.vpol, 0, "vmodes[] rows carry no polarity");
        }
    }
}
