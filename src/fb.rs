//! `UIO_SET_FBUF`: the ten-word burst that points the fabric's frame reader at
//! the Linux framebuffer, and the text of the kernel module's `mode` knob.
//!
//! This is step 3 of `docs/ARCHITECTURE.md` §1, specified in §5 and transcribed
//! from `video_fb_enable()` (`video.cpp:3474-3543`), `video_fb_config()`
//! (`video.cpp:3556-3584`) and `fb_write_module_params()`
//! (`video.cpp:3459-3471`) in Main_MiSTer at commit `6cda9cc`. Everything here
//! is pure arithmetic: the module composes words and a string and does no I/O
//! at all. The caller ([`crate::mailbox`] and [`crate::hw`]) sends them.
//!
//! # What the caller has to do around these words
//!
//! `video_fb_enable()` does not send the burst unconditionally. It first sends
//! the opcode and looks at the word the fabric clocks back:
//!
//! ```text
//! int res = spi_uio_cmd_cont(UIO_SET_FBUF);   // video.cpp:3480
//! if (res) { ...send the burst... }
//! else     { printf("Core doesn't support HPS frame buffer\n"); }
//! DisableIO();                                // video.cpp:3539, on both paths
//! ```
//!
//! `spi_uio_cmd_cont()` (`spi.cpp:106-110`) is `EnableIO()` followed by one
//! `spi_w(opcode)`, and it returns that transfer's **reply word**, i.e. GPI
//! `[15:0]` sampled during the opcode transfer (`fpga_io.cpp:688-720`, see
//! `docs/ARCHITECTURE.md` §2 step 4). So the semantics T2.2 has to honour are:
//!
//! - reply word **non-zero** — the core has an HPS frame buffer; send the
//!   payload words from [`enable_words`] (or [`disable_words`]).
//! - reply word **zero** — the core does not support it. Main prints "Core
//!   doesn't support HPS frame buffer" (`video.cpp:3535`) and sends no payload
//!   at all. We should do the same: no payload, drop `SSPI_IO_EN`, and report
//!   it rather than push ten words at a core that will not read them.
//!
//! Either way the enable bit is dropped afterwards, on both branches.
//!
//! Ordering: the fabric burst goes first, then the sysfs line. Main writes the
//! module parameters only for buffer 0 and only after the ten words are out
//! (`video.cpp:3514-3518`).
//!
//! # The `cfg` defaults this module leans on
//!
//! Two `MiSTer.ini` settings reach into these words: `cfg.fb_size` picks the
//! framebuffer's scale ([`FbGeometry::for_mode`]) and `cfg.direct_video` shifts
//! the scaled window (`video.cpp:3494-3499`). We never write a `MiSTer.ini`, so
//! both are at their unset value, and it is worth being exact about why that is
//! zero rather than waving at the `memset`:
//!
//! `cfg_parse()` does start with `memset(&cfg, 0, sizeof(cfg))` (`cfg.cpp:594`),
//! but the very next lines assign seventeen non-zero scalar defaults and four
//! string defaults (`cfg.cpp:595-618`) — among them `cfg.csync = 1`
//! (`cfg.cpp:595`) and `cfg.dvi_mode = 2` (`cfg.cpp:603`), both of which the
//! ADV7513 path cares about. **"Zeroed, therefore zero" is not a rule that holds
//! for `cfg` in general; do not copy it to another module.** What holds here is
//! narrower and was checked field by field: `fb_size` and `direct_video` appear
//! in `cfg.cpp` only as ini keys (`cfg.cpp:71` and `cfg.cpp:74`) and are never
//! assigned a default, so with no `MiSTer.ini` they really are 0.

// video.cpp:36 - `#define FB_SIZE (1920*1080)`; the pixel count of one buffer.
// It is both the step between buffers in the address formula and the threshold
// `video_fb_config()` compares a mode's area against, so it earns its name.
const FB_SIZE: u32 = 1920 * 1080;

