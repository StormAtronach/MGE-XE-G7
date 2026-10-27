//! The probe of the wgpu renderer before d3d8.dll chooses its path: `numidium.exe --probe`
//! run with a pipe and a time limit, and its answer read from its lines.
//!
//! The probe prints one line for each backend it asks, then `probe: use <backend>` or
//! `probe: none: <reason>`, then its time, and exits with 0 or the code of a refusal
//! (`EXIT_GPU` of the host crate, `MGE_PROBE_EXIT_GPU` of `mgeversion.h`; the C++ side gives
//! the number). Any other end counts as none: a crash of the probe with its code, or no
//! answer within the limit, so that the game plays on Direct3D 9 and the log says why
//! (the failure-path page of the project, section 4). A crate of its own, so that the
//! tests run: the `d3d8` crate is the library the game loads and builds no test program.
//!
//! Never call [`run`] or [`check`] from `DllMain`: a child process and a wait do not belong
//! under the loader lock, and a thread made there cannot start until the lock is free (the
//! first live run, 2026-10-27, hung in a join of the reader thread). d3d8.dll calls them when
//! the plugin or the game first asks for the wgpu path; `DllMain` reads only the cache
//! ([`read_cache`]). The probe's output is read on a thread of its own while the probe runs,
//! since a probe that writes more than the pipe holds (about 4 KB) blocks until it is read.
//! The probe starts no process of its own, so no other process holds the pipe's write end:
//! the reader ends when the probe ends. The probe's own lines go back with its answer, for
//! mgeXE.log; the cache file holds one line only, the key and the answer.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What the probe said: usable or not, and the line that says why (the answer line of the
/// probe, or what ended it).
#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    pub usable: bool,
    pub reason: String,
}

/// One run of the probe: its answer, its end (the exit code, or `None` when it did not start
/// or was ended at the limit), and its lines for mgeXE.log ([`probe_lines`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub answer: Answer,
    pub end: Option<i32>,
    pub lines: Vec<String>,
}

/// The most lines of a probe that go into mgeXE.log, and the most bytes of one line. The
/// probe prints one line for each backend, the answer and the time: four lines at most today.
const MOST_LINES: usize = 16;
const MOST_LINE_BYTES: usize = 600;

/// The probe's own lines of its output (those that start with `probe: `), for mgeXE.log: at
/// most [`MOST_LINES`], each cut at [`MOST_LINE_BYTES`]. Other output (a driver's or a
/// library's) stays out.
pub fn probe_lines(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim_end)
        .filter(|line| line.starts_with("probe: "))
        .take(MOST_LINES)
        .map(|line| {
            let mut end = line.len().min(MOST_LINE_BYTES);
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            line[..end].to_string()
        })
        .collect()
}

/// The answer from the probe's output and its end. `code` is the exit code, `None` when
/// the probe did not end within the limit.
pub fn answer(output: &str, code: Option<i32>, exit_gpu: i32, seconds: u64) -> Answer {
    let last = output.lines().rev().find_map(|line| {
        let line = line.trim_end();
        line.strip_prefix("probe: use ").map(|backend| (true, format!("use {backend}"))).or_else(|| line.strip_prefix("probe: none: ").map(|reason| (false, reason.to_string())))
    });
    match (last, code) {
        (Some((true, reason)), Some(0)) => Answer { usable: true, reason },
        (Some((false, reason)), Some(code)) if code == exit_gpu => Answer { usable: false, reason },
        (Some((_, reason)), Some(code)) => Answer { usable: false, reason: format!("the probe said \"{reason}\" and ended with the code {code:#x}") },
        (None, Some(code)) => Answer { usable: false, reason: format!("the probe ended with the code {code:#x} and no answer") },
        (_, None) => Answer { usable: false, reason: format!("the probe gave no answer in {seconds} s") },
    }
}

/// Runs `exe --probe` and reads its answer. A probe that runs past the limit is ended, and
/// what it wrote is read.
pub fn run(exe: &str, exit_gpu: i32, seconds: u64) -> Answer {
    run_full(exe, exit_gpu, seconds).answer
}

/// [`run`] with the probe's end and its lines.
fn run_full(exe: &str, exit_gpu: i32, seconds: u64) -> Run {
    let mut command = Command::new(exe);
    command.arg("--probe");
    run_command(command, exit_gpu, seconds)
}

