//! The probe of the wgpu renderer before d3d8.dll chooses its path: `numidium.exe --probe`
//! run with a pipe and a time limit, and its answer read from its lines.
//!
//! The probe prints one line for each backend it asks, then `probe: use <backend>` or
//! `probe: none: <reason>`, then its time, and exits with 0 or the code of a refusal
//! (`EXIT_GPU` of the host crate, `MGE_PROBE_EXIT_GPU` of `mgeversion.h`; the C++ side gives
//! the number). A crash of the probe counts as none, with its code, so that the game plays on
//! Direct3D 9 and the log says why (the failure-path page of the project, section 4). A probe
//! with no answer within the limit is ended and is not a none: the start tries the wgpu path,
//! and the next start asks again; a second "no answer" in a row for the same machine counts as
//! none ([`Verdict`]; the page's section 6 (a)). A crate of its own, so that the tests run: the
//! `d3d8` crate is the library the game loads and builds no test program.
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

/// How a run of the probe ended.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum End {
    /// The probe ended by itself, with this exit code.
    Code(i32),
    /// The probe gave no answer within the limit and was ended (TerminateProcess).
    Limit,
    /// The probe did not start (a scanner that holds the new copy of numidium.exe, for
    /// example): that says nothing about the machine.
    NotStarted,
}

/// One run of the probe: its answer, how it ended, and its lines for mgeXE.log
/// ([`probe_lines`]).
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub answer: Answer,
    pub end: End,
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
            return Run { answer, end: End::NotStarted, lines: Vec::new() };
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
    let end = code.map_or(End::Limit, End::Code);
    Run { answer: answer(&output, code, exit_gpu, seconds), end, lines: probe_lines(&output) }
}

/// What a cache line says about its machine.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Said {
    Usable,
    None,
    /// The last probe gave no answer within the limit: the next start asks again.
    NoAnswer,
}

impl Said {
    /// The word of the cache line.
    fn word(self) -> &'static str {
        match self {
            Said::Usable => "usable",
            Said::None => "none",
            Said::NoAnswer => "no answer",
        }
    }
}

/// The line that the cache file of the probe holds (`mgeXE-renderer-probe.txt` beside
/// mgeXE.log): the key, what the probe said and its reason, separated by tabs. The file is
/// read in `DllMain` (a file read only) and written after a probe.
#[derive(Debug, PartialEq, Eq)]
pub struct Cached {
    pub key: String,
    pub said: Said,
    pub reason: String,
}

/// The cache line of a file's text, or `None` for a text that is not one.
pub fn parse_cache(text: &str) -> Option<Cached> {
    let line = text.lines().next()?;
    let mut parts = line.splitn(3, '\t');
    let key = parts.next()?.to_string();
    let said = match parts.next()? {
        "usable" => Said::Usable,
        "none" => Said::None,
        "no answer" => Said::NoAnswer,
        _ => return None,
    };
    let reason = parts.next().unwrap_or("").to_string();
    (!key.is_empty()).then_some(Cached { key, said, reason })
}

/// The text of a cache file: one line. Tabs and line ends in the reason become spaces, so
/// that the line stays one line of three fields.
pub fn format_cache(key: &str, said: Said, reason: &str) -> String {
    let clean = |text: &str| text.replace(['\t', '\r', '\n'], " ");
    format!("{}\t{}\t{}\n", clean(key), said.word(), clean(reason))
}

/// The cache line of the file at `path`, or `None` when there is no such file or it is not
/// one.
pub fn read_cache(path: &str) -> Option<Cached> {
    parse_cache(&std::fs::read_to_string(path).ok()?)
}

/// What a start does about the wgpu path, as the number that d3d8.dll gets from
/// `mge_probe_check` (`MgeProbeVerdict` of `mge_probe_ffi.h`).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[repr(u32)]
pub enum Verdict {
    /// The probe said none or crashed: Direct3D 9, and the box.
    ProbeNone = 0,
    /// The probe said usable: the wgpu path.
    ProbeUsable = 1,
    /// The cache said none for this machine: Direct3D 9 (DllMain chose it), no box.
    CachedNone = 2,
    /// The cache said usable for this machine: the wgpu path, no probe.
    CachedUsable = 3,
    /// The probe gave no answer within the limit, the first time for this machine: it was
    /// ended, and the start tries the wgpu path. A GPU that cannot run it gets the live
    /// helper's refusal (code 20).
    NoAnswer = 4,
    /// The probe gave no answer twice in a row for this machine: none, with a box that says
    /// so, so that a probe that hangs on a broken driver cannot keep every start blind.
    NoAnswerTwice = 5,
    /// The probe did not start (another program, a virus scanner for example, can hold a new
    /// copy of numidium.exe): Direct3D 9 for this start, with a box that says so and blames
    /// no graphics card; not cached, so the next start asks again.
    NotStarted = 6,
}

