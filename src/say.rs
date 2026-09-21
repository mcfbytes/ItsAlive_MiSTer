//! `say`: put text where a human standing at the monitor can read it.
//!
//! In v1 that is one `write(2)` to `/dev/tty1` and nothing else: the kernel is
//! built with `CONFIG_FRAMEBUFFER_CONSOLE=y`, so fbcon owns the console and
//! paints whatever lands on the tty into `/dev/fb0`, which is the surface
//! `fb enable` has just pointed the fabric's frame reader at
//! (`docs/ARCHITECTURE.md` §1). That is the same mechanism stock MiSTer uses
//! to show `update_all` on screen, and it is why this module draws no pixels
//! itself. If the rig says fbcon is not bound, T5.1 replaces this with a
//! direct 8x16 draw into the framebuffer; see `PLAN.md` §2 unknown 1.
//!
//! Everything here except [`say`] is pure text, so the composition is tested
//! on the host and only the final `write` needs a tty.

use crate::Result;
use crate::hw::TextSink;

/// Clear the screen and put the cursor back at the top left.
///
/// `ESC [ 2 J` is ED with parameter 2, "erase the whole display", and
/// `ESC [ H` is CUP with no parameters, "cursor to row 1, column 1". The order
/// matters on a terminal that leaves the cursor where it was after an erase,
/// which is most of them.
///
/// **This relies on fbcon being bound to the console**, because the escape
/// sequence is interpreted by the kernel's terminal emulation and not by us:
/// on a tty with no console driver behind it these six bytes are simply
/// written and nothing is erased. `PLAN.md` §2 unknown 1 has not been
/// confirmed on hardware yet — the rig session (T3.1) is what answers it, and
/// if the answer is "no", T5.1's direct draw replaces this module rather than
/// patching the sequence.
pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";

/// The bytes [`say`] writes: the words joined with single spaces, one
/// trailing newline, optionally preceded by [`CLEAR_SCREEN`].
///
/// The join is single-space regardless of what the shell did with the
/// caller's quoting, so `itsalive say hello   world` and
/// `itsalive say "hello" "world"` put the same line on the screen. Empty text
/// writes nothing at all rather than a bare newline, so `say --clear` with no
/// words clears the screen and leaves the cursor at the top left.
pub fn message(clear: bool, words: &[String]) -> String {
    let mut out = String::new();
    if clear {
        out.push_str(CLEAR_SCREEN);
    }
    if !words.is_empty() {
        out.push_str(&words.join(" "));
        out.push('\n');
    }
    out
}

/// Write [`message`] to `sink` in one call.
///
/// One `write_text` rather than one per part: a tty is shared with whatever
/// else is writing to the console, and a single `write(2)` of the whole line
/// is the closest thing to atomicity a tty offers. `?Sized` so that a
/// `&mut dyn TextSink` can be passed straight through.
pub fn say<S: TextSink + ?Sized>(sink: &mut S, clear: bool, words: &[String]) -> Result<()> {
    let text = message(clear, words);
    if text.is_empty() {
        return Ok(());
    }
    sink.write_text(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    /// A recording [`TextSink`], and one that can be told to fail.
    struct FakeTty {
        written: Vec<u8>,
        fail: bool,
    }

    impl FakeTty {
        fn new() -> Self {
            Self {
                written: Vec::new(),
                fail: false,
            }
        }
    }

    impl TextSink for FakeTty {
        fn write_text(&mut self, bytes: &[u8]) -> Result<()> {
            if self.fail {
                return Err(Error::io(
                    "write /dev/tty1",
                    std::io::Error::from_raw_os_error(libc::ENXIO),
                ));
            }
            self.written.extend_from_slice(bytes);
            Ok(())
        }
    }

    fn words(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn the_clear_sequence_is_ed2_then_cup() {
        assert_eq!(CLEAR_SCREEN.as_bytes(), b"\x1b[2J\x1b[H");
    }

    #[test]
    fn words_are_joined_with_single_spaces_and_one_newline() {
        assert_eq!(message(false, &words(&["hello", "world"])), "hello world\n");
        assert_eq!(message(false, &words(&["one"])), "one\n");
    }

    /// The caller's own spacing survives; only the gaps between arguments are
    /// normalised, because the shell has already thrown the rest away.
    #[test]
    fn a_quoted_argument_keeps_its_spaces() {
        assert_eq!(
            message(false, &words(&["a  b", "c"])),
            "a  b c\n",
            "only the join is single-spaced"
        );
    }

    #[test]
    fn clear_comes_before_the_text() {
        assert_eq!(
            message(true, &words(&["Installing..."])),
            "\x1b[2J\x1b[HInstalling...\n"
        );
    }

    #[test]
    fn clear_with_no_text_writes_only_the_escape() {
        assert_eq!(message(true, &[]), "\x1b[2J\x1b[H");
    }

    #[test]
    fn no_text_and_no_clear_writes_nothing() {
        assert_eq!(message(false, &[]), "");
        let mut tty = FakeTty::new();
        say(&mut tty, false, &[]).unwrap();
        assert!(tty.written.is_empty());
    }

    #[test]
    fn say_writes_the_whole_message_in_one_call() {
        let mut tty = FakeTty::new();
        say(&mut tty, true, &words(&["step", "2"])).unwrap();
        assert_eq!(tty.written, b"\x1b[2J\x1b[Hstep 2\n".to_vec());
    }

    /// A tty that refuses the write is [`Error::Io`], exit 14: `say` is the
    /// one subcommand whose whole job is that write.
    #[test]
    fn a_failing_tty_is_exit_14() {
        let mut tty = FakeTty::new();
        tty.fail = true;
        let err = say(&mut tty, false, &words(&["x"])).unwrap_err();
        assert_eq!(err.exit_code(), 14);
    }

    /// `say` takes a trait object, so the CLI can hold one writer for the
    /// real tty and another for a test without a second generic parameter.
    #[test]
    fn say_accepts_a_trait_object() {
        let mut tty = FakeTty::new();
        let erased: &mut dyn TextSink = &mut tty;
        say(erased, false, &words(&["dyn"])).unwrap();
        assert_eq!(tty.written, b"dyn\n".to_vec());
    }
}