/// Runs a probe's command: its output is read on a thread of its own while the probe runs
/// (see the module's head), the wait is up to `seconds`, then the answer of its lines.
fn run_command(mut command: Command, exit_gpu: i32, seconds: u64) -> Run {
    let started = Instant::now();
    let mut child = match command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => child,
        // No end of its own, as for a probe ended at the limit: a start that failed (a scanner
        // that holds the new copy of numidium.exe) says nothing about the machine.
        Err(error) => {
            let answer = Answer { usable: false, reason: format!("the probe did not start: {error}") };
            return Run { answer, end: None, lines: Vec::new() };
        }
    };
    let mut stdout = child.stdout.take().expect("the probe's output is piped");
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        String::from_utf8_lossy(&output).into_owned()
    });
    let limit = Duration::from_secs(seconds);
    // The exit code, or `None` for a probe ended at the limit.
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status.code().unwrap_or(-1)),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(_) => break Some(-1),
        }
    };
    // The probe has ended, so its end of the pipe is closed and the reader ends.
    let output = reader.join().unwrap_or_default();
    Run { answer: answer(&output, code, exit_gpu, seconds), end: code, lines: probe_lines(&output) }
}

/// The answer that the cache file of the probe holds (`mgeXE-renderer-probe.txt` beside
/// mgeXE.log): one line of the key, the answer and its reason, separated by tabs. The file is
/// read in `DllMain` (a file read only) and written after a probe.
#[derive(Debug, PartialEq, Eq)]
pub struct Cached {
    pub key: String,
    pub answer: Answer,
}

/// The cached answer of a file's text, or `None` for a text that is not one.
pub fn parse_cache(text: &str) -> Option<Cached> {
    let line = text.lines().next()?;
    let mut parts = line.splitn(3, '\t');
    let key = parts.next()?.to_string();
    let usable = match parts.next()? {
        "usable" => true,
        "none" => false,
        _ => return None,
    };
    let reason = parts.next().unwrap_or("").to_string();
    (!key.is_empty()).then_some(Cached { key, answer: Answer { usable, reason } })
}

/// The text of a cache file: the line of the key and the answer. Tabs and line ends in the
/// reason become spaces, so that the line stays one line of three fields.
pub fn format_cache(key: &str, answer: &Answer) -> String {
    let clean = |text: &str| text.replace(['\t', '\r', '\n'], " ");
    format!("{}\t{}\t{}\n", clean(key), if answer.usable { "usable" } else { "none" }, clean(&answer.reason))
}

/// The cached answer of the file at `path`, or `None` when there is no such file or it is
/// not one.
pub fn read_cache(path: &str) -> Option<Cached> {
    parse_cache(&std::fs::read_to_string(path).ok()?)
}

/// Where an answer of [`check`] came from.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Source {
    /// The cache had the same key: no probe ran.
    Cache,
    /// The key differed or there was no cache: the probe ran and the cache has its answer now.
    Probe,
}

/// The result of [`check`]: the answer, where it came from, and the probe's lines for
/// mgeXE.log (none for a cached answer). The cache file holds the key and the answer only.
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub answer: Answer,
    pub source: Source,
    pub lines: Vec<String>,
}

/// The answer for this machine: the cache's when its key is `key`, else the probe's, which
/// then goes into the cache. A cache that cannot be written leaves the answer as it is; the
/// probe then runs again at the next start.
pub fn check(exe: &str, path: &str, key: &str, exit_gpu: i32, seconds: u64) -> Outcome {
    check_with(path, key, || run_full(exe, exit_gpu, seconds))
}

