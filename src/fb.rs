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

use crate::{Error, Result};

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

// ---------------------------------------------------------------------------
// Reading the knob back, and where an image lands in what it reports
// ---------------------------------------------------------------------------

/// Bytes per pixel in the `8888` format the driver registers.
///
/// `setup_fb_info()`'s 32-bit arm sets `bits_per_pixel = 32`
/// (`MiSTer_fb.c:219`), and the stride it computes when the knob leaves the
/// field empty is `(width*4 + 255) & ~255` (`MiSTer_fb.c:152`): four bytes a
/// pixel in both places, and the same four [`mode_param_line`] multiplies by.
pub const BYTES_PER_PIXEL: u32 = 4;

/// The `format` value that selects those four bytes (`MiSTer_fb.c:227`, the
/// `else` arm's `format = 8888`, which is also what it normalises any
/// unrecognised value to).
pub const FORMAT_8888: u32 = 8888;

/// The red/blue swap flag, as [`mode_param_line`] always writes it.
pub const RB_SWAPPED: u32 = 1;

/// The five numbers `/sys/module/MiSTer_fb/parameters/mode` reports back.
///
/// # Why this is read rather than assumed
///
/// [`mode_param_line`] is what *asks* for a geometry, and a successful write
/// of it proves nothing. `mode_set()`'s whole body sits inside `if(p_fbdev)`
/// and it `return 0;` either way (`MiSTer_fb.c:343-361`), so on a kernel where
/// the driver never probed — no `MiSTer_fb` platform device, no reserved
/// region — the store succeeds, changes nothing, and reports success.
///
/// `mode_get()` is the other half of the same `kernel_param_ops`
/// (`MiSTer_fb.c:364-371`). It prints `"%u %u %u %u %u"` — `format rb width
/// height stride` — from the same five statics the store parsed into
/// (`MiSTer_fb.c:31`), and returns 0 bytes under the same `if(p_fbdev)`. So
/// reading the knob back is the only evidence available in Linux at all, and
/// an empty read is precisely the "the driver is not there" case rather than a
/// malformed one.
///
/// # What a non-empty read does *not* prove
///
/// It does not prove that `itsalive fb enable` ran, and it says nothing about
/// the fabric. `rb` defaults to 1 and `format` to 0 (`MiSTer_fb.c:31`), and
/// `setup_fb_info()` fills the rest in at probe — `if(!width) width = 640;
/// if(!height) height = 480; if(!stride) stride = (width*4 + 255) & ~255;`
/// (`MiSTer_fb.c:150-152`), which is 2560 — before the `else` arm rewrites
/// `format` to 8888 (`MiSTer_fb.c:227`). A board where nothing has ever
/// written this knob therefore reads back exactly `8888 1 640 480 2560`, which
/// passes every check in [`BlitPlan::new`] while the fabric's frame reader is
/// still on the core's own buffer and nothing scans out the bytes we write.
///
/// The one signal that says the core really has an HPS frame buffer switched
/// in is the `UIO_SET_FBUF` reply word, and that belongs to `fb enable`:
/// `image` opens no `/dev/mem` and asks the fabric nothing
/// (`docs/ARCHITECTURE.md` §5). So this read means "the driver probed, and
/// here is the layout it registered" — and `image` exiting 0 means the bytes
/// reached `/dev/fb0`, not that they are on a screen. On the rig only a
/// photograph settles that (`docs/PLAN.md` §3 step 6).
///
/// # The byte order this crate writes, and why it is BGRX
///
/// `setup_fb_info()`'s `8888` arm starts from **RGB**:
///
/// ```text
/// info->var.red.offset   = 0;      // MiSTer_fb.c:220
/// info->var.green.offset = 8;      // MiSTer_fb.c:221
/// info->var.blue.offset  = 16;     // MiSTer_fb.c:222
/// ```
///
/// and then, unconditionally on the flag, exchanges red and blue:
///
/// ```text
/// if(rb)
/// {
///     u32 tmp = info->var.red.offset;
///     info->var.red.offset = info->var.blue.offset;
///     info->var.blue.offset = tmp;
/// }                                // MiSTer_fb.c:230-234
/// ```
///
/// [`mode_param_line`] always writes `rb = 1` — the `1` is a literal there,
/// matching the `FB_FMT_RxB` bit of the fabric's format word
/// (`video.cpp:3502`), because both sides have to agree about the same bytes.
/// So the live layout is **red = 16, green = 8, blue = 0** in a 32-bit
/// little-endian word, which on ARM is the byte sequence **B, G, R, X**: byte
/// 0 of each pixel is blue, byte 3 is the unused padding byte.
///
/// Getting this backwards raises no error anywhere. The picture simply comes
/// out with red and blue exchanged, which on a photograph reads as blue faces
/// and looks like a bad source file rather than a format bug — so
/// [`BlitPlan::new`] refuses to paint unless the knob reports `8888` **and**
/// `rb == 1`. Any other combination means these four bytes mean something
/// else, and painting anyway is worse than exiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FbMode {
    /// `format`: `8888` is the four-byte case (`MiSTer_fb.c:227`).
    pub format: u32,
    /// `rb`: non-zero swaps red and blue (`MiSTer_fb.c:230`).
    pub rb: u32,
    /// `width`: visible pixels a row, `info->var.xres` (`MiSTer_fb.c:154`).
    pub width: u32,
    /// `height`: visible rows, `info->var.yres` (`MiSTer_fb.c:155`).
    pub height: u32,
    /// `stride`: **bytes** a row, `info->fix.line_length`
    /// (`MiSTer_fb.c:156`). Not in general `width * 4`; see [`BlitPlan`].
    pub stride: u32,
}