// video.cpp:37 - `#define FB_ADDR (0x20000000 + (32*1024*1024))`, commented
// "512mb + 32mb(Core's fb)": the top of the DDR window Linux does not use,
// plus the 32 MiB the core's own frame buffer lives in.
//   0x20000000 + 0x02000000 = 0x22000000
const FB_ADDR: u32 = 0x2000_0000 + (32 * 1024 * 1024);

// The format field. video.cpp:39-44 documents the bits:
//   [2:0] 011=8bpp(palette) 100=16bpp 101=24bpp 110=32bpp
//   [3]   0=16bits 565, 1=16bits 1555
//   [4]   0=RGB, 1=BGR (for 16/24/32 modes)
//   [5]   TBD
//
// video.cpp:49 - `#define FB_FMT_8888 0b00110`: bits [2:0] = 110 = 32bpp,
// bit [3] = 0 (only meaningful for the 16-bit formats), bit [4] = 0.
const FB_FMT_8888: u16 = 0b0_0110; // = 0x06

// video.cpp:51 - `#define FB_FMT_RxB 0b10000`: bit [4] = 1, i.e. the red and
// blue channels are swapped relative to FB_FMT_8888's plain RGB.
const FB_FMT_RXB: u16 = 0b1_0000; // = 0x10

// video.cpp:52 - `#define FB_EN 0x8000`: the frame reader's enable flag.
const FB_EN: u16 = 0x8000;

/// Word 1 of the enable burst: `FB_EN | FB_FMT_RxB | FB_FMT_8888`
/// (`video.cpp:3502`).
///
///   `0x8000 | 0b1_0000 | 0b0_0110` = `0x8000 | 0x10 | 0x06` = `0x8016`
const FB_FORMAT_WORD: u16 = FB_EN | FB_FMT_RXB | FB_FMT_8888;

/// The buffer the Linux framebuffer driver owns.
///
/// Main uses buffers 1 and 2 for the menu's background images, which it paints
/// itself through its own mapping of the same DDR region (`menu_bgn` is 1 or 2,
/// `video.cpp:3948`). We only ever want `/dev/fb0`, which is buffer 0.
const FB_BUFFER: u32 = 0;

/// Base address of buffer `n`, transcribed from `video.cpp:3491`:
///
/// ```text
/// uint32_t fb_addr = FB_ADDR + (FB_SIZE * 4 * n) + (n ? 0 : 4096);
/// ```
///
/// Four bytes per pixel, so each buffer is `FB_SIZE * 4` bytes and buffer `n`
/// starts that far into the region.
///
/// The `n ? 0 : 4096` term is the interesting one: **buffer 0 alone is pushed
/// forward by one 4 KiB page.** That page is the kernel driver's *palette*
/// page, not a header: `MiSTer_fb` points `info->pseudo_palette` at the base
/// of the reservation and starts the pixels one page later
/// (`MiSTer_fb.c:241`, `:269-270` — `smem_start = fb_res->start + 4096`,
/// `screen_base = fb_base + 4096`). So `/dev/fb0`'s pixels begin at
/// `FB_ADDR + 4096` and that is where the fabric's frame reader must point, or
/// it scans one page out of step with what fbcon draws.
///
/// There is nothing for anyone to write there on this path. The palette page
/// only matters in the 8bpp `PAL8` format, which we never select — in `8888`
/// it is simply skipped by both sides. Do not go looking for a header the
/// fabric expects; there isn't one. Main's other use of the same address agrees
/// verbatim: the `video_cmd` path hard-codes `uint32_t addr = FB_ADDR + 4096;`
/// (`video.cpp:4282`) because that path is always buffer 0. Buffers 1 and 2 are
/// never handed to the kernel driver — Main maps and paints them itself — so
/// they carry no palette page and get no skip.
///
/// For buffer 0: `0x22000000 + 0 + 0x1000 = 0x22001000`.
const fn fb_addr(n: u32) -> u32 {
    FB_ADDR + (FB_SIZE * 4 * n) + if n == 0 { 4096 } else { 0 }
}

