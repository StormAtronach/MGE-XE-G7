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
//! d3d8.dll calls [`run`] from its `DllMain`, under the loader lock of Windows. A thread
//! made there cannot start until the lock is free, so nothing here may make or wait for a
//! thread: the probe's output is read after the probe ends, on the calling thread. The
//! output is about 1 KB, inside the 4 KB of the pipe; a probe that filled the pipe would
//! block, reach the time limit and count as none (the first live run, 2026-10-27, hung in a
//! join of a reader thread here).

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

/// Runs the probe and reads its answer, on the calling thread only (see the module's head):
/// it waits for the probe to end, up to the limit, then reads the pipe to its end. A probe
/// that runs past the limit is ended, and what it wrote is read.
pub fn run(exe: &str, exit_gpu: i32, seconds: u64) -> Answer {
    let started = Instant::now();
    let mut child = match Command::new(exe).arg("--probe").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(error) => return Answer { usable: false, reason: format!("the probe did not start: {error}") },
    };
    let mut stdout = child.stdout.take().expect("the probe's output is piped");
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
    // The probe has ended, so its end of the pipe is closed and the read ends.
    let mut output = String::new();
    let _ = stdout.read_to_string(&mut output);
    answer(&output, code, exit_gpu, seconds)
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

/// The answer for this machine: the cache's when its key is `key`, else the probe's, which
/// then goes into the cache. A cache that cannot be written leaves the answer as it is; the
/// probe then runs again at the next start.
pub fn check(exe: &str, path: &str, key: &str, exit_gpu: i32, seconds: u64) -> (Answer, Source) {
    if let Some(cached) = read_cache(path).filter(|cached| cached.key == key) {
        return (cached.answer, Source::Cache);
    }
    let answer = run(exe, exit_gpu, seconds);
    let _ = std::fs::write(path, format_cache(key, &answer));
    (answer, Source::Probe)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The same key: the cache answers, no probe runs (the program is not there).
        let (answer, source) = check("a-program-that-is-not-there.exe", path, "key A", 20, 1);
        assert_eq!(source, Source::Cache);
        assert_eq!(answer, Answer { usable: false, reason: "lacks x".to_string() });
        // Another key: the probe runs (and fails to start: none), and the cache has the new key.
        let (answer, source) = check("a-program-that-is-not-there.exe", path, "key B", 20, 1);
        assert_eq!(source, Source::Probe);
        assert!(!answer.usable && answer.reason.starts_with("the probe did not start"), "{}", answer.reason);
        assert_eq!(read_cache(path).unwrap().key, "key B");
        let _ = std::fs::remove_file(path);
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