impl FbMode {
    /// How many bytes the driver has registered: `stride * height`.
    ///
    /// That is `setup_fb_info()`'s own `smem_len`
    /// (`info->fix.line_length * info->var.yres`, `MiSTer_fb.c:239`), which is
    /// the length `fb_sys_write` clamps a write against, so it is exactly what
    /// `--clear` has to zero and no more.
    pub const fn byte_len(&self) -> u64 {
        (self.stride as u64) * (self.height as u64)
    }
}

/// The stride the fabric's frame reader was told, for a framebuffer `width`
/// pixels wide: `fb_width * 4` (`video.cpp:3511`, word 10 of `UIO_SET_FBUF`,
/// transcribed in [`enable_words`]).
///
/// This is not readable from Linux — it lives in the frame reader's registers,
/// reachable only through the mailbox — so it is a rule rather than a
/// measurement, and [`stride_warning`] is what happens when the knob disagrees
/// with it.
pub const fn frame_reader_stride(width: u32) -> u64 {
    (width as u64) * (BYTES_PER_PIXEL as u64)
}

/// A line for stderr when the knob's stride is not the one the frame reader
/// walks DDR with, or `None` when the two agree.
///
/// Two strides decide where a pixel ends up and only one of them is readable:
/// `info->fix.line_length` (`MiSTer_fb.c:156`), which the knob reports and
/// which `fb_sys_write` clamps against, and word 10 of `UIO_SET_FBUF`
/// (`video.cpp:3511`), which is what the fabric scans out. [`mode_param_line`]
/// and [`enable_words`] are written together and both say `width * 4`, so for
/// any geometry `itsalive fb enable` programmed they are the same number.
///
/// They come apart when something else registered the fbdev — the driver's own
/// auto-pad, `if(!stride) stride = (width*4 + 255) & ~255;`
/// (`MiSTer_fb.c:152`), gives a 720-pixel row 3072 where the fabric is at 2880
/// — and then no blit is right for both: rows placed by the knob match the
/// fbdev byte for byte and shear diagonally across the screen.
///
/// This warns rather than refusing. The reported stride is the only number
/// `image` has, `width * 4` is a guess about a register it cannot read, and
/// §7's exit 2 means "bug in the caller", which a padded knob is not.
pub fn stride_warning(mode: &FbMode) -> Option<String> {
    let programmed = frame_reader_stride(mode.width);
    if u64::from(mode.stride) == programmed {
        return None;
    }
    Some(format!(
        "itsalive: the framebuffer reports a stride of {} bytes, but a frame reader \
         configured for a {}-pixel row scans {programmed} (video.cpp:3511); rows go where the \
         driver says, so if the two disagree the picture will shear: run `itsalive fb enable` \
         to write both",
        mode.stride, mode.width
    ))
}

/// The complaint for a knob that is not five numbers.
///
/// `mode_get` returns 0 bytes when the driver never probed, so the empty
/// string lands here too; the message names the fix rather than the parse.
fn geometry_unset(line: &str) -> Error {
    let trimmed = line.trim();
    Error::Usage(format!(
        "the kernel framebuffer geometry reads {trimmed:?}, not \
         \"format rb width height stride\": run `itsalive fb enable` first"
    ))
}

/// Parse what `mode_get` printed (`MiSTer_fb.c:364-371`) back into an
/// [`FbMode`].
///
/// Exactly five unsigned decimal fields, whitespace separated. The kernel's
/// `param_attr_show` appends a newline to whatever `.get` produced, so a
/// trailing one is expected rather than tolerated; `split_whitespace` eats it
/// along with any other spacing.
///
/// Every rejection is the same [`Error::Usage`] (exit 2, "bug in the caller"):
/// an empty knob, a short one and an unparseable one are all the one thing an
/// operator can act on, which is that `itsalive fb enable` has not run in this
/// boot. **A knob that cannot be *read at all* is a different failure** —
/// [`Error::Io`], exit 14 — and it is raised by the reader in
/// [`crate::hw`], not here: this function only ever sees bytes that arrived.
pub fn parse_mode_line(line: &str) -> Result<FbMode> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [format, rb, width, height, stride] = fields[..] else {
        return Err(geometry_unset(line));
    };
    let (Ok(format), Ok(rb), Ok(width), Ok(height), Ok(stride)) = (
        format.parse::<u32>(),
        rb.parse::<u32>(),
        width.parse::<u32>(),
        height.parse::<u32>(),
        stride.parse::<u32>(),
    ) else {
        return Err(geometry_unset(line));
    };
    Ok(FbMode {
        format,
        rb,
        width,
        height,
        stride,
    })
}

