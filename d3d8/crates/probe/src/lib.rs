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

/// Runs the probe and reads its answer. The output is read to its end; a probe that runs
/// past the limit is ended.
pub fn run(exe: &str, exit_gpu: i32, seconds: u64) -> Answer {
    let started = Instant::now();
    let mut child = match Command::new(exe).arg("--probe").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(error) => return Answer { usable: false, reason: format!("the probe did not start: {error}") },
    };
    let mut stdout = child.stdout.take().expect("the probe's output is piped");
    // The output is read on a thread of its own: a pipe that fills up would hold the probe.
    let reader = std::thread::spawn(move || {
        let mut output = String::new();
        let _ = stdout.read_to_string(&mut output);
        output
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
    let output = reader.join().unwrap_or_default();
    answer(&output, code, exit_gpu, seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

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