/// Everything about the picture that the `UIO_SET_FBUF` burst needs: the
/// framebuffer's own size and the active area of the mode it is displayed in.
///
/// **These are two different pairs of numbers and they are not
/// interchangeable.** The C draws words 4, 5 and 10 from the globals
/// `fb_width`/`fb_height` (`video.cpp:3505`, `:3506`, `:3511`) and words 7 and 9
/// from `v_cur.item[1]`/`v_cur.item[5]` (`video.cpp:3508`, `:3510`), and
/// `video_fb_config()` sets the first pair to the second **divided by
/// `fb_scale`** (`video.cpp:3575-3576`). For the two modes we carry the divisor
/// is 1 and the pairs coincide, which is exactly why they are carried here as
/// one value with named fields rather than as four positional `u16`s that a
/// caller could transpose without any test noticing.
///
/// Build one with [`FbGeometry::for_mode`], which is the C's own rule; the
/// fields are private so there is no second way to get them out of step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbGeometry {
    /// `fb_width`: the framebuffer's width in pixels (`video.cpp:3575`).
    /// Words 4 and 10.
    width: u16,
    /// `fb_height`: the framebuffer's height in pixels (`video.cpp:3576`).
    /// Word 5.
    height: u16,
    /// `v_cur.item[1]`: the video mode's horizontal active pixels
    /// (`video.cpp:2011`; a preset copies it out of `vmodes[n].vpar[0]` at
    /// `video.cpp:2401`). Word 7.
    hact: u16,
    /// `v_cur.item[5]`: the video mode's vertical active lines
    /// (`video.cpp:2015`, `vmodes[n].vpar[4]`). Word 9.
    vact: u16,
}

impl FbGeometry {
    /// The geometry `video_fb_config()` (`video.cpp:3556-3576`) computes for a
    /// mode with `hact` x `vact` active pixels, where `pixel_repeat` is the
    /// mode's `pr` flag (`vmodes[n].pr`, copied to `v_cur.param.pr` at
    /// `video.cpp:2403`; 0 for both of the modes in `docs/ARCHITECTURE.md` §3).
    ///
    /// The C:
    ///
    /// ```text
    /// int fb_scale = cfg.fb_size;                       // :3560
    /// if (fb_scale <= 1) {                              // :3562
    ///     if (((v_cur.item[1] * v_cur.item[5]) > FB_SIZE)) fb_scale = 2;
    ///     else fb_scale = 1;                            // :3564-3567
    /// }
    /// else if (fb_scale == 3) fb_scale = 2;             // :3569
    /// else if (fb_scale > 4)  fb_scale = 4;             // :3570
    /// const int fb_scale_x = fb_scale;                  // :3572
    /// const int fb_scale_y = v_cur.param.pr == 0 ? fb_scale : fb_scale * 2;
    /// fb_width  = v_cur.item[1] / fb_scale_x;           // :3575
    /// fb_height = v_cur.item[5] / fb_scale_y;           // :3576
    /// ```
    ///
    /// `cfg.fb_size` is 0 for us (see the module note on `cfg` defaults), so
    /// only the `fb_scale <= 1` arm can run and the `== 3` / `> 4` arms at
    /// `video.cpp:3569-3570` are unreachable. They are therefore not
    /// transcribed: the scale is entirely a function of the mode's area, and a
    /// mode that does not fit in one `FB_SIZE` buffer is displayed at half
    /// resolution rather than overrunning the 8 MiB the buffer is given.
    pub const fn for_mode(hact: u16, vact: u16, pixel_repeat: bool) -> Self {
        // video.cpp:3564 - `(v_cur.item[1] * v_cur.item[5]) > FB_SIZE`. `item`
        // is `uint32_t item[32]` (video.cpp:189), so the C multiplies in 32
        // bits and so do we; two u16 factors reach at most 65535 * 65535 =
        // 4_294_836_225, which still fits a u32, so the product cannot wrap.
        let area = (hact as u32) * (vact as u32);

        // video.cpp:3565,3567 - the only two values `fb_scale` can take here.
        let fb_scale: u16 = if area > FB_SIZE { 2 } else { 1 };

        // video.cpp:3572 - `const int fb_scale_x = fb_scale;`
        let fb_scale_x = fb_scale;
        // video.cpp:3573 - `fb_scale_y = v_cur.param.pr == 0 ? fb_scale : fb_scale * 2`
        // A pixel-repeat mode carries half its lines in the mode table, so the
        // vertical divisor doubles. Max value 4, so no overflow.
        let fb_scale_y = if pixel_repeat { fb_scale * 2 } else { fb_scale };

        // video.cpp:3575-3576. The divisors are 1, 2 or 4 by construction —
        // never 0 — and dividing a u16 by a positive integer can only shrink
        // it, so neither division can trap or lose the value's range.
        Self {
            width: hact / fb_scale_x,
            height: vact / fb_scale_y,
            hact,
            vact,
        }
    }

