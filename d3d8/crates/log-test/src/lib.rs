//! The tests of MGE XE's log (`d3d8/cpp/support/log.cpp`): a line longer than the log's buffer,
//! and the binary dump. Against the code before the fix the test fails at the long line's
//! length: its line end went to `buf + 5000`, past the 4096-byte buffer and past MSVC's stack
//! cookie, so the write corrupted the stack without a crash, and the file got no line end.

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};

    unsafe extern "C" {
        fn mge_log_test_write(path: *const c_char) -> c_int;
    }

    fn written() -> String {
        let path = std::env::temp_dir().join(format!("mge-log-test-{}.log", std::process::id()));
        let text = CString::new(path.to_str().unwrap()).unwrap();
        // SAFETY: a NUL-terminated path that lives through the call.
        assert_eq!(unsafe { mge_log_test_write(text.as_ptr()) }, 0, "the log at {} did not open", path.display());
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        String::from_utf8(bytes).unwrap()
    }

    /// One test, since the log is one file of the process. A line longer than the buffer is
    /// cut to 4091 characters and ends with its line end, and the next line follows it whole
    /// (before the fix the line end went to `buf + 5000`). The binary dump shows each byte of
    /// a line once, the last line only its own bytes (before the fix every place showed the
    /// byte after the line, `ptr[n]`).
    #[test]
    fn a_long_line_stays_inside_the_buffer_and_the_dump_shows_each_byte() {
        let text = written();
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines[0].len(), 4091);
        assert!(lines[0].bytes().all(|byte| byte == b'x'));
        assert_eq!(lines[1], "short 7");
        assert_eq!(lines[2], "  41 42 43 44 45 46 47 48 49 4A 4B 4C 4D 4E 4F 50     ABCDEFGHIJKLMNOP");
        assert_eq!(lines[3], "  51 52 53 54     QRST");
    }
}
