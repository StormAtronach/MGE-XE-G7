//! The tests of MGE XE's log (`d3d8/cpp/support/log.cpp`): a line longer than the log's buffer,
//! the binary dump, the log of the run before as `.1`, and the plugin's lines through the export
//! `MGE_LogLine`. Against the code before the fix the test fails at the long line's
//! length: its line end went to `buf + 5000`, past the 4096-byte buffer and past MSVC's stack
//! cookie, so the write corrupted the stack without a crash, and the file got no line end.

#[cfg(test)]
mod tests {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int};

    unsafe extern "C" {
        fn mge_log_test_write(path: *const c_char) -> c_int;
        fn mge_log_test_run(path: *const c_char, line: *const c_char) -> c_int;
        fn mge_log_test_plugin_text(path: *const c_char, text: *const c_char) -> c_int;
        fn mge_log_test_two_threads(path: *const c_char, count: c_int) -> c_int;
    }

    /// The log is one file of the process (one handle in log.cpp): the tests take turns.
    static ONE_LOG: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _log = ONE_LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let text = written();
        let lines: Vec<&str> = text.split("\r\n").collect();
        assert_eq!(lines[0].len(), 4091);
        assert!(lines[0].bytes().all(|byte| byte == b'x'));
        assert_eq!(lines[1], "short 7");
        assert_eq!(lines[2], "  41 42 43 44 45 46 47 48 49 4A 4B 4C 4D 4E 4F 50     ABCDEFGHIJKLMNOP");
        assert_eq!(lines[3], "  51 52 53 54     QRST");
    }

    /// One run of the log at `path`, with `line` in it.
    fn run(path: &std::path::Path, line: &str) {
        let (path_text, line_text) = (CString::new(path.to_str().unwrap()).unwrap(), CString::new(line).unwrap());
        // SAFETY: NUL-terminated strings that live through the call.
        assert_eq!(unsafe { mge_log_test_run(path_text.as_ptr(), line_text.as_ptr()) }, 0, "the log at {} did not open", path.display());
    }

    /// The log keeps the run before as `.1`, in place of the one before that; the first run
    /// makes no `.1` (before the change each open erased the run before: `CREATE_ALWAYS`).
    #[test]
    fn the_log_keeps_the_run_before_as_dot_1() {
        let _log = ONE_LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let folder = std::env::temp_dir().join(format!("mge-log-runs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("mgeXE.log");
        let before = folder.join("mgeXE.log.1");
        let read = |path: &std::path::Path| std::fs::read_to_string(path).unwrap();

        run(&path, "run 1");
        assert_eq!(read(&path), "run 1\r\n");
        assert!(!before.exists(), "the first run made a .1");
        run(&path, "run 2");
        assert_eq!((read(&path).as_str(), read(&before).as_str()), ("run 2\r\n", "run 1\r\n"));
        run(&path, "run 3");
        assert_eq!((read(&path).as_str(), read(&before).as_str()), ("run 3\r\n", "run 2\r\n"));

        std::fs::remove_dir_all(&folder).unwrap();
    }

    /// A log file of its own for one test, in a folder of its own.
    fn test_log(name: &str) -> (std::path::PathBuf, CString) {
        let folder = std::env::temp_dir().join(format!("mge-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("mgeXE.log");
        let text = CString::new(path.to_str().unwrap()).unwrap();
        (path, text)
    }

    /// Each line of the plugin's text gets the prefix and the log's line end, a text that ends
    /// with its line end has no empty line after it, a CRLF inside keeps one line end, a long
    /// line is whole (the log's own lines stop at 4091 characters), and a null text writes
    /// nothing.
    #[test]
    fn the_plugin_lines_get_the_prefix_each() {
        let _log = ONE_LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (path, path_text) = test_log("plugin");
        let long = "y".repeat(6000);
        let text = CString::new(format!("own device: made\nstream: two\r\nlines\n{long}\n")).unwrap();
        // SAFETY: NUL-terminated strings that live through the call.
        assert_eq!(unsafe { mge_log_test_plugin_text(path_text.as_ptr(), text.as_ptr()) }, 0);
        let written = std::fs::read_to_string(&path).unwrap();
        let expected = format!("before\r\nnirender: own device: made\r\nnirender: stream: two\r\nnirender: lines\r\nnirender: {long}\r\nafter\r\n");
        assert_eq!(written, expected);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// Two threads write at once: every line of the file is one whole line of one of them,
    /// and every line is there.
    #[test]
    fn the_lines_of_two_threads_do_not_mix() {
        let _log = ONE_LOG.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (path, path_text) = test_log("threads");
        // SAFETY: a NUL-terminated string that lives through the call.
        assert_eq!(unsafe { mge_log_test_two_threads(path_text.as_ptr(), 2000) }, 0);
        let written = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = written.split_terminator("\r\n").collect();
        let plugin = lines.iter().filter(|line| **line == "nirender: plugin line of the cell worker").count();
        let own = lines.iter().filter(|line| **line == "own line of MGE XE").count();
        assert_eq!((plugin, own, lines.len()), (2000, 2000, 4000), "a line mixed with another");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
