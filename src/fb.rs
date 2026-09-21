//! `UIO_SET_FBUF`: the ten-word burst that points the fabric's frame reader at
//! the Linux framebuffer, and the text of the kernel module's `mode` knob.
//!
//! This is step 3 of `docs/ARCHITECTURE.md` §1, specified in §5 and transcribed
//! from `video_fb_enable()` (`video.cpp:3474-3543`) and
//! `fb_write_module_params()` (`video.cpp:3459-3471`) in Main_MiSTer at commit
//! `6cda9cc`. Everything here is pure arithmetic: the module composes words and
//! a string and does no I/O at all. The caller ([`crate::mailbox`] and
//! [`crate::hw`]) sends them.
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

// video.cpp:36 - `#define FB_SIZE (1920*1080)`; the pixel count of one buffer.
// Only used to step between buffers, which we never do (we are always buffer
// 0), but it is part of the address formula so it is transcribed with it.
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
/// forward by one 4 KiB page.** That page belongs to the `MiSTer_fb` kernel
/// module, which is the only consumer of buffer 0; the pixels it exposes as
/// `/dev/fb0` begin at `FB_ADDR + 4096`, so that is where the fabric's frame
/// reader must be pointed or the picture would be one page of header out of
/// step with what fbcon draws. Main's other use of the same address agrees
/// verbatim: the `video_cmd` path hard-codes `uint32_t addr = FB_ADDR + 4096;`
/// (`video.cpp:4282`) because that path is always buffer 0. Buffers 1 and 2 are
/// never handed to the kernel driver — Main maps and paints them itself — so
/// they carry no header page and get no skip.
///
/// For buffer 0: `0x22000000 + 0 + 0x1000 = 0x22001000`.
const fn fb_addr(n: u32) -> u32 {
    FB_ADDR + (FB_SIZE * 4 * n) + if n == 0 { 4096 } else { 0 }
}