    /// The framebuffer's width in pixels, for the caller's log line — Main's is
    /// `"Linux frame buffer: %dx%d, stride = %d bytes"` (`video.cpp:3513`).
    pub const fn width(&self) -> u16 {
        self.width
    }

    /// The framebuffer's height in pixels. See [`FbGeometry::width`].
    pub const fn height(&self) -> u16 {
        self.height
    }
}

/// The ten payload words of `UIO_SET_FBUF` that switch the display to
/// `/dev/fb0`, for buffer 0 with direct video off.
///
/// Transcribed from `video.cpp:3491-3511`.
///
/// `xoff`/`yoff` (`video.cpp:3494-3499`) are zero: they are non-zero only when
/// `cfg.direct_video` is set, and we never set it (`docs/ARCHITECTURE.md` §5,
/// and the module note on `cfg` defaults above for why it is 0).
///
/// The words, in order:
///
/// | # | Value | C |
/// |---|-------|---|
/// | 1 | `0x8016`, format and enable | `video.cpp:3502` |
/// | 2 | `fb_addr & 0xFFFF`, base address low word | `:3503` |
/// | 3 | `fb_addr >> 16`, base address high word | `:3504` |
/// | 4 | `fb_width` | `:3505` |
/// | 5 | `fb_height` | `:3506` |
/// | 6 | `0`, scaled left | `:3507` |
/// | 7 | `hact - 1`, scaled right | `:3508` |
/// | 8 | `0`, scaled top | `:3509` |
/// | 9 | `vact - 1`, scaled bottom | `:3510` |
/// | 10 | `fb_width * 4`, stride in bytes | `:3511` |
pub const fn enable_words(geom: &FbGeometry) -> [u16; 10] {
    let addr = fb_addr(FB_BUFFER);

    [
        // video.cpp:3502 - `spi_w((uint16_t)(FB_EN | FB_FMT_RxB | FB_FMT_8888))`
        FB_FORMAT_WORD,
        // video.cpp:3503 - `spi_w((uint16_t)fb_addr)`, the cast keeping [15:0]
        (addr & 0xFFFF) as u16,
        // video.cpp:3504 - `spi_w(fb_addr >> 16)`
        (addr >> 16) as u16,
        // video.cpp:3505 - `spi_w(fb_width)`
        geom.width,
        // video.cpp:3506 - `spi_w(fb_height)`
        geom.height,
        // video.cpp:3507 - `spi_w(xoff)`, and xoff == 0 with direct video off
        0,
        // video.cpp:3508 - `spi_w(xoff + v_cur.item[1] - 1)`. wrapping_sub
        // rather than `- 1` so that a nonsense hact of 0 truncates to 0xFFFF
        // the way C's int arithmetic would, instead of panicking in a debug
        // build; panics are banned on the hardware paths.
        geom.hact.wrapping_sub(1),
        // video.cpp:3509 - `spi_w(yoff)`, and yoff == 0 with direct video off
        0,
        // video.cpp:3510 - `spi_w(yoff + v_cur.item[5] - 1)`
        geom.vact.wrapping_sub(1),
        // video.cpp:3511 - `spi_w(fb_width * 4)`, four bytes per pixel. Note
        // this is the *framebuffer's* width, not the mode's: a downscaled mode
        // has a correspondingly shorter stride. The C computes in int and
        // spi_w truncates to 16 bits; wrapping_mul is that same truncation.
        // 1920*4 = 7680 still fits, so no real mode wraps.
        geom.width.wrapping_mul(4),
    ]
}