/// [`check`] with the probe as a function that gives its run. A probe that did not start or
/// was ended at the limit gave no answer of its own: its none is used for this start and not
/// written into the cache, so that a slow or blocked start never stores a none for the
/// machine; its reason says so, for the log line.
fn check_with(path: &str, key: &str, probe: impl FnOnce() -> Run) -> Outcome {
    if let Some(cached) = read_cache(path).filter(|cached| cached.key == key) {
        return Outcome { answer: cached.answer, source: Source::Cache, lines: Vec::new() };
    }
    let Run { mut answer, end, lines } = probe();
    if end.is_some() {
        let _ = std::fs::write(path, format_cache(key, &answer));
    } else {
        answer.reason.push_str("; not cached, the next start probes again");
    }
    Outcome { answer, source: Source::Probe, lines }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe that writes far more than the pipe holds still answers: the reader drains the
    /// pipe while the probe runs (the Reviewer's case: a full pipe blocked the probe until the
    /// limit, and a usable GPU became a none).
    #[test]
    fn a_probe_with_more_than_64_kb_of_output_answers() {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd");
        // 4000 lines of 40 bytes: 160 KB, then the answer line.
        command.raw_arg("/C \"(for /L %i in (1,1,4000) do @echo line %i of a long output of the probe) & echo probe: use vulkan\"");
        let started = Instant::now();
        let run = run_command(command, 20, 30);
        assert_eq!(run.end, Some(0), "{:?}", run.answer);
        assert_eq!(run.answer, Answer { usable: true, reason: "use vulkan".to_string() });
        // Only the probe's own line goes to the log, not the 4000 others.
        assert_eq!(run.lines, vec!["probe: use vulkan".to_string()]);
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// A run with no lines, for the tests of the cache.
    fn ran(answer: Answer, end: Option<i32>) -> Run {
        Run { answer, end, lines: Vec::new() }
    }

    /// A probe ended at the limit is not written into the cache; a probe that answered is.
    #[test]
    fn a_probe_ended_at_the_limit_is_not_cached() {
        let folder = std::env::temp_dir().join(format!("mge-probe-limit-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("cache.txt");
        let path = path.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        let outcome = check_with(path, "key", || ran(answer("", None, 20, 5), None));
        assert_eq!(outcome.source, Source::Probe);
        assert_eq!(outcome.answer.reason, "the probe gave no answer in 5 s; not cached, the next start probes again");
        assert!(read_cache(path).is_none(), "a probe ended at the limit was cached");
        let outcome = check_with(path, "key", || ran(Answer { usable: true, reason: "use dx12".to_string() }, Some(0)));
        assert_eq!(outcome.source, Source::Probe);
        assert_eq!(read_cache(path).unwrap().answer.reason, "use dx12");
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(&folder);
    }

    #[test]
    fn a_cache_line_reads_back_as_written() {
        let usable = Answer { usable: true, reason: "use vulkan".to_string() };
        let text = format_cache("NVIDIA GeForce RTX 5070 32.0.16.1714; numidium.exe 46896640 133000", &usable);
        assert_eq!(parse_cache(&text), Some(Cached { key: "NVIDIA GeForce RTX 5070 32.0.16.1714; numidium.exe 46896640 133000".to_string(), answer: usable }));
        let none = Answer { usable: false, reason: "dx12: lacks\tthe arrays\nand more".to_string() };
        let read = parse_cache(&format_cache("k", &none)).unwrap();
        assert_eq!(read.answer.reason, "dx12: lacks the arrays and more");
        assert!(!read.answer.usable);
    }

    #[test]
    fn a_text_that_is_not_a_cache_line_is_none() {
        assert_eq!(parse_cache(""), None);
        assert_eq!(parse_cache("key\tmaybe\treason\n"), None);
        assert_eq!(parse_cache("\tusable\tx\n"), None);
        assert_eq!(parse_cache("only a key\n"), None);
    }

    #[test]
    fn the_cache_answers_for_its_key_and_the_probe_for_another() {
        let folder = std::env::temp_dir().join(format!("mge-probe-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("cache.txt");
        let path = path.to_str().unwrap();
        std::fs::write(path, format_cache("key A", &Answer { usable: false, reason: "lacks x".to_string() })).unwrap();
        // The same key: the cache answers, no probe runs (the program is not there), and there
        // are no probe lines for the log.
        let outcome = check("a-program-that-is-not-there.exe", path, "key A", 20, 1);
        assert_eq!(outcome.source, Source::Cache);
        assert_eq!(outcome.answer, Answer { usable: false, reason: "lacks x".to_string() });
        assert!(outcome.lines.is_empty());
        // Another key: the probe runs and answers with its lines; the cache has the new key
        // and the answer, one line, and not the probe's lines.
        let lines = probe_lines(OK);
        let outcome = check_with(path, "key B", || Run { answer: Answer { usable: true, reason: "use vulkan".to_string() }, end: Some(0), lines: lines.clone() });
        assert_eq!(outcome.source, Source::Probe);
        assert!(outcome.answer.usable);
        assert_eq!(outcome.lines, lines);
        assert_eq!(read_cache(path).unwrap().key, "key B");
        assert_eq!(std::fs::read_to_string(path).unwrap().lines().count(), 1);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(&folder);
    }

    /// A probe that cannot start is none for this start and is not cached (a scanner can hold
    /// a new copy of numidium.exe at its first start): the cache keeps what it had.
    #[test]
    fn a_probe_that_cannot_start_is_not_cached() {
        let folder = std::env::temp_dir().join(format!("mge-probe-start-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("cache.txt");
        let path = path.to_str().unwrap();
        let _ = std::fs::remove_file(path);
        let Outcome { answer, source, .. } = check("a-program-that-is-not-there.exe", path, "key", 20, 1);
        assert_eq!(source, Source::Probe);
        assert!(!answer.usable && answer.reason.starts_with("the probe did not start"), "{}", answer.reason);
        assert!(answer.reason.ends_with("; not cached, the next start probes again"), "{}", answer.reason);
        assert!(read_cache(path).is_none(), "a probe that did not start was cached");
        let _ = std::fs::remove_dir(&folder);
    }

    const OK: &str = "nirender-host v0.1.0\nprobe: vulkan: NVIDIA GeForce RTX 5070 (Vulkan, driver NVIDIA 617.14): ok; binding arrays 16384 textures and 64 samplers (needs 256 and 8), BC yes, sampled textures 524288 (needs 32), depth Depth32FloatStencil8\nprobe: use vulkan\nprobe: 0.26 s\n";
    const NONE: &str = "nirender-host v0.1.0\nprobe: vulkan: A card: lacks room in the binding arrays: has 1 textures and 64 samplers, needs 256 and 8\nprobe: dx12: A card: lacks room in the binding arrays: has 1 textures and 64 samplers, needs 256 and 8\nprobe: none: vulkan: A card: lacks room in the binding arrays: has 1 textures and 64 samplers, needs 256 and 8; dx12: A card: lacks room in the binding arrays: has 1 textures and 64 samplers, needs 256 and 8\nprobe: 0.34 s\n";

    #[test]
    fn a_usable_gpu_names_its_backend() {
        assert_eq!(answer(OK, Some(0), 20, 5), Answer { usable: true, reason: "use vulkan".to_string() });
    }

    #[test]
    fn a_refusal_carries_the_reason_of_the_probe() {
        let answer = answer(NONE, Some(20), 20, 5);
        assert!(!answer.usable);
        assert!(answer.reason.starts_with("vulkan: A card: lacks room in the binding arrays: has 1 textures"), "{}", answer.reason);
    }

    #[test]
    fn a_crash_and_a_silence_count_as_none_with_their_cause() {
        // A fast fail of a Rust panic, as Windows reports it (0xC0000409 as a signed code).
        assert_eq!(answer("", Some(-1073740791), 20, 5).reason, "the probe ended with the code 0xc0000409 and no answer");
        assert!(!answer("", Some(-1073740791), 20, 5).usable);
        assert_eq!(answer("probe: use vulkan\n", Some(3), 20, 5).reason, "the probe said \"use vulkan\" and ended with the code 0x3");
        assert_eq!(answer("probe: vulkan: A card: lacks x\n", None, 20, 5).reason, "the probe gave no answer in 5 s");
    }

    #[test]
    fn the_log_gets_the_probes_own_lines() {
        let lines = probe_lines(NONE);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines.iter().all(|line| line.starts_with("probe: ")));
        assert_eq!(lines[3], "probe: 0.34 s");
        // At most 16 lines of at most 600 bytes, cut at a character's boundary.
        let long = format!("probe: {}\n", "\u{e9}".repeat(400)).repeat(20);
        let lines = probe_lines(&long);
        assert_eq!(lines.len(), 16);
        assert!(lines.iter().all(|line| line.len() <= 600 && line.len() >= 599), "{}", lines[0].len());
    }

    #[test]
    fn the_last_answer_line_counts() {
        let twice = "probe: none: first\nprobe: use dx12\nprobe: 0.3 s\n";
        assert_eq!(answer(twice, Some(0), 20, 5), Answer { usable: true, reason: "use dx12".to_string() });
    }

    #[test]
    fn a_probe_that_does_not_start_is_none() {
        let answer = run("a-program-that-is-not-there.exe", 20, 1);
        assert!(!answer.usable);
        assert!(answer.reason.starts_with("the probe did not start"), "{}", answer.reason);
    }
}