/// One row of a blit: where it goes, and which bytes of the source it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// Byte offset into `/dev/fb0` of this row's leftmost pixel.
    pub dest: u64,
    /// Byte offset into the source image of this row's leftmost pixel.
    pub src: usize,
    /// The row's length in bytes: the source's width times four.
    pub len: usize,
}

/// Where each row of a source image lands in the framebuffer the driver
/// reports.
///
/// Pure: building one decides the whole blit, including every refusal, before
/// `/dev/fb0` is opened. The bytes are [`FbMode`]'s BGRX8888 and this type
/// never inspects them.
///
/// # Addressing is by the reported stride, and what that is worth
///
/// Row `r` of the source starts at `(y + r) * stride + x * 4`, where `stride`
/// is `info->fix.line_length` exactly as the knob reported it
/// (`MiSTer_fb.c:156`).
///
/// **It is trusted because `fb enable` writes both sides of it, not because
/// the driver's padded stride would be paintable.** Two numbers decide where a
/// byte ends up, and only the first is readable from Linux:
///
/// - `info->fix.line_length`, what the knob reports and what `fb_sys_write`
///   clamps a write against (`smem_len = line_length * yres`,
///   `MiSTer_fb.c:239`); and
/// - word 10 of `UIO_SET_FBUF`, `fb_width * 4` (`video.cpp:3511`,
///   [`enable_words`]), the stride the fabric's frame reader walks DDR with —
///   the only one that decides what reaches the screen.
///
/// [`mode_param_line`] and [`enable_words`] go out together and both say
/// `width * 4`, so against any geometry this crate programmed the two agree
/// and addressing by the reported stride *is* addressing by the frame
/// reader's. Where they differ — the driver's auto-pad, `if(!stride) stride =
/// (width*4 + 255) & ~255;` (`MiSTer_fb.c:152`), which a module loaded with
/// `width=`/`height=` and no `stride=` gets, computes 3072 for a 720-pixel row
/// where the fabric is at 2880 — no blit can be right for both, and rows
/// placed by the knob match the fbdev byte for byte while shearing diagonally
/// on screen. [`stride_warning`] names that case on stderr; this type has only
/// the knob's number and uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlitPlan {
    /// The source's width in pixels.
    width: u32,
    /// The source's height in pixels.
    height: u32,
    /// Destination column of the source's left edge, in pixels.
    x: u32,
    /// Destination row of the source's top edge.
    y: u32,
    /// The framebuffer's stride in bytes, as the knob reported it.
    stride: u32,
    /// `width * BYTES_PER_PIXEL`, checked to fit a `usize`.
    row_bytes: usize,
}