/// The single payload word of `UIO_SET_FBUF` that hands the display back to the
/// core's own frame buffer: `spi_w(0)` (`video.cpp:3527`), i.e. word 1 with
/// [`FB_EN`] clear.
///
/// The disable path sends nothing else — no address, no geometry.
pub const fn disable_words() -> [u16; 1] {
    [0]
}

/// The line `fb_write_module_params()` writes to
/// `/sys/module/MiSTer_fb/parameters/mode` (`video.cpp:3459-3471`):
///
/// ```text
/// fprintf(fp, "%d %d %d %d %d\n", 8888, 1, width, height, width * 4);
/// ```
///
/// The five fields are `<fmt> <rb> <width> <height> <stride>`, named by the
/// other writer of the same knob (`video.cpp:4307`), where `fmt` selects the
/// pixel format the driver registers (`8888` is the four-bytes-per-pixel case,
/// `video.cpp:4245-4247`) and `rb` is the red/blue swap that corresponds to
/// `FB_FMT_RxB` in the fabric's format word (`video.cpp:4271-4274`). Both are
/// literals here because the burst above is likewise fixed at `8888` plus
/// `RxB`.
///
/// Writing this re-registers `/dev/fb0` at the new geometry so fbcon follows.
/// It goes out **after** the fabric burst (`video.cpp:3514-3517`), and only for
/// buffer 0, which is the only buffer we use.
///
/// The width and height are the framebuffer's, not the mode's: the C copies
/// them straight out of `fb_width`/`fb_height` (`video.cpp:3461-3462`), the
/// same globals as words 4 and 5 of [`enable_words`].
///
/// Note the stride here is computed in `int` in the C and is not truncated to
/// 16 bits the way word 10 of the burst is; for every mode we support they are
/// the same number.
pub fn mode_param_line(geom: &FbGeometry) -> String {
    // video.cpp:3468 - `8888` (the pixel format) and `1` (red/blue swapped) are
    // literals in the C, and `width * 4` is the stride in bytes.
    let width = geom.width;
    let height = geom.height;
    let stride = u32::from(width) * 4;
    format!("8888 1 {width} {height} {stride}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // vmodes[0], video.cpp:127:
    //     { { 1280, 110, 40, 220, 720, 5, 5, 20 }, 74.25, 4, 0 }  //0 1280x720@60
    // vpar is copied into item[1..8] (video.cpp:2401), so hact = item[1] = 1280
    // and vact = item[5] = 720; the trailing 0 is `pr`.
    const P720: FbGeometry = FbGeometry::for_mode(1280, 720, false);

    // vmodes[6], video.cpp:133:
    //     { { 640, 16, 96, 48, 480, 10, 2, 33 }, 25.175, 1, 0 }  //6 640x480@60
    const P480: FbGeometry = FbGeometry::for_mode(640, 480, false);

    // vmodes[12], video.cpp:139:
    //     { { 1920, 48, 32, 80, 1440, 2, 4, 38 }, 185.203, 0, 0 }  //12 1920x1440@60
    // Not a mode we offer, but it is the C's own smallest example of the case
    // where the framebuffer is *not* the active area: 1920*1440 = 2_764_800 is
    // greater than FB_SIZE = 2_073_600, so video.cpp:3565 picks fb_scale = 2
    // and fb_width/fb_height come out 960x720 against hact/vact of 1920x1440.
    // It exists here to pin which of the two pairs feeds which word.
    const DOWNSCALED: FbGeometry = FbGeometry::for_mode(1920, 1440, false);

    // vmodes[14], video.cpp:141:
    //     { { 1280, 24, 16, 40, 1440, 3, 5, 33 }, 120.75, 0, 1 }  //14 2560x1440@60 (pr)
    // The other way for the pairs to differ: 1280*1440 = 1_843_200 fits, so
    // fb_scale = 1, but pr = 1 doubles the vertical divisor alone
    // (video.cpp:3573), giving 1280x720 against hact/vact of 1280x1440.
    const PIXEL_REPEAT: FbGeometry = FbGeometry::for_mode(1280, 1440, true);

    #[test]
    fn format_word_is_derived_not_pasted() {
        // video.cpp:49 - FB_FMT_8888 0b00110
        assert_eq!(FB_FMT_8888, 0x06);
        // video.cpp:51 - FB_FMT_RxB 0b10000
        assert_eq!(FB_FMT_RXB, 0x10);
        // video.cpp:52 - FB_EN 0x8000
        assert_eq!(FB_EN, 0x8000);
        // video.cpp:3502 - the OR of the three
        assert_eq!(FB_FORMAT_WORD, 0x8016);

        // Bit by bit against the field map at video.cpp:39-43: [2:0] = 110
        // (32bpp), [3] = 0, [4] = 1 (BGR), [5] = 0, and bit 15 = enable.
        assert_eq!(FB_FORMAT_WORD & 0b111, 0b110);
        assert_eq!(FB_FORMAT_WORD & (1 << 3), 0);
        assert_eq!(FB_FORMAT_WORD & (1 << 4), 1 << 4);
        assert_eq!(FB_FORMAT_WORD & (1 << 5), 0);
        assert_eq!(FB_FORMAT_WORD & 0x8000, 0x8000);
    }

    #[test]
    fn base_address_skips_one_page_for_buffer_zero_only() {
        // video.cpp:37 - 0x20000000 + 32 MiB
        assert_eq!(FB_ADDR, 0x2200_0000);
        // video.cpp:36 - 1920*1080 pixels, 4 bytes each = 0x7E9000 per buffer
        assert_eq!(FB_SIZE, 2_073_600);
        assert_eq!(FB_SIZE * 4, 0x7E_9000);

        // video.cpp:3491 - `FB_ADDR + (FB_SIZE * 4 * n) + (n ? 0 : 4096)`.
        // Buffer 0 is pushed past the MiSTer_fb header page, which matches the
        // hard-coded `FB_ADDR + 4096` of the other caller (video.cpp:4282).
        assert_eq!(fb_addr(0), 0x2200_1000);
        // Buffers 1 and 2 are Main's menu backgrounds: no header page, no skip.
        assert_eq!(fb_addr(1), 0x2200_0000 + 0x7E_9000);
        assert_eq!(fb_addr(2), 0x2200_0000 + 0xFD_2000);

        // Words 2 and 3 are that address split low/high (video.cpp:3503-3504).
        assert_eq!(fb_addr(0) & 0xFFFF, 0x1000);
        assert_eq!(fb_addr(0) >> 16, 0x2200);
    }

    #[test]
    fn geometry_follows_video_fb_config() {
        // Both modes we ship fit in one FB_SIZE buffer and have pr == 0, so
        // fb_scale is 1 (video.cpp:3567) and the framebuffer is the active
        // area: 1280*720 = 921_600 and 640*480 = 307_200, against 2_073_600.
        assert_eq!(
            P720,
            FbGeometry {
                width: 1280,
                height: 720,
                hact: 1280,
                vact: 720
            }
        );
        assert_eq!(
            P480,
            FbGeometry {
                width: 640,
                height: 480,
                hact: 640,
                vact: 480
            }
        );

        // video.cpp:3564 tests `>`, not `>=`, so a mode of exactly FB_SIZE
        // pixels is still scale 1: vmodes[8] 1920x1080 (video.cpp:135) has
        // 1920*1080 = 2_073_600 == FB_SIZE. This is the boundary the operator
        // choice turns on.
        const { assert!(1920 * 1080 == FB_SIZE) };
        assert_eq!(
            FbGeometry::for_mode(1920, 1080, false),
            FbGeometry {
                width: 1920,
                height: 1080,
                hact: 1920,
                vact: 1080
            }
        );

        // One pixel more and fb_scale becomes 2 (video.cpp:3565): vmodes[12],
        // 1920x1440 = 2_764_800 pixels.
        const { assert!(1920 * 1440 > FB_SIZE) };
        assert_eq!(
            DOWNSCALED,
            FbGeometry {
                width: 960,
                height: 720,
                hact: 1920,
                vact: 1440
            }
        );

        // pr == 1 halves the height alone (video.cpp:3573): vmodes[14].
        const { assert!(1280 * 1440 <= FB_SIZE) };
        assert_eq!(
            PIXEL_REPEAT,
            FbGeometry {
                width: 1280,
                height: 720,
                hact: 1280,
                vact: 1440
            }
        );
    }

    #[test]
    fn enable_burst_720p_word_by_word() {
        let w = enable_words(&P720);

        // 1: format and enable, FB_EN | FB_FMT_RxB | FB_FMT_8888
        //    = 0x8000 | 0x10 | 0x06                       (video.cpp:3502)
        assert_eq!(w[0], 0x8016);
        // 2: base address low word, 0x22001000 & 0xFFFF   (video.cpp:3503)
        assert_eq!(w[1], 0x1000);
        // 3: base address high word, 0x22001000 >> 16     (video.cpp:3504)
        assert_eq!(w[2], 0x2200);
        // 4: frame width, fb_width                        (video.cpp:3505)
        assert_eq!(w[3], 1280);
        // 5: frame height, fb_height                      (video.cpp:3506)
        assert_eq!(w[4], 720);
        // 6: scaled left, xoff = 0 with direct video off  (video.cpp:3507)
        assert_eq!(w[5], 0);
        // 7: scaled right, xoff + item[1] - 1 = 1280 - 1  (video.cpp:3508)
        assert_eq!(w[6], 1279);
        // 8: scaled top, yoff = 0                         (video.cpp:3509)
        assert_eq!(w[7], 0);
        // 9: scaled bottom, yoff + item[5] - 1 = 720 - 1  (video.cpp:3510)
        assert_eq!(w[8], 719);
        // 10: stride in bytes, fb_width * 4 = 1280 * 4    (video.cpp:3511)
        assert_eq!(w[9], 5120);

        // And the whole burst at once, as TASKS.md T1.4 states it.
        assert_eq!(
            w,
            [0x8016, 0x1000, 0x2200, 1280, 720, 0, 1279, 0, 719, 5120]
        );
    }

    #[test]
    fn enable_burst_480p_word_by_word() {
        let w = enable_words(&P480);

        // 1: unchanged — the format word does not depend on the mode.
        assert_eq!(w[0], 0x8016);
        // 2-3: unchanged — buffer 0 is at 0x22001000 whatever the mode.
        assert_eq!(w[1], 0x1000);
        assert_eq!(w[2], 0x2200);
        // 4: frame width, fb_width = 640                  (video.cpp:3505)
        assert_eq!(w[3], 640);
        // 5: frame height, fb_height = 480                (video.cpp:3506)
        assert_eq!(w[4], 480);
        // 6: scaled left, xoff = 0                        (video.cpp:3507)
        assert_eq!(w[5], 0);
        // 7: scaled right, item[1] - 1 = 640 - 1          (video.cpp:3508)
        assert_eq!(w[6], 639);
        // 8: scaled top, yoff = 0                         (video.cpp:3509)
        assert_eq!(w[7], 0);
        // 9: scaled bottom, item[5] - 1 = 480 - 1         (video.cpp:3510)
        assert_eq!(w[8], 479);
        // 10: stride in bytes, 640 * 4                    (video.cpp:3511)
        assert_eq!(w[9], 2560);

        assert_eq!(w, [0x8016, 0x1000, 0x2200, 640, 480, 0, 639, 0, 479, 2560]);
    }

    #[test]
    fn framebuffer_words_and_active_area_words_are_not_the_same_numbers() {
        // 720p and 480p cannot tell words 4/5/10 from words 7/9, because for
        // them fb_width == hact and fb_height == vact. These two cases can:
        // swapping either pair, or taking the stride from hact instead of
        // fb_width, changes at least one word here.

        // vmodes[12], 1920x1440 at fb_scale 2 (video.cpp:3565,3575-3576).
        let w = enable_words(&DOWNSCALED);
        assert_eq!(w[3], 960); // fb_width, not hact 1920
        assert_eq!(w[4], 720); // fb_height, not vact 1440
        assert_eq!(w[6], 1919); // hact - 1, not fb_width - 1 = 959
        assert_eq!(w[8], 1439); // vact - 1, not fb_height - 1 = 719
        assert_eq!(w[9], 3840); // fb_width * 4, not hact * 4 = 7680
        assert_eq!(
            w,
            [0x8016, 0x1000, 0x2200, 960, 720, 0, 1919, 0, 1439, 3840]
        );

        // vmodes[14], 1280x1440 with pr = 1: the vertical pair differs while
        // the horizontal pair does not (video.cpp:3573).
        let w = enable_words(&PIXEL_REPEAT);
        assert_eq!(w[3], 1280);
        assert_eq!(w[4], 720); // fb_height, not vact 1440
        assert_eq!(w[6], 1279);
        assert_eq!(w[8], 1439); // vact - 1, not fb_height - 1 = 719
        assert_eq!(w[9], 5120);
        assert_eq!(
            w,
            [0x8016, 0x1000, 0x2200, 1280, 720, 0, 1279, 0, 1439, 5120]
        );
    }

    #[test]
    fn disable_is_a_single_zero_word() {
        // video.cpp:3527 - `spi_w(0); // enable flag`
        assert_eq!(disable_words(), [0]);
        // Which is word 1 of the enable burst with FB_EN cleared, and nothing
        // after it.
        assert_eq!(disable_words()[0] & FB_EN, 0);
        assert_eq!(disable_words().len(), 1);
    }

    #[test]
    fn mode_param_line_720p() {
        // video.cpp:3468 - "%d %d %d %d %d\n", 8888, 1, width, height, width*4
        assert_eq!(mode_param_line(&P720), "8888 1 1280 720 5120\n");
    }

    #[test]
    fn mode_param_line_480p() {
        assert_eq!(mode_param_line(&P480), "8888 1 640 480 2560\n");
    }

    #[test]
    fn mode_param_line_takes_the_framebuffers_size_not_the_modes() {
        // video.cpp:3461-3462 copies fb_width/fb_height, so the downscaled
        // mode's knob says 960x720, not 1920x1440, and its stride is 960*4.
        assert_eq!(mode_param_line(&DOWNSCALED), "8888 1 960 720 3840\n");
        assert_eq!(mode_param_line(&PIXEL_REPEAT), "8888 1 1280 720 5120\n");
    }

    #[test]
    fn mode_param_line_agrees_with_the_burst() {
        // The kernel knob and the fabric burst describe the same buffer, so
        // width, height and stride have to match words 4, 5 and 10. This is a
        // consistency check between the two functions, not an independent
        // anchor — the literal expectations above are the anchor.
        for geom in [P720, P480, DOWNSCALED, PIXEL_REPEAT] {
            let w = enable_words(&geom);
            let line = mode_param_line(&geom);
            assert_eq!(line, format!("8888 1 {} {} {}\n", w[3], w[4], w[9]));
            // ...including the trailing newline the C's format string carries.
            assert!(line.ends_with('\n'));
            // And the accessors report the same pair.
            assert_eq!(geom.width(), w[3]);
            assert_eq!(geom.height(), w[4]);
        }
    }
}
