//! Golden-vector test for the ADV7513 register tables.
//!
//! `tests/golden/adv7513.cpp` is Main_MiSTer's `hdmi_config_init()`,
//! `hdmi_config_audio()`, `hdmi_config_set_csc()` and
//! `hdmi_config_set_mode()` with the i2c plumbing removed, compiled against
//! upstream's own `mat4x4.h` (vendored beside it);
//! `tests/golden/adv7513-gen.sh` compiles it with the host `c++` and writes
//! `tests/golden/adv7513.json`. This test reads that JSON and asserts
//! [`itsalive::adv7513`] reproduces every row of every table, in order.
//!
//! The numbers are never typed in by hand and never copied out of
//! `src/adv7513.rs`. They come out of the C, because a test whose expected
//! values are lifted from the code under test only restates the transcription:
//! it would still pass with a wrong clock divider at `0x9D`, a 96 kHz audio N
//! at `0x02`, or two `init_data[]` rows swapped. Those are exactly the slips
//! that produce a black or mistimed screen and that no other host test can see.
//!
//! The JSON is *also* checked against a handful of values derived by reading
//! the C (see `golden_json_itself_is_sane`), so a corrupt or wrongly
//! regenerated JSON cannot quietly make this test vacuous by agreeing with a
//! broken table.
//!
//! There is no serde in this crate (`libc` is the only dependency), so the
//! scanner below is hand-rolled. It knows exactly the shape `adv7513.cpp`
//! prints and nothing more.

use itsalive::adv7513::{AUDIO, CHIP_ADDR, CSC, INIT, mode_regs, writes};

/// The generated vectors, compiled into the test binary so it needs no files
/// at run time.
const GOLDEN: &str = include_str!("golden/adv7513.json");

/// One `"modes"` entry: the inputs `hdmi_config_set_mode()` was given and the
/// `(reg, value)` pairs it wrote.
#[derive(Debug)]
struct Mode {
    vic: u8,
    pr: u8,
    hpol: u8,
    vpol: u8,
    rows: Vec<(u8, u8)>,
}

/// Step past `"<key>"` and its colon, returning the offset of the value.
fn value_at(src: &str, from: usize, key: &str) -> usize {
    let pat = format!("\"{key}\"");
    let rel = src[from..]
        .find(&pat)
        .unwrap_or_else(|| panic!("golden json: no {pat} after byte {from}"));
    let mut i = from + rel + pat.len();
    let b = src.as_bytes();
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    assert_eq!(b[i], b':', "golden json: expected ':' after {pat}");
    i += 1;
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Take the run of decimal digits at `from`, returning its value and the
/// offset just past it.
fn number_at(src: &str, from: usize) -> (u32, usize) {
    let b = src.as_bytes();
    let mut j = from;
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    assert!(j > from, "golden json: no number at byte {from}");
    let n = src[from..j]
        .parse()
        .unwrap_or_else(|e| panic!("golden json: {:?}: {e}", &src[from..j]));
    (n, j)
}

/// Read an unsigned field, checking it fits a byte.
fn u8_field(src: &str, from: usize, key: &str) -> (u8, usize) {
    let (n, next) = number_at(src, value_at(src, from, key));
    (
        u8::try_from(n).unwrap_or_else(|_| panic!("golden json: {key} = {n} does not fit a byte")),
        next,
    )
}

/// Read a `[[a, b], [a, b], ...]` array of register pairs.
///
/// `at` is the offset of the opening bracket.
fn rows_at(src: &str, at: usize) -> (Vec<(u8, u8)>, usize) {
    let b = src.as_bytes();
    assert_eq!(b[at], b'[', "golden json: expected a row array at {at}");
    let mut i = at + 1;
    let mut rows = Vec::new();

    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
        if b[i] == b']' {
            return (rows, i + 1);
        }

        assert_eq!(b[i], b'[', "golden json: expected '[' at byte {i}");
        i += 1;
        let (reg, next) = number_at(src, i);
        i = next;
        while b[i].is_ascii_whitespace() || b[i] == b',' {
            i += 1;
        }
        let (val, next) = number_at(src, i);
        i = next;
        while b[i].is_ascii_whitespace() {
            i += 1;
        }
        assert_eq!(b[i], b']', "golden json: expected ']' at byte {i}");
        i += 1;

        let byte = |n: u32, what: &str| {
            u8::try_from(n)
                .unwrap_or_else(|_| panic!("golden json: {what} {n} does not fit a byte"))
        };
        rows.push((byte(reg, "register"), byte(val, "value")));
    }
}

