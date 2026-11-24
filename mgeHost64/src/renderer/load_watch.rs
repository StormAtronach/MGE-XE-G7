//! The load watch of the distant land (W640 step 1a; the Architect's design
//! `designs/w634-load-readiness-2026-11-24.md`, section 2). With the host's switch
//! `MGE_XE_LOAD_WATCH=1`, the log gets the `load watch:` lines of a wait after a game load or a
//! teleport. The switch is off by default.
//!
//! - `load watch: arrived at frame N`: when [`super::ARRIVED`] goes, the time of each of its
//!   conditions from the start of the wait: the end of the load of the data, the second frame of
//!   the wait, the distant statics near the camera settled, the host's list of stage pipelines
//!   made, and the host's mip chains made (or the timeout; [`super::arrival`]).
//! - `load watch: statics at frame N`: in the window, each part of a frame in which subsets of the
//!   distant statics arrived: their count, the grass subsets among them, their bytes, and the
//!   subsets asked for that did not arrive yet.
//! - `load watch: the lamp grid of <block> at frame N`: in the window, a list and a grid of the
//!   distant lamps that went into the buffers.
//!
//! The window is the wait and [`WINDOW_AFTER`] after its ARRIVED. The frame numbers are those of
//! the host's `load watch: frame N` lines (nirender `host/src/load_watch.rs`): a line of this
//! module comes before the host's line of its frame in the log.

use std::time::{Duration, Instant};

use super::arrival::Facts;

/// The time that the window stays open after an ARRIVED: as the host's window after the last
/// frame with no world.
pub const WINDOW_AFTER: Duration = Duration::from_secs(20);

/// A condition of the arrival: the time and the frame at which it became true and stayed true.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Met {
    at: Instant,
    frame: u64,
}

/// The conditions of one wait, from its start to its ARRIVED.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrivalWatch {
    start: Instant,
    start_frame: u64,
    loaded: Option<Met>,
    two_frames: Option<Met>,
    settled: Option<Met>,
    list: Option<Met>,
    chains: Option<Met>,
}

impl ArrivalWatch {
    /// A wait that starts at `now`, in the frame `frame`.
    pub fn new(now: Instant, frame: u64) -> ArrivalWatch {
        ArrivalWatch {
            start: now,
            start_frame: frame,
            loaded: None,
            two_frames: None,
            settled: None,
            list: None,
            chains: None,
        }
    }

    /// The conditions at the end of a frame of the wait ([`Facts`]). Each condition keeps the
    /// frame at which it became true; a condition that is false again loses it.
    pub fn note(&mut self, now: Instant, frame: u64, facts: &Facts) {
        let met = Met { at: now, frame };
        for (condition, true_now) in [
            (&mut self.loaded, facts.loaded),
            (&mut self.two_frames, facts.frames >= 2),
            (&mut self.settled, facts.settled),
            (&mut self.list, facts.list_made()),
            (&mut self.chains, facts.chains_made()),
        ] {
            match true_now {
                true => {
                    condition.get_or_insert(met);
                }
                false => *condition = None,
            }
        }
    }

    /// The line of the ARRIVED at `now`, in the frame `frame`: `serial` is the number of the wait
    /// that it answers (0 for the wait of a game load with no such number).
    pub fn line(&self, now: Instant, frame: u64, serial: u32) -> String {
        let seconds = |at: Instant| at.saturating_duration_since(self.start).as_secs_f64();
        let condition = |name: &str, met: Option<Met>| match met {
            Some(met) => format!("{name} at {:.2} s (frame {})", seconds(met.at), met.frame),
            None => format!("{name}: no"),
        };
        let wait = match serial {
            0 => "the wait of a game load".to_string(),
            serial => format!("wait {serial}"),
        };
        format!(
            "load watch: arrived at frame {frame}, {wait}, {:.2} s after its start at frame {}; {}, {}, {}, {}, {}",
            seconds(now),
            self.start_frame,
            condition("the data loaded", self.loaded),
            condition("the second frame", self.two_frames),
            condition("the statics settled", self.settled),
            condition("the list made", self.list),
            condition("the chains made", self.chains),
        )
    }
}

/// The arrivals of distant statics in one part of a frame, for the line of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StaticsArrivals {
    pub subsets: u32,
    pub grass: u32,
    pub bytes: u64,
}

impl StaticsArrivals {
    /// The line of the arrivals in the frame `frame`; `asked` is the subsets asked for that did
    /// not arrive yet. None when no subset arrived.
    pub fn line(&self, frame: u64, asked: usize) -> Option<String> {
        (self.subsets != 0).then(|| {
            format!(
                "load watch: statics at frame {frame}: {} subsets arrived ({} of grass), {:.2} MB; {asked} asked for and not arrived",
                self.subsets,
                self.grass,
                self.bytes as f64 / 1e6
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each condition gives the time and the frame at which it became true and stayed true.
    /// The facts of a frame of a wait: `list_jobs` of the host's list left.
    fn facts(loaded: bool, frames: u32, settled: bool, list_jobs: usize) -> Facts {
        Facts {
            loaded,
            frames,
            settled,
            ready: nirender_wgpu::HostReadiness {
                list_jobs,
                chain_jobs: 0,
            },
        }
    }

    #[test]
    fn the_line_names_each_condition_with_its_time_and_frame() {
        let zero = Instant::now();
        let at = |ms: u64| zero + Duration::from_millis(ms);
        let mut watch = ArrivalWatch::new(zero, 10);
        // The statics are settled before the first frame asks for the new place, then not; the
        // host's list is made in the second frame.
        watch.note(at(100), 11, &facts(false, 1, true, 3));
        watch.note(at(200), 12, &facts(true, 2, false, 0));
        watch.note(at(1500), 13, &facts(true, 3, true, 0));
        assert_eq!(
            watch.line(at(1500), 13, 7),
            "load watch: arrived at frame 13, wait 7, 1.50 s after its start at frame 10; the data loaded at 0.20 s (frame 12), the second frame at 0.20 s (frame 12), the statics settled at 1.50 s (frame 13), the list made at 0.20 s (frame 12), the chains made at 0.10 s (frame 11)"
        );
    }

    /// A timeout leaves the statics not settled; a wait with no number is that of a game load.
    #[test]
    fn a_timeout_names_the_condition_not_met() {
        let zero = Instant::now();
        let mut watch = ArrivalWatch::new(zero, 1);
        watch.note(zero, 2, &facts(true, 2, false, 12));
        let line = watch.line(zero + Duration::from_secs(10), 300, 0);
        assert!(
            line.starts_with("load watch: arrived at frame 300, the wait of a game load, 10.00 s after"),
            "{line}"
        );
        assert!(
            line.ends_with(", the statics settled: no, the list made: no, the chains made at 0.00 s (frame 2)"),
            "{line}"
        );
    }

    /// A part of a frame with no arrival gives no line.
    #[test]
    fn the_statics_line_comes_with_an_arrival_only() {
        assert_eq!(StaticsArrivals::default().line(5, 3), None);
        let arrivals = StaticsArrivals {
            subsets: 4,
            grass: 1,
            bytes: 2_500_000,
        };
        assert_eq!(
            arrivals.line(5, 3).as_deref(),
            Some("load watch: statics at frame 5: 4 subsets arrived (1 of grass), 2.50 MB; 3 asked for and not arrived")
        );
    }
}