impl Verdict {
    /// Every verdict, in the order of its number.
    pub const ALL: [Verdict; 7] = [
        Verdict::ProbeNone,
        Verdict::ProbeUsable,
        Verdict::CachedNone,
        Verdict::CachedUsable,
        Verdict::NoAnswer,
        Verdict::NoAnswerTwice,
        Verdict::NotStarted,
    ];

    /// Whether the start takes the wgpu path.
    pub fn wgpu(self) -> bool {
        matches!(self, Verdict::ProbeUsable | Verdict::CachedUsable | Verdict::NoAnswer)
    }

    /// Whether the start shows a box.
    pub fn shows_box(self) -> bool {
        matches!(self, Verdict::ProbeNone | Verdict::NoAnswerTwice | Verdict::NotStarted)
    }
}

/// The result of [`check`]: the verdict, its reason for the log and the box, and the probe's
/// lines for mgeXE.log (none when no probe ran).
#[derive(Debug, PartialEq, Eq)]
pub struct Outcome {
    pub verdict: Verdict,
    pub reason: String,
    pub lines: Vec<String>,
}

/// The verdict for this machine: the cache's when its key is `key` and it holds an answer,
/// else the probe's ([`decide`]), whose cache line then goes into the file. A cache that
/// cannot be written leaves the verdict as it is; the probe then runs again at the next start.
pub fn check(exe: &str, path: &str, key: &str, exit_gpu: i32, seconds: u64) -> Outcome {
    check_with(path, key, seconds, || run_full(exe, exit_gpu, seconds))
}

/// [`check`] with the probe as a function that gives its run.
fn check_with(path: &str, key: &str, seconds: u64, probe: impl FnOnce() -> Run) -> Outcome {
    let no_answer_before = match read_cache(path).filter(|cached| cached.key == key) {
        Some(Cached { said: Said::Usable, reason, .. }) => return Outcome { verdict: Verdict::CachedUsable, reason, lines: Vec::new() },
        Some(Cached { said: Said::None, reason, .. }) => return Outcome { verdict: Verdict::CachedNone, reason, lines: Vec::new() },
        Some(Cached { said: Said::NoAnswer, .. }) => true,
        None => false,
    };
    let run = probe();
    let (verdict, reason, line) = decide(no_answer_before, &run, seconds);
    if let Some((said, cached_reason)) = line {
        let _ = std::fs::write(path, format_cache(key, said, &cached_reason));
    }
    Outcome { verdict, reason, lines: run.lines }
}