/// Read the `"<key>": [[a, b], ...]` array named by `key`.
fn rows_field(src: &str, from: usize, key: &str) -> (Vec<(u8, u8)>, usize) {
    rows_at(src, value_at(src, from, key))
}

/// The whole file, in the order `adv7513.cpp` prints it.
struct Golden {
    hdmi_has_int: u8,
    init: Vec<(u8, u8)>,
    audio: Vec<(u8, u8)>,
    csc: Vec<(u8, u8)>,
    writes: Vec<(u8, u8)>,
    modes: Vec<Mode>,
    repeat_rows_without_invalidate: u8,
}

fn parse(src: &str) -> Golden {
    let (hdmi_has_int, at) = u8_field(src, 0, "hdmi_has_int");
    let (init_len, at) = u8_field(src, at, "init_len");
    let (audio_len, at) = u8_field(src, at, "audio_len");
    let (csc_len, at) = u8_field(src, at, "csc_len");
    let (writes, at) = rows_field(src, at, "writes");

    // The C appends all three tables to one buffer, in the order
    // hdmi_config_init() calls them; the three lengths are where each one
    // started. Slicing here is what proves the order came from the C.
    let total = usize::from(init_len) + usize::from(audio_len) + usize::from(csc_len);
    assert_eq!(
        writes.len(),
        total,
        "golden json: init_len + audio_len + csc_len must account for every row"
    );
    let init = writes[..usize::from(init_len)].to_vec();
    let audio = writes[usize::from(init_len)..total - usize::from(csc_len)].to_vec();
    let csc = writes[total - usize::from(csc_len)..].to_vec();

    let mut at = value_at(src, at, "modes");
    let mut modes = Vec::new();
    let repeat = src
        .find("\"repeat_rows_without_invalidate\"")
        .unwrap_or_else(|| panic!("golden json: no repeat_rows_without_invalidate"));
    while let Some(rel) = src[at..].find("\"vic\"") {
        if at + rel > repeat {
            break;
        }
        let start = at + rel;
        let (vic, i) = u8_field(src, start, "vic");
        let (pr, i) = u8_field(src, i, "pr");
        let (hpol, i) = u8_field(src, i, "hpol");
        let (vpol, i) = u8_field(src, i, "vpol");
        let (rows, next) = rows_field(src, i, "rows");
        modes.push(Mode {
            vic,
            pr,
            hpol,
            vpol,
            rows,
        });
        at = next;
    }

    let (repeat_rows_without_invalidate, _) = u8_field(src, at, "repeat_rows_without_invalidate");

    Golden {
        hdmi_has_int,
        init,
        audio,
        csc,
        writes,
        modes,
        repeat_rows_without_invalidate,
    }
}