impl BlitPlan {
    /// Plan a `src_w` x `src_h` source into `mode`, centred, or say why not.
    ///
    /// Every refusal here is [`Error::Usage`] (exit 2). They are all "the
    /// caller handed us something this command cannot paint", which is what
    /// `docs/ARCHITECTURE.md` §7 calls a bug in the caller, and none of them
    /// needs a device to be opened to decide.
    ///
    /// The checks, in the order they run:
    ///
    /// 1. **`format == 8888` and `rb == 1`.** Anything else means the four
    ///    bytes of a pixel are not B, G, R, X (see [`FbMode`]), so the bytes
    ///    we are about to write mean something other than what the caller
    ///    thinks. Painting garbage silently is worse than exiting.
    /// 2. **The reported geometry is addressable**: non-zero, and a stride of
    ///    at least `width * 4`. A shorter stride than the visible width is not
    ///    a layout this code knows how to address; the kernel would clamp the
    ///    tail of the write and leave a torn picture.
    /// 3. **The source fits.** `image` neither scales nor crops — the point of
    ///    the command is that the build host already resized — so a source
    ///    wider or taller than the screen is refused rather than trimmed.
    /// 4. **The row arithmetic fits a `usize`**, so [`BlitPlan::rows`] can
    ///    index a slice without any cast that could wrap on a 32-bit target.
    ///
    /// Then the centring, which is integer division of the slack:
    /// `x = (width - src_w) / 2` and `y = (height - src_h) / 2`. An odd
    /// remainder therefore goes to the right and to the bottom.
    pub fn new(mode: &FbMode, src_w: u32, src_h: u32) -> Result<Self> {
        if mode.format != FORMAT_8888 || mode.rb != RB_SWAPPED {
            return Err(Error::Usage(format!(
                "the framebuffer is registered as format {} rb {}, and `image` writes only \
                 BGRX8888 (format {FORMAT_8888}, rb {RB_SWAPPED}): run `itsalive fb enable` \
                 to set it",
                mode.format, mode.rb
            )));
        }
        if mode.width == 0 || mode.height == 0 {
            return Err(Error::Usage(format!(
                "the framebuffer geometry is {}x{}: run `itsalive fb enable` first",
                mode.width, mode.height
            )));
        }
        let fb_row = u64::from(mode.width) * u64::from(BYTES_PER_PIXEL);
        if u64::from(mode.stride) < fb_row {
            return Err(Error::Usage(format!(
                "the framebuffer stride is {} bytes but a {}-pixel row is {fb_row} bytes: \
                 that geometry is not one `image` can address",
                mode.stride, mode.width
            )));
        }
        if src_w == 0 || src_h == 0 {
            return Err(Error::Usage(format!(
                "the image is {src_w}x{src_h}: both dimensions must be at least 1"
            )));
        }
        if src_w > mode.width || src_h > mode.height {
            return Err(Error::Usage(format!(
                "the image is {src_w}x{src_h} and the framebuffer is {}x{}: `image` does not \
                 scale and does not crop, so resize the source on the build host",
                mode.width, mode.height
            )));
        }

        let too_big = || {
            Error::Usage(format!(
                "a {src_w}x{src_h} BGRX8888 image does not fit in this machine's address space"
            ))
        };
        let Ok(row_bytes) = usize::try_from(u64::from(src_w) * u64::from(BYTES_PER_PIXEL)) else {
            return Err(too_big());
        };
        let Ok(rows) = usize::try_from(src_h) else {
            return Err(too_big());
        };
        if row_bytes.checked_mul(rows).is_none() {
            return Err(too_big());
        }

        Ok(Self {
            width: src_w,
            height: src_h,
            // Integer division of the slack, so an odd remainder lands on the
            // right and on the bottom. Both subtractions are guarded by the
            // "the source fits" check above and cannot wrap.
            x: (mode.width - src_w) / 2,
            y: (mode.height - src_h) / 2,
            stride: mode.stride,
            row_bytes,
        })
    }

    /// The source's width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// The source's height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The destination column of the source's left edge, in pixels.
    pub const fn x(&self) -> u32 {
        self.x
    }

    /// The destination row of the source's top edge.
    pub const fn y(&self) -> u32 {
        self.y
    }

    /// One row of the source in bytes: `width * 4`.
    pub const fn row_bytes(&self) -> usize {
        self.row_bytes
    }

    /// Exactly how many bytes the source file must be: `width * height * 4`.
    ///
    /// Computed in `u64` so it is the true number even on a machine where it
    /// would not fit a `usize`; [`BlitPlan::new`] has already refused that
    /// case, so a plan that exists has a `source_len` a `usize` can hold.
    pub const fn source_len(&self) -> u64 {
        (self.width as u64) * (self.height as u64) * (BYTES_PER_PIXEL as u64)
    }

    /// Refuse a source whose length is not exactly [`BlitPlan::source_len`].
    ///
    /// Both numbers are in the message on purpose. The failing input this is
    /// written for is a mistyped `convert`/`ffmpeg` line on a build host —
    /// `-pix_fmt bgr24` instead of `bgra`, or a `--size` that does not match
    /// what was actually rendered — and for a **short** source the ratio
    /// between the two counts is what names it: three quarters of the expected
    /// length is the missing padding byte.
    ///
    /// A **long** source is reported as "longer than", not as a count, because
    /// [`crate::hw::read_source`] stops one byte past `source_len`: `actual`
    /// is then a lower bound and printing it as the file's size would be a
    /// lie. That cap is deliberate — a mistyped path or the wrong file on the
    /// left of a pipe must not have to fit in a 1 GB board's RAM to be
    /// diagnosed — and `ls -l` gives the exact number when the ratio is what
    /// the operator wants.
    pub fn check_source_len(&self, actual: u64) -> Result<()> {
        let expected = self.source_len();
        if actual == expected {
            return Ok(());
        }
        let counted = if actual > expected {
            format!("the image is longer than {expected} bytes")
        } else {
            format!("the image is {actual} bytes")
        };
        Err(Error::Usage(format!(
            "{counted}; a {}x{} BGRX8888 image is exactly {expected} \
             ({} x {} x {BYTES_PER_PIXEL}): check --size and that the source is raw \
             4-byte-per-pixel BGRX",
            self.width, self.height, self.width, self.height
        )))
    }

    /// Byte offset in `/dev/fb0` of source row `row`'s leftmost pixel, or
    /// `None` past the bottom of the source.
    ///
    /// `(y + row) * stride + x * 4`, all in `u64`. [`BlitPlan::new`] has
    /// checked that `y + height <= fb height` and that `x * 4 + row_bytes <=
    /// stride`, so every offset this returns addresses a whole row inside the
    /// `stride * height` bytes the driver registered (`MiSTer_fb.c:239`).
    pub const fn row_offset(&self, row: u32) -> Option<u64> {
        if row >= self.height {
            return None;
        }
        Some(
            ((self.y as u64) + (row as u64)) * (self.stride as u64)
                + (self.x as u64) * (BYTES_PER_PIXEL as u64),
        )
    }