/// The ten payload words of `UIO_SET_FBUF` that switch the display to
/// `/dev/fb0`, for buffer 0 with direct video off.
///
/// Transcribed from `video.cpp:3491-3511`. The C reads its arguments out of two
/// globals and the current mode:
///
/// - `fb_width` / `fb_height` — the framebuffer's own geometry, set by
///   `video_fb_config()` as `fb_width = v_cur.item[1] / fb_scale_x` and
///   `fb_height = v_cur.item[5] / fb_scale_y` (`video.cpp:3575-3576`).
/// - `v_cur.item[1]` and `v_cur.item[5]` — the mode's horizontal and vertical
///   active pixel counts, `hact` and `vact` (`video.cpp:2011,2015`; a preset
///   mode copies them out of `vmodes[n].vpar` at `video.cpp:2028`). They are
///   what the scaled-right and scaled-bottom words are derived from.
///
/// With no `MiSTer.ini` every `cfg` field is zero (`cfg.cpp:594` memsets it),
/// so `cfg.fb_size == 0`; `video_fb_config()` then picks `fb_scale = 1` for any
/// mode whose `hact * vact` fits in `FB_SIZE` (`video.cpp:3562-3568`), and
/// `fb_scale_y` equals `fb_scale` because our modes have `pr == 0`
/// (`video.cpp:3573`). Both of our modes fit (1280*720 = 921_600 and
/// 640*480 = 307_200, against `FB_SIZE` = 2_073_600), so in practice
/// `width == hact` and `height == vact`. They stay separate arguments because
/// that is what the C computes, and a downscaled mode would make them differ.
///
/// `xoff`/`yoff` (`video.cpp:3494-3499`) are zero: they are non-zero only when
/// `cfg.direct_video` is set, and we never set it (`docs/ARCHITECTURE.md` §5).
///
/// The words, in order:
///
/// | # | Value | C |
/// |---|-------|---|
/// | 1 | `0x8016`, format and enable | `video.cpp:3502` |
/// | 2 | `fb_addr & 0xFFFF`, base address low word | `:3503` |
/// | 3 | `fb_addr >> 16`, base address high word | `:3504` |
/// | 4 | `width` | `:3505` |
/// | 5 | `height` | `:3506` |
/// | 6 | `0`, scaled left | `:3507` |
/// | 7 | `hact - 1`, scaled right | `:3508` |
/// | 8 | `0`, scaled top | `:3509` |
/// | 9 | `vact - 1`, scaled bottom | `:3510` |
/// | 10 | `width * 4`, stride in bytes | `:3511` |
pub const fn enable_words(width: u16, height: u16, hact: u16, vact: u16) -> [u16; 10] {
    let addr = fb_addr(FB_BUFFER);

    [
        // video.cpp:3502 - `spi_w((uint16_t)(FB_EN | FB_FMT_RxB | FB_FMT_8888))`
        FB_FORMAT_WORD,
        // video.cpp:3503 - `spi_w((uint16_t)fb_addr)`, the cast keeping [15:0]
        (addr & 0xFFFF) as u16,
        // video.cpp:3504 - `spi_w(fb_addr >> 16)`
        (addr >> 16) as u16,
        // video.cpp:3505 - `spi_w(fb_width)`
        width,
        // video.cpp:3506 - `spi_w(fb_height)`
        height,
        // video.cpp:3507 - `spi_w(xoff)`, and xoff == 0 with direct video off
        0,
        // video.cpp:3508 - `spi_w(xoff + v_cur.item[1] - 1)`. wrapping_sub
        // rather than `- 1` so that a nonsense hact of 0 truncates to 0xFFFF
        // the way C's int arithmetic would, instead of panicking in a debug
        // build; panics are banned on the hardware paths.
        hact.wrapping_sub(1),
        // video.cpp:3509 - `spi_w(yoff)`, and yoff == 0 with direct video off
        0,
        // video.cpp:3510 - `spi_w(yoff + v_cur.item[5] - 1)`
        vact.wrapping_sub(1),
        // video.cpp:3511 - `spi_w(fb_width * 4)`, four bytes per pixel. The C
        // computes in int and spi_w truncates to 16 bits; wrapping_mul is that
        // same truncation. 1920*4 = 7680 still fits, so no real mode wraps.
        width.wrapping_mul(4),
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
/// `width` and `height` are the framebuffer's geometry — the same
/// `fb_width`/`fb_height` as words 4 and 5 of [`enable_words`]
/// (`video.cpp:3461-3462`).
///
/// Note the stride here is computed in `int` in the C and is not truncated to
/// 16 bits the way word 10 of the burst is; for every mode we support they are
/// the same number.
pub fn mode_param_line(width: u16, height: u16) -> String {
    // video.cpp:3468 - `8888` (the pixel format) and `1` (red/blue swapped) are
    // literals in the C, and `width * 4` is the stride in bytes.
    let stride = u32::from(width) * 4;
    format!("8888 1 {width} {height} {stride}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // vmodes[0], video.cpp:127:
    //     { { 1280, 110, 40, 220, 720, 5, 5, 20 }, 74.25, 4, 0 }  //0 1280x720@60
    // vpar is copied into item[1..8] (video.cpp:2028), so hact = item[1] = 1280
    // and vact = item[5] = 720. video_fb_config() with cfg.fb_size == 0 and
    // pr == 0 leaves fb_scale = 1, because 1280*720 = 921_600 <= FB_SIZE
    // (1920*1080 = 2_073_600), so fb_width = 1280 and fb_height = 720
    // (video.cpp:3560-3576).
    const P720_WIDTH: u16 = 1280;
    const P720_HEIGHT: u16 = 720;
    const P720_HACT: u16 = 1280;
    const P720_VACT: u16 = 720;

    // vmodes[6], video.cpp:133:
    //     { { 640, 16, 96, 48, 480, 10, 2, 33 }, 25.175, 1, 0 }  //6 640x480@60
    // Same reasoning: 640*480 = 307_200 <= FB_SIZE and pr == 0, so fb_scale = 1
    // and fb_width/fb_height are the active area.
    const P480_WIDTH: u16 = 640;
    const P480_HEIGHT: u16 = 480;
    const P480_HACT: u16 = 640;
    const P480_VACT: u16 = 480;

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
    fn enable_burst_720p_word_by_word() {
        let w = enable_words(P720_WIDTH, P720_HEIGHT, P720_HACT, P720_VACT);

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
        let w = enable_words(P480_WIDTH, P480_HEIGHT, P480_HACT, P480_VACT);

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
        assert_eq!(
            mode_param_line(P720_WIDTH, P720_HEIGHT),
            "8888 1 1280 720 5120\n"
        );
    }

    #[test]
    fn mode_param_line_480p() {
        assert_eq!(
            mode_param_line(P480_WIDTH, P480_HEIGHT),
            "8888 1 640 480 2560\n"
        );
    }

    #[test]
    fn mode_param_line_agrees_with_the_burst() {
        // The kernel knob and the fabric burst describe the same buffer, so
        // width, height and stride have to match words 4, 5 and 10.
        for (width, height, hact, vact) in [
            (P720_WIDTH, P720_HEIGHT, P720_HACT, P720_VACT),
            (P480_WIDTH, P480_HEIGHT, P480_HACT, P480_VACT),
        ] {
            let w = enable_words(width, height, hact, vact);
            let line = mode_param_line(width, height);
            assert_eq!(line, format!("8888 1 {} {} {}\n", w[3], w[4], w[9]));
            // ...including the trailing newline the C's format string carries.
            assert!(line.ends_with('\n'));
        }
    }
}