/// The JSON must say what a reader of `video.cpp` expects it to say.
///
/// Every value asserted here was derived by reading the C, not by running it:
///
/// - `init_data[]` in `hdmi_config_init()` (`video.cpp:1499-1604`) has 51
///   address/value pairs, the audio table (`video.cpp:1420-1453`) 13 and
///   `csc_data[]` (`video.cpp:1365-1397`) 28, for 92 writes in all.
/// - The table ends are `0x98, 03` (`video.cpp:1500`, a C octal literal, so 3)
///   and `0xFA, 0x7D` (`:1603`); `0xAF` (`:1422`) and `0x09, 0x0A` (`:1452`);
///   `0x18` (`:1366`) and `0xC3` (`:1396`).
/// - `0x02` is `cfg.hdmi_audio_96k ? 0x30 : 0x18` (`video.cpp:1447`) and the
///   default is 0 (`cfg.cpp:594`), so N = 6144 and the byte is 0x18.
/// - `clipMax` is `0xFFF` without limited range (`video.cpp:1362`), so
///   `0xC2 = 0x0F` and `0xC3 = 0xFF`.
/// - `0x94` is `int0`, and the generator pins `hdmi_has_int()` to 0.
///
/// If a regenerated `adv7513.json` ever disagrees with this, the C was not the
/// C.
#[test]
fn golden_json_itself_is_sane() {
    let g = parse(GOLDEN);

    assert_eq!(g.init.len(), 51, "init_data[] is 51 rows");
    assert_eq!(g.audio.len(), 13, "the audio table is 13 rows");
    assert_eq!(g.csc.len(), 28, "csc_data[] is 28 rows");
    assert_eq!(g.writes.len(), 92);

    assert_eq!(g.init[0], (0x98, 0x03));
    assert_eq!(g.init[50], (0xFA, 0x7D));
    assert_eq!(g.audio[0].0, 0xAF);
    assert_eq!(g.audio[12], (0x09, 0x0A));
    assert_eq!(g.csc[0].0, 0x18);
    assert_eq!(g.csc[27], (0xC3, 0xFF));

    assert!(
        g.audio.contains(&(0x02, 0x18)),
        "N must be 6144 (48 kHz), not the 96 kHz 0x30"
    );
    assert!(g.csc.contains(&(0xC2, 0x0F)), "clipMax = 0xFFF");

    assert_eq!(
        g.hdmi_has_int, 0,
        "the generator must model itsalive: no interrupt query, so int0 = 0x00"
    );
    assert!(g.init.contains(&(0x94, 0x00)));

    // Three registers appear in both the bulk table and the mode write; that
    // is Main's own sequencing (video.cpp:1535-1536 then :1710-1712).
    assert_eq!(g.modes[0].rows.len(), 3);
    let mode_regs_written: Vec<u8> = g.modes[0].rows.iter().map(|&(reg, _)| reg).collect();
    assert_eq!(mode_regs_written, vec![0x17, 0x3B, 0x3C]);

    // video.cpp:1706: a second identical call writes nothing at all.
    assert_eq!(
        g.repeat_rows_without_invalidate, 0,
        "Main's mode-write cache suppresses an unchanged repeat"
    );
}

/// The whole point: the Rust reproduces the C, row for row.
#[test]
fn every_table_matches_the_compiled_c() {
    let g = parse(GOLDEN);

    for (i, (&got, &want)) in INIT.iter().zip(g.init.iter()).enumerate() {
        assert_eq!(got, want, "INIT row {i}: got {got:02X?} want {want:02X?}");
    }
    assert_eq!(INIT.len(), g.init.len(), "INIT row count");

    for (i, (&got, &want)) in AUDIO.iter().zip(g.audio.iter()).enumerate() {
        assert_eq!(got, want, "AUDIO row {i}: got {got:02X?} want {want:02X?}");
    }
    assert_eq!(AUDIO.len(), g.audio.len(), "AUDIO row count");

    for (i, (&got, &want)) in CSC.iter().zip(g.csc.iter()).enumerate() {
        assert_eq!(got, want, "CSC row {i}: got {got:02X?} want {want:02X?}");
    }
    assert_eq!(CSC.len(), g.csc.len(), "CSC row count");
}

/// `writes()` must reproduce the C's write order, not just its contents: the
/// chip is a state machine and `0x41` powers the output up part-way through.
#[test]
fn writes_matches_the_compiled_c_in_order() {
    let g = parse(GOLDEN);
    let got: Vec<(u8, u8)> = writes().collect();

    for (i, (&got, &want)) in got.iter().zip(g.writes.iter()).enumerate() {
        assert_eq!(got, want, "write {i}: got {got:02X?} want {want:02X?}");
    }
    assert_eq!(got.len(), g.writes.len(), "write count");
}

/// Every mode vector the C was run on, including the `pr != 0` rows that no
/// mode this crate ships exercises yet.
#[test]
fn mode_regs_matches_the_compiled_c() {
    let g = parse(GOLDEN);
    assert!(!g.modes.is_empty(), "golden json parsed to no modes");

    for m in &g.modes {
        let got = mode_regs(m.hpol, m.vpol, m.vic, m.pr);
        assert_eq!(
            got.as_slice(),
            m.rows.as_slice(),
            "hpol {} vpol {} vic {} pr {}: got {:02X?} want {:02X?}",
            m.hpol,
            m.vpol,
            m.vic,
            m.pr,
            got,
            m.rows
        );
    }

    // The vectors have to cover both pr arms, or this test would pass with the
    // 2x-clock branch deleted (video.cpp:1699).
    assert!(g.modes.iter().any(|m| m.pr == 0));
    assert!(g.modes.iter().any(|m| m.pr != 0));
}

/// `video.cpp:1470`, the chip address the tables are written to.
#[test]
fn chip_address_is_the_main_register_map() {
    assert_eq!(CHIP_ADDR, 0x39);
}