    /// Every row of the blit, top to bottom.
    ///
    /// Top to bottom is the order the source is laid out in and the order the
    /// screen is scanned in, so a partial write — an `ENOSPC`, a signal, a
    /// device that stops taking bytes — leaves a picture that is filled in
    /// from the top rather than scattered.
    pub fn rows(&self) -> impl Iterator<Item = Row> {
        let y = self.y as u64;
        let x_bytes = (self.x as u64) * (BYTES_PER_PIXEL as u64);
        let stride = self.stride as u64;
        let len = self.row_bytes;
        // A running counter rather than `row * len`: `new` proved the product
        // fits a `usize`, and this way there is no cast from `u32` to `usize`
        // to get wrong on a 32-bit target.
        let mut src = 0usize;
        (0..self.height).map(move |row| {
            let here = Row {
                dest: (y + (row as u64)) * stride + x_bytes,
                src,
                len,
            };
            src = src.saturating_add(len);
            here
        })
    }
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

    // -----------------------------------------------------------------------
    // Reading the knob back, and the blit plan
    // -----------------------------------------------------------------------

    /// A framebuffer this tool would itself have programmed.
    const fn knob(width: u32, height: u32, stride: u32) -> FbMode {
        FbMode {
            format: FORMAT_8888,
            rb: RB_SWAPPED,
            width,
            height,
            stride,
        }
    }

    fn must_plan(mode: &FbMode, w: u32, h: u32) -> BlitPlan {
        match BlitPlan::new(mode, w, h) {
            Ok(plan) => plan,
            Err(err) => panic!("expected a plan for {w}x{h}, got {err}"),
        }
    }

    fn refusal(mode: &FbMode, w: u32, h: u32) -> String {
        match BlitPlan::new(mode, w, h) {
            Ok(_) => panic!(
                "{w}x{h} into {}x{} should have been refused",
                mode.width, mode.height
            ),
            Err(err) => {
                assert_eq!(err.exit_code(), 2, "every refusal here is a usage error");
                err.to_string()
            }
        }
    }

    /// What [`mode_param_line`] writes is what [`parse_mode_line`] reads.
    ///
    /// The two are written independently — one is a `format!` transcribed from
    /// `video.cpp:3468`, the other a `split_whitespace` transcribed from
    /// `mode_get`'s `sprintf` (`MiSTer_fb.c:368`) — so this is a real pairing
    /// and not a tautology: it fails if either side changes the field order,
    /// the count, or which of width/height/stride goes where.
    #[test]
    fn a_written_mode_line_parses_back_to_the_same_geometry() {
        for geom in [P720, P480, DOWNSCALED, PIXEL_REPEAT] {
            let line = mode_param_line(&geom);
            let back = parse_mode_line(&line).expect("our own line must parse");
            assert_eq!(
                back,
                FbMode {
                    format: 8888,
                    rb: 1,
                    width: u32::from(geom.width()),
                    height: u32::from(geom.height()),
                    stride: u32::from(geom.width()) * 4,
                },
                "round trip of {line:?}"
            );
            // And the two constants name the same numbers the literal does.
            assert_eq!(back.format, FORMAT_8888);
            assert_eq!(back.rb, RB_SWAPPED);
        }
    }

    /// The kernel's `param_attr_show` appends a newline to whatever `.get`
    /// produced, so the knob never reads back exactly as it was written.
    #[test]
    fn parsing_eats_the_kernels_trailing_newline_and_any_spacing() {
        let want = knob(1280, 720, 5120);
        for text in [
            "8888 1 1280 720 5120",
            "8888 1 1280 720 5120\n",
            "  8888  1   1280 720\t5120  \n",
        ] {
            assert_eq!(parse_mode_line(text).expect("parses"), want, "{text:?}");
        }
    }

    /// `mode_get` returns 0 bytes when the driver never probed
    /// (`MiSTer_fb.c:364-371`), so the empty knob is the common failure and it
    /// gets the message that names the fix.
    #[test]
    fn an_unset_or_malformed_knob_is_a_usage_error_naming_fb_enable() {
        for text in [
            "",                       // the driver never probed
            "\n",                     // ...and the kernel's newline
            "8888 1 1280 720",        // short: mode_get always prints five
            "8888 1 1280 720 5120 0", // long
            "8888 1 1280 720 abc",    // unparseable
            "-1 1 1280 720 5120",     // signed: the statics are u32
            "8888,1,1280,720,5120",   // not whitespace separated
        ] {
            let err =
                parse_mode_line(text).expect_err("a knob that is not five numbers must not parse");
            assert_eq!(err.exit_code(), 2, "{text:?}");
            let msg = err.to_string();
            assert!(msg.contains("fb enable"), "{text:?} said {msg:?}");
        }
    }

