//! Golden-vector test for the PLL solver.
//!
//! `tests/golden/pll.c` is Main_MiSTer's `getPLLdiv()`, `findPLLpar()` and
//! `setPLL()` with the tracing removed; `tests/golden/gen.sh` compiles it with
//! the host `cc` and writes `tests/golden/pll.json`. This test reads that JSON
//! and asserts [`itsalive::video::solve_pll`] reproduces every row.
//!
//! The numbers are never computed by hand. They come out of the C, because the
//! whole point of the exercise is that the Rust and the C agree bit for bit:
//! these values end up in FPGA PLL registers, where a wrong bit is a black
//! screen and no host test would catch it.
//!
//! The JSON is *also* checked against a handful of values derived by reading
//! the C (see `golden_json_itself_is_sane`), so a corrupt or wrongly
//! regenerated JSON cannot quietly make this test vacuous by agreeing with a
//! broken solver.
//!
//! There is no serde in this crate (`libc` is the only dependency), so the
//! scanner below is hand-rolled. It knows exactly the shape `pll.c` prints and
//! nothing more.

use itsalive::video::solve_pll;

/// The generated vectors, compiled into the test binary so it needs no files
/// at run time.
const GOLDEN: &str = include_str!("golden/pll.json");

/// One `"rows"` entry of `pll.json`.
#[derive(Debug)]
struct Row {
    /// The `Fout` passed to `setPLL()`, in MHz.
    fout: f64,
    /// `setPLL()`'s `c`.
    c: u32,
    /// `setPLL()`'s `m`.
    m: u32,
    /// `setPLL()`'s `k`.
    k: u32,
    /// `v->Fpix`.
    fpix: f64,
    /// `item[9]` through `item[20]`.
    item: [u32; 12],
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

/// Take the run of JSON number characters at `from`, returning it and the
/// offset just past it.
fn number_at(src: &str, from: usize) -> (&str, usize) {
    let b = src.as_bytes();
    let mut j = from;
    while j < b.len() && matches!(b[j], b'0'..=b'9' | b'+' | b'-' | b'.' | b'e' | b'E') {
        j += 1;
    }
    assert!(j > from, "golden json: no number at byte {from}");
    (&src[from..j], j)
}

/// Read a `u32` field.
fn u32_field(src: &str, from: usize, key: &str) -> (u32, usize) {
    let (text, next) = number_at(src, value_at(src, from, key));
    (
        text.parse()
            .unwrap_or_else(|e| panic!("golden json: {key} = {text:?}: {e}")),
        next,
    )
}

/// Read an `f64` field.
fn f64_field(src: &str, from: usize, key: &str) -> (f64, usize) {
    let (text, next) = number_at(src, value_at(src, from, key));
    (
        text.parse()
            .unwrap_or_else(|e| panic!("golden json: {key} = {text:?}: {e}")),
        next,
    )
}

fn parse(src: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut at = 0usize;

    while let Some(rel) = src[at..].find("\"fout\"") {
        let start = at + rel;
        let (fout, mut i) = f64_field(src, start, "fout");
        let (c, next) = u32_field(src, i, "c");
        i = next;
        let (m, next) = u32_field(src, i, "m");
        i = next;
        let (k, next) = u32_field(src, i, "k");
        i = next;
        let (fpix, next) = f64_field(src, i, "fpix");
        i = next;

        // "item": [ a, b, ... ] -- twelve of them.
        let mut j = value_at(src, i, "item");
        assert_eq!(
            src.as_bytes()[j],
            b'[',
            "golden json: item must be an array"
        );
        j += 1;
        let mut item = [0u32; 12];
        for slot in &mut item {
            while src.as_bytes()[j].is_ascii_whitespace() || src.as_bytes()[j] == b',' {
                j += 1;
            }
            let (text, next) = number_at(src, j);
            *slot = text
                .parse()
                .unwrap_or_else(|e| panic!("golden json: item entry {text:?}: {e}"));
            j = next;
        }
        while src.as_bytes()[j].is_ascii_whitespace() {
            j += 1;
        }
        assert_eq!(
            src.as_bytes()[j],
            b']',
            "golden json: item must hold exactly 12 values"
        );

        rows.push(Row {
            fout,
            c,
            m,
            k,
            fpix,
            item,
        });
        at = j;
    }

    rows
}

/// Find the row for a pixel clock, by the `f64` the JSON itself carries.
fn row(rows: &[Row], fout: f64) -> &Row {
    rows.iter()
        .find(|r| r.fout == fout)
        .unwrap_or_else(|| panic!("golden json: no row for {fout} MHz"))
}

/// The JSON must say what a reader of `video.cpp` expects it to say.
///
/// Every value asserted here was derived by reading the C, not by running it:
///
/// - `findPLLpar()` starts at `c = 1` and raises it while `Fout * c < 400`
///   (`video.cpp:226-227`). For 74.25 that stops at `c = 6` (445.5 MHz); for
///   25.175 at `c = 16` (402.8 MHz).
/// - `m = (uint32_t)(fvco / 50)` (`video.cpp:232`): `445.5 / 50 = 8.91` and
///   `402.8 / 50 = 8.056`, so M is 8 for both, and `ko` (0.91 and 0.056) is
///   inside `(0.05f, 0.95f)` either way, so the first candidate is accepted.
/// - The last three rates exist because the first six are all like that — one
///   candidate, non-zero `ko`, done — which leaves the retry, the `return 0`
///   and the whole fallback untested. 40 MHz has `ko == 0` exactly, 20.001
///   takes one `c++` retry, and 49.92 exhausts the search and lands in the
///   fallback's `ko >= 0.95f` arm. Each is walked through below.
/// - `getPLLdiv()` (`video.cpp:218-222`) on an even divider is
///   `((div / 2) << 8) | (div / 2)`: M = 8 gives `item[10] = 0x404`, C = 6
///   gives `item[14] = 0x303`, C = 16 gives `0x808`.
/// - `item[9], [11], [12], [13], [15]..[19]` are the constants at
///   `video.cpp:301-312`.
///
/// If a regenerated `pll.json` ever disagrees with this, the C was not the C.
#[test]
fn golden_json_itself_is_sane() {
    let rows = parse(GOLDEN);
    assert_eq!(rows.len(), 9, "pll.c emits nine rates");

    let r = row(&rows, 74.25);
    assert_eq!(r.c, 6, "74.25 * 6 = 445.5 is the first Fvco >= 400");
    assert_eq!(r.m, 8, "trunc(445.5 / 50) = 8");
    assert_eq!(r.item[1], 0x0404, "item[10] = getPLLdiv(8)");
    assert_eq!(r.item[5], 0x0303, "item[14] = getPLLdiv(6)");
    assert_eq!(r.item[11], r.k, "item[20] = K");
    assert_eq!(r.fpix, 74.25, "an exact solution reproduces Fout");

    // The nine constant entries, video.cpp:301-312.
    assert_eq!(r.item[0], 4); // item[9]
    assert_eq!(r.item[2], 3); // item[11]
    assert_eq!(r.item[3], 0x10000); // item[12]
    assert_eq!(r.item[4], 5); // item[13]
    assert_eq!(r.item[6], 9); // item[15]
    assert_eq!(r.item[7], 2); // item[16]
    assert_eq!(r.item[8], 8); // item[17]
    assert_eq!(r.item[9], 7); // item[18]
    assert_eq!(r.item[10], 7); // item[19]

    let r = row(&rows, 25.175);
    assert_eq!(r.c, 16, "25.175 * 16 = 402.8 is the first Fvco >= 400");
    assert_eq!(r.m, 8, "trunc(402.8 / 50) = 8");
    assert_eq!(r.item[1], 0x0404, "item[10] = getPLLdiv(8)");
    assert_eq!(r.item[5], 0x0808, "item[14] = getPLLdiv(16)");

    // An odd divider sets bit 17 and makes the two half-counts differ by one:
    // 65 MHz lands on C = 7, M = 9 (video.cpp:220).
    let r = row(&rows, 65.0);
    assert_eq!(r.c, 7);
    assert_eq!(r.m, 9);
    assert_eq!(r.item[1], 0x2_0504, "item[10] = getPLLdiv(9)");
    assert_eq!(r.item[5], 0x2_0403, "item[14] = getPLLdiv(7)");

    // 40 MHz is vmodes[5] (800x600@60, video.cpp:132) and is the one rate
    // here whose Fvco comes out exact: 40 * 10 = 400, so ko is 0. That makes
    // `if (ko && (...))` (video.cpp:238) false on the *first* candidate --
    // without the `ko &&` the 0 would count as "outside the allowed range" and
    // the search would run on to C = 11 -- and it is the case `k = ko ? ... :
    // 1` (video.cpp:293) turns into the literal 1.
    let r = row(&rows, 40.0);
    assert_eq!(r.c, 10, "40 * 10 = 400 is the first Fvco >= 400");
    assert_eq!(r.m, 8, "400 / 50 = 8 exactly");
    assert_eq!(r.k, 1, "ko == 0, so K is the literal 1 of video.cpp:293");
    assert_eq!(r.item[11], 1, "item[20] = K = 1, never 0");
    assert_eq!(r.item[5], 0x0505, "item[14] = getPLLdiv(10)");
    assert_eq!(r.fpix, 40.0);

    // 20.001 * 20 = 400.02, so the first candidate is C = 20 with
    // ko = 0.0004, which is <= 0.05f: findPLLpar takes the `c++` at
    // video.cpp:247 and tries again. C = 21 gives ko = 0.40042 and is
    // accepted. Nothing else in this file reaches that retry.
    let r = row(&rows, 20.001);
    assert_eq!(r.c, 21, "one retry past the first candidate, C = 20");
    assert_eq!(r.m, 8, "20.001 * 21 = 420.021, trunc(/50) = 8");
    assert_eq!(
        r.item[5], 0x2_0B0A,
        "item[14] = getPLLdiv(21), an odd divider"
    );

    // 49.92 is the exhaustion case. C = 9 is the first Fvco >= 400 (449.28)
    // and every candidate from there has ko >= 0.95f, so findPLLpar keeps
    // retrying until fvco passes 1500 MHz and returns 0 (video.cpp:241-245).
    // setPLL's fallback (video.cpp:273-291) then redoes C = 9, finds
    // ko = 0.9856 >= 0.95f, and takes the carry: M becomes 9 and ko becomes 0
    // (video.cpp:286-289). So Fpix is (0 + 9) * 50 / 9 = 50, not 49.92, and
    // both dividers are getPLLdiv(9).
    let r = row(&rows, 49.92);
    assert_eq!(r.c, 9, "49.92 * 9 = 449.28 is the first Fvco >= 400");
    assert_eq!(r.m, 9, "the fallback's m++ at video.cpp:288");
    assert_eq!(r.k, 1, "the fallback zeroes ko, so K is 1");
    assert_eq!(r.item[1], 0x2_0504, "item[10] = getPLLdiv(9)");
    assert_eq!(r.item[5], 0x2_0504, "item[14] = getPLLdiv(9)");
    assert_eq!(
        r.fpix, 50.0,
        "the fallback trades exactness for termination: 49.92 in, 50 out"
    );

    // K is never zero on the wire: setPLL sends 1 instead (video.cpp:293).
    for r in &rows {
        assert_ne!(r.k, 0, "{} MHz: K must not be 0", r.fout);
    }
}

/// The whole point: the Rust reproduces the C, row for row.
#[test]
fn solve_pll_reproduces_every_golden_row() {
    let rows = parse(GOLDEN);
    assert!(!rows.is_empty(), "golden json parsed to nothing");

    for r in &rows {
        let got = solve_pll(r.fout)
            .unwrap_or_else(|| panic!("{} MHz: solve_pll refused a rate the C solved", r.fout));
        assert_eq!(got.c, r.c, "{} MHz: C", r.fout);
        assert_eq!(got.m, r.m, "{} MHz: M", r.fout);
        assert_eq!(got.k, r.k, "{} MHz: K", r.fout);
        assert_eq!(
            got.f_pix_mhz.to_bits(),
            r.fpix.to_bits(),
            "{} MHz: Fpix must match the C bit for bit, got {} want {}",
            r.fout,
            got.f_pix_mhz,
            r.fpix
        );
        assert_eq!(got.item, r.item, "{} MHz: item[9..=20]", r.fout);
    }
}

/// 25.175 MHz is the case where the arithmetic is visibly lossy: the solution
/// does not land back exactly on the requested clock. If someone "tidied" the
/// solver into rational arithmetic this would start passing exactly and the
/// hardware would get a different K.
#[test]
fn fpix_is_not_silently_rounded() {
    let rows = parse(GOLDEN);
    let r = row(&rows, 25.175);
    assert_ne!(
        r.fpix.to_bits(),
        25.175_f64.to_bits(),
        "the C's Fpix for 25.175 is a few ulp off; the Rust must be too"
    );
    assert_eq!(
        solve_pll(r.fout)
            .expect("25.175 MHz is a real clock")
            .f_pix_mhz
            .to_bits(),
        r.fpix.to_bits()
    );
}
