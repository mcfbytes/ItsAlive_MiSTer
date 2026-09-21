//! The exit codes, out of a real process.
//!
//! `src/main.rs`'s unit tests prove that [`itsalive::Error::exit_code`] maps
//! every variant to the number `docs/ARCHITECTURE.md` §7 promises, but they
//! stop one step short of what the installer sees: the conversion into a
//! [`std::process::ExitCode`] and the wait status the shell reads back. These
//! tests run the binary itself and check that status.
//!
//! **Only argument errors are exercised here**, plus `--help`. Every other
//! subcommand opens `/dev/mem` or an i2c adapter, which would make the result
//! depend on the machine the tests run on (and, as root, would poke the build
//! host's hardware). Parsing happens before anything is opened — that is why
//! `src/main.rs` splits `parse` from `execute` — so every case below exits
//! without touching a device node.

use std::process::{Command, Output};

/// Run the binary Cargo just built for this test, with `args`.
fn itsalive(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_itsalive"))
        .args(args)
        .output()
        .expect("the binary Cargo built for this test should be runnable")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// T2.2's "Done when": `itsalive` with no args exits 2 with usage.
#[test]
fn no_arguments_exits_2_and_prints_usage() {
    let out = itsalive(&[]);
    assert_eq!(out.status.code(), Some(2));

    // The usage text goes to stderr, because stdout belongs to `probe`.
    let err = stderr(&out);
    assert!(err.contains("usage:"), "stderr was {err:?}");
    assert!(err.contains("itsalive probe"), "stderr was {err:?}");
    assert!(err.contains("itsalive up"), "stderr was {err:?}");
    assert!(
        stdout(&out).is_empty(),
        "usage must not go to stdout: {:?}",
        stdout(&out)
    );
}

/// Every shape of argument error is exit 2, and each one says which argument
/// it choked on before the usage text.
#[test]
fn every_argument_error_exits_2() {
    let cases: &[&[&str]] = &[
        &["bogus"],
        &["probe", "--jsonn"],
        &["hdmi", "--mode"],
        &["hdmi", "--mode", "1080p"],
        &["hdmi", "--on"],
        &["fb"],
        &["fb", "on"],
        &["fb", "disable", "--mode", "720p"],
        &["fb", "enable", "--mode", "240p"],
        &["say"],
        &["say", "--quiet", "hi"],
        &["up", "--off"],
        &["up", "--mode", "720i"],
        &["leds", "0xFF"],
        &["--help", "extra"],
    ];
    for args in cases {
        let out = itsalive(args);
        assert_eq!(out.status.code(), Some(2), "{args:?} should exit 2");
        assert!(
            stderr(&out).contains("usage:"),
            "{args:?} printed no usage: {:?}",
            stderr(&out)
        );
    }
}

/// `--help` is the one subcommand that writes to stdout and exits 0 without
/// opening a device.
#[test]
fn help_exits_0_on_stdout() {
    for args in [&["--help"], &["-h"]] {
        let out = itsalive(args);
        assert_eq!(out.status.code(), Some(0));
        let text = stdout(&out);
        assert!(text.contains("itsalive fb enable"), "stdout was {text:?}");
        assert!(text.contains("exit codes:"), "stdout was {text:?}");
        // `leds` is v1.1 and not implemented; the usage text must not offer it.
        assert!(!text.contains("leds"), "stdout was {text:?}");
        assert!(stderr(&out).is_empty());
    }
}

/// An argument that is not valid UTF-8 is a usage error and **not** a panic.
/// `std::env::args()` panics on one, which with `panic = "abort"` would be a
/// SIGABRT whose wait status is not a §7 code at all; `main` uses `args_os`
/// for exactly this reason.
#[test]
#[cfg(unix)]
fn a_non_utf8_argument_is_a_usage_error_not_a_crash() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let out = Command::new(env!("CARGO_BIN_EXE_itsalive"))
        .arg("hdmi")
        .arg("--mode")
        .arg(OsStr::from_bytes(b"\xff\xfe720p"))
        .output()
        .expect("the binary should run");

    assert_eq!(out.status.code(), Some(2), "signalled instead of exiting?");
    assert!(stderr(&out).contains("unknown mode"), "{:?}", stderr(&out));
}