    /// The whole reason the knob is read: a layout that is not BGRX8888 means
    /// these four bytes are not B, G, R, X, and painting anyway puts a picture
    /// up with red and blue exchanged and no error anywhere (see [`FbMode`]).
    #[test]
    fn a_blit_is_refused_unless_the_layout_is_bgrx8888() {
        // rb = 0 leaves `setup_fb_info`'s RGB offsets alone (MiSTer_fb.c:220-222,
        // and the swap at :230-234 is what `rb` gates), so the bytes would be
        // R, G, B, X. This is the mutation that is invisible on a greyscale
        // test image and obvious on a face.
        let msg = refusal(
            &FbMode {
                rb: 0,
                ..knob(64, 32, 256)
            },
            8,
            8,
        );
        assert!(msg.contains("rb 0"), "{msg:?}");
        assert!(msg.contains("8888"), "{msg:?}");

        // Any other `format` is a different pixel size entirely: 565 and 1555
        // are two bytes (MiSTer_fb.c:178-198) and 8 is one (`:205-213`).
        for format in [0, 8, 565, 1555, 24, 888] {
            let msg = refusal(
                &FbMode {
                    format,
                    ..knob(64, 32, 256)
                },
                8,
                8,
            );
            assert!(msg.contains(&format!("format {format}")), "{msg:?}");
        }

        // And the pair this tool writes is accepted.
        assert!(BlitPlan::new(&knob(64, 32, 256), 8, 8).is_ok());
    }

    /// A source the size of the screen starts at the origin and walks the
    /// stride, which is the `fb enable` + `image` case the installer uses.
    #[test]
    fn a_full_screen_image_starts_at_the_origin() {
        let mode = knob(1280, 720, 5120);
        let plan = must_plan(&mode, 1280, 720);
        assert_eq!((plan.x(), plan.y()), (0, 0));
        assert_eq!(plan.row_bytes(), 5120);
        assert_eq!(plan.source_len(), 1280 * 720 * 4);
        assert_eq!(plan.row_offset(0), Some(0));
        assert_eq!(plan.row_offset(1), Some(5120));
        assert_eq!(plan.row_offset(719), Some(719 * 5120));
        assert_eq!(plan.row_offset(720), None, "one past the bottom");
        // The last row ends exactly at the end of what the driver registered
        // (`smem_len = line_length * yres`, MiSTer_fb.c:239).
        assert_eq!(719 * 5120 + 5120, mode.byte_len());
        assert_eq!(mode.byte_len(), plan.source_len());
    }

    /// Centring is integer division of the slack, so an odd remainder goes to
    /// the **right** and to the **bottom**.
    ///
    /// The numbers are deliberately not square and the two axes are
    /// deliberately different, so that swapping `x` and `y` changes the answer
    /// (30 and 21 give 8604; swapped they give 12204).
    #[test]
    fn centring_sends_the_odd_pixel_right_and_down() {
        // 101 - 40 = 61, an odd slack: 30 on the left, 31 on the right.
        // 54 - 11 = 43, an odd slack: 21 above, 22 below.
        let mode = knob(101, 54, 404);
        let plan = must_plan(&mode, 40, 11);
        assert_eq!(plan.x(), 30, "61 / 2, the extra column on the right");
        assert_eq!(plan.y(), 21, "43 / 2, the extra row at the bottom");
        assert_eq!(plan.row_offset(0), Some(21 * 404 + 30 * 4));
        assert_eq!(plan.row_offset(0), Some(8604));
        // Spelled out so an off-by-one in either division fails here, and so
        // that transposing the two fails as well.
        assert_ne!(plan.row_offset(0), Some(31 * 404 + 30 * 4), "y off by one");
        assert_ne!(plan.row_offset(0), Some(21 * 404 + 31 * 4), "x off by one");
        assert_ne!(
            plan.row_offset(0),
            Some(30 * 404 + 21 * 4),
            "x and y swapped"
        );
        assert_eq!(plan.row_offset(10), Some(31 * 404 + 30 * 4));
        assert_eq!(plan.row_offset(11), None);

        // An even slack splits evenly, which is the case that hides an
        // off-by-one, so it is asserted next to the odd one rather than alone.
        let plan = must_plan(&mode, 41, 12);
        assert_eq!((plan.x(), plan.y()), (30, 21));
    }