/// The decision after a probe ran: from how the probe ended and whether the cache said "no
/// answer" for this machine before, the verdict, its reason, and the cache line to write
/// (what it says and its reason; `None`: the file stays as it is).
///
/// - An answer (an exit code): usable or none, cached.
/// - No answer within the limit: the first time the wgpu path is tried and "no answer" is
///   cached, so that the next start, warm, asks again; the second time in a row it is none.
/// - No start: none for this start, not cached: a scanner that holds a new copy of
///   numidium.exe says nothing about the machine.
fn decide(no_answer_before: bool, run: &Run, seconds: u64) -> (Verdict, String, Option<(Said, String)>) {
    let answer = &run.answer;
    match run.end {
        End::Code(_) => {
            let (verdict, said) = if answer.usable { (Verdict::ProbeUsable, Said::Usable) } else { (Verdict::ProbeNone, Said::None) };
            (verdict, answer.reason.clone(), Some((said, answer.reason.clone())))
        }
        End::Limit if no_answer_before => {
            let reason = format!("the probe gave no answer twice in a row ({seconds} s each)");
            (Verdict::NoAnswerTwice, reason.clone(), Some((Said::None, reason)))
        }
        End::Limit => (Verdict::NoAnswer, format!("{}; the wgpu path is tried", answer.reason), Some((Said::NoAnswer, answer.reason.clone()))),
        End::NotStarted => (Verdict::NotStarted, format!("{}; not cached, the next start probes again", answer.reason), None),
    }
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
        assert_eq!(run.end, End::Code(0), "{:?}", run.answer);
        assert_eq!(run.answer, Answer { usable: true, reason: "use vulkan".to_string() });
        // Only the probe's own line goes to the log, not the 4000 others.
        assert_eq!(run.lines, vec!["probe: use vulkan".to_string()]);
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    /// A folder of its own for a test's cache file, and the file's path.
    fn cache_file(test: &str) -> (std::path::PathBuf, String) {
        let folder = std::env::temp_dir().join(format!("mge-probe-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("cache.txt").to_str().unwrap().to_string();
        let _ = std::fs::remove_file(&path);
        (folder, path)
    }

    /// The probe's runs of the decision table, with no lines.
    fn ran(usable: bool, reason: &str, end: End) -> Run {
        Run { answer: Answer { usable, reason: reason.to_string() }, end, lines: Vec::new() }
    }
    fn usable() -> Run {
        ran(true, "use dx12", End::Code(0))
    }
    fn none() -> Run {
        ran(false, "dx12: lacks x", End::Code(20))
    }
    fn silent() -> Run {
        ran(false, "the probe gave no answer in 5 s", End::Limit)
    }
    fn unstarted() -> Run {
        ran(false, "the probe did not start: access denied", End::NotStarted)
    }

    /// The decision table of a start (the failure-path page, section 6 (a)): the cache line of
    /// this machine before and how the probe ended (no probe: the cache answers) give the
    /// verdict, its path and its box, and the cache line after.
    #[test]
    fn the_decisions_of_a_start() {
        use Verdict::*;
        let (folder, path) = cache_file("table");
        type Row = (Option<Said>, Option<fn() -> Run>, Verdict, bool, bool, Option<Said>);
        #[rustfmt::skip]
        let table: [Row; 10] = [
            // before,              probe,            verdict,       wgpu,  box,   after
            (None,                  Some(usable),     ProbeUsable,   true,  false, Some(Said::Usable)),
            (None,                  Some(none),       ProbeNone,     false, true,  Some(Said::None)),
            (None,                  Some(silent),     NoAnswer,      true,  false, Some(Said::NoAnswer)),
            (None,                  Some(unstarted),  NotStarted,    false, true,  None),
            (Some(Said::Usable),    None,             CachedUsable,  true,  false, Some(Said::Usable)),
            (Some(Said::None),      None,             CachedNone,    false, false, Some(Said::None)),
            (Some(Said::NoAnswer),  Some(usable),     ProbeUsable,   true,  false, Some(Said::Usable)),
            (Some(Said::NoAnswer),  Some(none),       ProbeNone,     false, true,  Some(Said::None)),
            (Some(Said::NoAnswer),  Some(silent),     NoAnswerTwice, false, true,  Some(Said::None)),
            (Some(Said::NoAnswer),  Some(unstarted),  NotStarted,    false, true,  Some(Said::NoAnswer)),
        ];
        for (row, (before, probe, verdict, wgpu, shows_box, after)) in table.into_iter().enumerate() {
            let _ = std::fs::remove_file(&path);
            if let Some(said) = before {
                std::fs::write(&path, format_cache("key", said, "before")).unwrap();
            }
            let outcome = check_with(&path, "key", 5, || match probe {
                Some(probe) => probe(),
                None => panic!("row {row}: a probe ran though the cache had the answer"),
            });
            assert_eq!(outcome.verdict, verdict, "row {row}");
            assert_eq!((outcome.verdict.wgpu(), outcome.verdict.shows_box()), (wgpu, shows_box), "row {row}");
            assert_eq!(read_cache(&path).map(|cached| cached.said), after, "row {row}");
        }

        // The reasons of the two verdicts of a silent probe, in the log, the box and the cache.
        let _ = std::fs::remove_file(&path);
        let first = check_with(&path, "key", 5, silent);
        assert_eq!(first.reason, "the probe gave no answer in 5 s; the wgpu path is tried");
        assert_eq!(read_cache(&path).unwrap().reason, "the probe gave no answer in 5 s");
        let second = check_with(&path, "key", 5, silent);
        assert_eq!(second.reason, "the probe gave no answer twice in a row (5 s each)");
        assert_eq!(read_cache(&path).unwrap().reason, "the probe gave no answer twice in a row (5 s each)");

        // A "no answer" of another machine (another key) is no line for this one: the first
        // silence of this machine tries the wgpu path.
        std::fs::write(&path, format_cache("another key", Said::NoAnswer, "before")).unwrap();
        assert_eq!(check_with(&path, "key", 5, silent).verdict, NoAnswer);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&folder);
    }

    /// The C++ reads the verdict as `MgeProbeVerdict` of mge_probe_ffi.h: each value there is
    /// the number of the verdict of the same name here, and the two lists are the same.
    #[test]
    fn the_verdicts_of_the_cpp_are_the_ones_of_this_crate() {
        let header = include_str!("../../../cpp/mge_probe_ffi.h");
        let mut cpp: Vec<(String, u32)> = header
            .lines()
            .filter_map(|line| {
                let (name, value) = line.trim().strip_suffix(',')?.split_once(" = ")?;
                name.starts_with("MgeProbe").then(|| (name.to_string(), value.trim().parse().expect("a number")))
            })
            .collect();
        // The C++ names drop the word "Probe" of a verdict that starts with it.
        let mut rust: Vec<(String, u32)> = Verdict::ALL.iter().map(|&verdict| (format!("MgeProbe{}", format!("{verdict:?}").trim_start_matches("Probe")), verdict as u32)).collect();
        cpp.sort();
        rust.sort();
        assert_eq!(cpp, rust);

        // ALL is a list by hand: each verdict in it once, in the order of its number. The
        // match has no wildcard, so a new verdict does not compile here until it is named,
        // and then the count and the order below ask for it in ALL.
        let last = |verdict: Verdict| match verdict {
            Verdict::ProbeNone | Verdict::ProbeUsable | Verdict::CachedNone | Verdict::CachedUsable | Verdict::NoAnswer | Verdict::NoAnswerTwice => false,
            Verdict::NotStarted => true,
        };
        assert!(last(Verdict::ALL[Verdict::ALL.len() - 1]));
        assert_eq!(Verdict::ALL.len(), Verdict::NotStarted as usize + 1);
        for (number, verdict) in Verdict::ALL.iter().enumerate() {
            assert_eq!(*verdict as usize, number, "{verdict:?}");
        }
    }

    #[test]
    fn a_cache_line_reads_back_as_written() {
        let key = "NVIDIA GeForce RTX 5070 32.0.16.1714; numidium.exe 46896640 133000";
        let text = format_cache(key, Said::Usable, "use vulkan");
        assert_eq!(parse_cache(&text), Some(Cached { key: key.to_string(), said: Said::Usable, reason: "use vulkan".to_string() }));
        let read = parse_cache(&format_cache("k", Said::None, "dx12: lacks\tthe arrays\nand more")).unwrap();
        assert_eq!((read.said, read.reason.as_str()), (Said::None, "dx12: lacks the arrays and more"));
        let read = parse_cache(&format_cache("k", Said::NoAnswer, "the probe gave no answer in 5 s")).unwrap();
        assert_eq!(read.said, Said::NoAnswer);
        assert_eq!(text.lines().count(), 1);
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
        let (folder, path) = cache_file("keys");
        std::fs::write(&path, format_cache("key A", Said::None, "lacks x")).unwrap();
        // The same key: the cache answers, no probe runs (the program is not there), and there
        // are no probe lines for the log.
        let outcome = check("a-program-that-is-not-there.exe", &path, "key A", 20, 1);
        assert_eq!(outcome, Outcome { verdict: Verdict::CachedNone, reason: "lacks x".to_string(), lines: Vec::new() });
        // Another key: the probe runs and answers with its lines; the cache has the new key
        // and the answer, one line, and not the probe's lines.
        let lines = probe_lines(OK);
        let outcome = check_with(&path, "key B", 5, || Run { lines: lines.clone(), ..usable() });
        assert_eq!(outcome.verdict, Verdict::ProbeUsable);
        assert_eq!(outcome.lines, lines);
        assert_eq!(read_cache(&path).unwrap().key, "key B");
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&folder);
    }

    /// A probe that cannot start is none for this start and is not cached (a scanner can hold
    /// a new copy of numidium.exe at its first start): the cache keeps what it had.
    #[test]
    fn a_probe_that_cannot_start_is_not_cached() {
        let (folder, path) = cache_file("start");
        let outcome = check("a-program-that-is-not-there.exe", &path, "key", 20, 1);
        assert_eq!(outcome.verdict, Verdict::NotStarted);
        assert!(outcome.reason.starts_with("the probe did not start"), "{}", outcome.reason);
        assert!(outcome.reason.ends_with("; not cached, the next start probes again"), "{}", outcome.reason);
        assert!(read_cache(&path).is_none(), "a probe that did not start was cached");
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

    /// A crash is a none with its code; a silence has its own reason (its verdict is
    /// [`decide`]'s).
    #[test]
    fn a_crash_and_a_silence_have_their_cause() {
        // A fast fail of a Rust panic, as Windows reports it (0xC0000409 as a signed code).
        assert_eq!(answer("", Some(-1073740791), 20, 5).reason, "the probe ended with the code 0xc0000409 and no answer");
        assert!(!answer("", Some(-1073740791), 20, 5).usable);
        assert_eq!(answer("probe: use vulkan\n", Some(3), 20, 5).reason, "the probe said \"use vulkan\" and ended with the code 0x3");
        assert_eq!(answer("probe: vulkan: A card: lacks x\n", None, 20, 5).reason, "the probe gave no answer in 5 s");
    }

    /// A probe that runs past the limit is ended (TerminateProcess), so that no probe runs
    /// beside the live helper, and its run ends at the limit, not at its own end.
    #[test]
    fn a_probe_past_the_limit_is_ended() {
        // ping is one process of its own: about 9 s for 10 echoes.
        let mut command = Command::new("ping");
        command.args(["-n", "10", "127.0.0.1"]);
        let started = Instant::now();
        let run = run_command(command, 20, 1);
        assert_eq!(run.end, End::Limit);
        assert_eq!(run.answer.reason, "the probe gave no answer in 1 s");
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
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