    /// Rows are addressed by the **reported stride**, not by `width * 4`.
    ///
    /// Our two modes cannot produce such a geometry — [`mode_param_line`]
    /// writes `width * 4` — so this one is constructed by hand from the
    /// driver's own padding rule: `if(!stride) stride = (width*4 + 255) & ~255;`
    /// (`MiSTer_fb.c:152`), which for a 100-pixel row is `(400 + 255) & ~255`
    /// = 512. A hardcoded `width * 4` would advance 400 bytes a row and shear
    /// the picture 112 bytes further left on every line.
    #[test]
    fn rows_are_addressed_by_the_stride_the_driver_reports() {
        // `MiSTer_fb.c:152` transcribed, so the example geometry below is
        // the driver's own and not an invented one.
        fn driver_stride(width: u32) -> u32 {
            (width * 4 + 255) & !255
        }
        assert_eq!(driver_stride(100), 512, "(400 + 255) & ~255");
        // Our own modes are already multiples of 256 bytes a row, which is
        // why `mode_param_line`'s `width * 4` and this rule agree for them and
        // why the case below has to be constructed by hand.
        assert_eq!(driver_stride(1280), 5120);
        assert_eq!(driver_stride(640), 2560);

        let mode = knob(100, 10, 512);
        assert_eq!(
            mode.byte_len(),
            5120,
            "stride * height, not width * 4 * height"
        );

        let plan = must_plan(&mode, 20, 3);
        assert_eq!((plan.x(), plan.y()), (40, 3));
        assert_eq!(plan.row_offset(0), Some(3 * 512 + 40 * 4));
        assert_eq!(plan.row_offset(0), Some(1696));
        assert_eq!(plan.row_offset(1), Some(2208));
        assert_eq!(plan.row_offset(2), Some(2720));
        // The step between rows is the stride and nothing else.
        assert_ne!(
            plan.row_offset(0),
            Some(3 * 400 + 40 * 4),
            "width * 4 as stride"
        );
        for row in 0..2u32 {
            let (a, b) = (plan.row_offset(row), plan.row_offset(row + 1));
            assert_eq!(b.zip(a).map(|(b, a)| b - a), Some(512));
        }
    }

    /// No scaling and no cropping in v1: a source bigger than the screen is
    /// refused rather than trimmed, in either axis on its own.
    #[test]
    fn an_oversize_source_is_refused_in_either_axis() {
        let mode = knob(1280, 720, 5120);
        for (w, h) in [(1281, 720), (1280, 721), (1920, 1080), (1281, 721)] {
            let msg = refusal(&mode, w, h);
            assert!(msg.contains(&format!("{w}x{h}")), "{msg:?}");
            assert!(msg.contains("1280x720"), "{msg:?}");
        }
        // The exact fit is not oversize.
        assert!(BlitPlan::new(&mode, 1280, 720).is_ok());
    }

    /// A geometry with nothing in it, and one whose stride cannot hold a row,
    /// are both refused before any offset is computed.
    ///
    /// The messages are asserted, not merely the refusal, because a zero
    /// dimension is refused twice over: drop the geometry check and a 1x1
    /// source is still "bigger than" a 0-pixel-wide screen, so the command
    /// still exits 2 — while telling the operator that the image is wrong when
    /// what is wrong is that the driver never got a geometry. The `0x0` source
    /// is the shape the real path reaches, because `--size` omitted means
    /// "the whole framebuffer" and the whole of a 0x0 framebuffer is 0x0
    /// (`main.rs`, the `image` arm).
    #[test]
    fn a_degenerate_geometry_is_refused() {
        for (mode, w, h) in [
            (knob(0, 720, 5120), 1, 1),
            (knob(1280, 0, 5120), 1, 1),
            // `--size` omitted against a driver that never got a geometry.
            (knob(0, 0, 0), 0, 0),
        ] {
            let msg = refusal(&mode, w, h);
            assert!(
                msg.contains(&format!("geometry is {}x{}", mode.width, mode.height)),
                "the framebuffer is what is wrong, not the source: {msg:?}"
            );
            assert!(msg.contains("fb enable"), "{msg:?}");
        }
        // A stride shorter than the visible row: rows would overlap, and the
        // kernel would clamp the tail against `smem_len` (MiSTer_fb.c:239).
        let msg = refusal(&knob(100, 10, 399), 4, 4);
        assert!(msg.contains("stride"), "{msg:?}");
        // Exactly `width * 4` is the normal case and is fine.
        assert!(BlitPlan::new(&knob(100, 10, 400), 4, 4).is_ok());
        // A zero-sized source against a real geometry is the other message:
        // here the source is what is wrong, and `--size` is what to fix.
        for (w, h) in [(0, 4), (4, 0)] {
            let msg = refusal(&knob(100, 10, 400), w, h);
            assert!(msg.contains(&format!("the image is {w}x{h}")), "{msg:?}");
            assert!(msg.contains("at least 1"), "{msg:?}");
        }
    }

    /// The knob reports one stride; the frame reader walks another, and
    /// nothing in Linux can read it back.
    ///
    /// [`mode_param_line`] (`video.cpp:3468`) and word 10 of [`enable_words`]
    /// (`video.cpp:3511`) are both `width * 4`, so every geometry this crate
    /// writes is silent here. The noisy case is a geometry it did not write.
    #[test]
    fn a_stride_the_frame_reader_was_never_told_is_warned_about() {
        assert_eq!(frame_reader_stride(1280), 5120);
        assert_eq!(stride_warning(&knob(1280, 720, 5120)), None);
        assert_eq!(stride_warning(&knob(640, 480, 2560)), None);

        // The driver's auto-pad (`MiSTer_fb.c:152`) for a 720-pixel row is
        // 3072, where any fabric this crate configured is at 2880: 192 bytes
        // of shear a row, matching the fbdev exactly and the screen not at
        // all.
        assert_eq!((720 * 4 + 255) & !255, 3072);
        let msg = stride_warning(&knob(720, 480, 3072)).expect("3072 is not 720 * 4");
        assert!(msg.contains("3072"), "{msg:?}");
        assert!(msg.contains("2880"), "{msg:?}");
        assert!(msg.contains("fb enable"), "{msg:?}");

        // A stride shorter than the row is warned about as well, on its way
        // to being refused outright by `BlitPlan::new`.
        assert!(stride_warning(&knob(100, 10, 399)).is_some());

        // 640 is the case where the pad changes nothing — which is why the
        // driver's own defaults look exactly like a geometry someone asked
        // for (see `FbMode`).
        assert_eq!((640 * 4 + 255) & !255, 2560);
    }

    /// The source has to be exactly `w * h * 4` bytes, and the message names
    /// both counts because the ratio is what diagnoses the converter.
    #[test]
    fn a_source_of_the_wrong_length_is_refused_with_both_counts() {
        let plan = must_plan(&knob(101, 54, 404), 40, 11);
        assert_eq!(plan.source_len(), 40 * 11 * 4);
        assert_eq!(plan.source_len(), 1760);
        assert!(plan.check_source_len(1760).is_ok());

        // Short: the exact count is in the message, because the reader has
        // the whole file and the ratio is the diagnosis. 1320 is the
        // `-pix_fmt bgr24` mistake — three bytes a pixel, three quarters of
        // 1760 — and nothing guesses: the operator reads the two counts.
        assert_eq!(40 * 11 * 3, 1320);
        for actual in [0u64, 1, 1320, 1759] {
            let err = plan
                .check_source_len(actual)
                .expect_err("only the exact length is accepted");
            assert_eq!(err.exit_code(), 2);
            let msg = err.to_string();
            assert!(
                msg.contains(&format!("the image is {actual} bytes")),
                "{msg:?}"
            );
            assert!(msg.contains("1760"), "{msg:?}");
        }

        // Long: "longer than", never a count. `hw::read_source` stops one
        // byte past `source_len`, so anything above the expectation is a
        // lower bound — 1761 here is "at least 1761", and printing it as the
        // file's size would be a lie. That cap is what keeps a mistyped path
        // to something huge an exit 2 instead of an allocation failure.
        for actual in [1761u64, 3520, u64::MAX] {
            let err = plan
                .check_source_len(actual)
                .expect_err("only the exact length is accepted");
            assert_eq!(err.exit_code(), 2);
            let msg = err.to_string();
            assert!(msg.contains("longer than 1760 bytes"), "{msg:?}");
            assert!(
                !msg.contains("3520"),
                "no count it cannot stand behind: {msg:?}"
            );
        }
    }

    /// [`BlitPlan::rows`] walks the source once, top to bottom, with no gap
    /// and no overlap in the source and a stride's step in the destination.
    #[test]
    fn rows_walk_the_source_once_top_to_bottom() {
        let plan = must_plan(&knob(8, 4, 32), 3, 2);
        // 8 - 3 = 5 -> x = 2; 4 - 2 = 2 -> y = 1.
        assert_eq!((plan.x(), plan.y()), (2, 1));
        assert_eq!(
            plan.rows().collect::<Vec<_>>(),
            vec![
                // row 0 -> (y + 0) * stride + x * 4 = 1 * 32 + 2 * 4
                Row {
                    dest: 40,
                    src: 0,
                    len: 12
                },
                // row 1 -> 2 * 32 + 2 * 4
                Row {
                    dest: 72,
                    src: 12,
                    len: 12
                },
            ],
            "top row first, and the source consumed in order"
        );

        // Against a full-screen plan: every byte of the source is covered
        // exactly once, in order, and the destinations agree with
        // `row_offset`.
        let plan = must_plan(&knob(64, 48, 256), 64, 48);
        let rows: Vec<Row> = plan.rows().collect();
        assert_eq!(rows.len(), 48);
        let mut expect_src = 0usize;
        for (row, r) in rows.iter().enumerate() {
            assert_eq!(
                r.src, expect_src,
                "row {row} starts where row {row} - 1 ended"
            );
            assert_eq!(r.len, plan.row_bytes());
            let index = u32::try_from(row).expect("48 rows fit a u32");
            assert_eq!(Some(r.dest), plan.row_offset(index));
            expect_src = expect_src.saturating_add(r.len);
        }
        assert_eq!(expect_src as u64, plan.source_len());
    }
}
