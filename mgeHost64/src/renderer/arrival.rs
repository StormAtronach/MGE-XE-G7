//! The ARRIVED of the distant land after a game load or a teleport (W640 part 3; the Architect's
//! design `designs/w634-load-readiness-2026-11-24.md`, section 5). d3d8.dll holds the game's
//! loading screen until ARRIVED. It goes when the load of the data has ended, two world frames of
//! the wait have passed, the distant statics near the camera are settled, and the host has made what
//! it still makes for the frames to come ([`HostReadiness`]: the stage pipelines of its list, and
//! the jobs of the mip chains). One bound holds for all: the limit, counted from the end of the
//! load of the data. The line of the ARRIVED names the time of each condition, or the conditions not
//! met at the limit.

use std::time::{Duration, Instant};

use nirender_wgpu::HostReadiness;

/// The facts of a wait at the end of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Facts {
    /// The load of the data has ended.
    pub loaded: bool,
    /// The world frames of the wait: the first one asks for the statics of the new place.
    pub frames: u32,
    /// The distant statics near the camera are there.
    pub settled: bool,
    /// What the host still makes.
    pub ready: HostReadiness,
}

impl Facts {
    /// The list's pipelines are made.
    pub fn list_made(&self) -> bool {
        self.ready.list_jobs == 0
    }

    /// The mip chains are made.
    pub fn chains_made(&self) -> bool {
        self.ready.chain_jobs == 0
    }
}

/// What a wait does at the end of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Not yet: a condition is not met, and the limit is not reached.
    Wait,
    /// Every condition is met.
    Arrived,
    /// The limit is reached with a condition not met: ARRIVED goes anyway.
    AtTheLimit,
}

/// The decision of a wait, with `waited` since the end of the load of the data and the `limit`.
pub fn decide(facts: &Facts, waited: Duration, limit: Duration) -> Decision {
    if !facts.loaded || facts.frames < 2 {
        return Decision::Wait;
    }
    if facts.settled && facts.list_made() && facts.chains_made() {
        Decision::Arrived
    } else if waited >= limit {
        Decision::AtTheLimit
    } else {
        Decision::Wait
    }
}

/// The times at which the conditions of one wait became true and stayed true.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Times {
    statics: Option<Instant>,
    list: Option<Instant>,
    chains: Option<Instant>,
}

impl Times {
    /// The conditions at `now`: each keeps the time at which it became true; a condition that is
    /// false again loses it.
    pub fn note(&mut self, now: Instant, facts: &Facts) {
        for (time, true_now) in [
            (&mut self.statics, facts.settled),
            (&mut self.list, facts.list_made()),
            (&mut self.chains, facts.chains_made()),
        ] {
            match true_now {
                true => {
                    time.get_or_insert(now);
                }
                false => *time = None,
            }
        }
    }

    /// The line of the ARRIVED at `now` for the wait that counts from `since` (the end of the load
    /// of the data): `serial` is the number of the wait (0 for none), `facts` the facts of the frame.
    /// A time before `since` is 0.
    pub fn line(&self, since: Instant, now: Instant, serial: u32, decision: Decision, facts: &Facts) -> String {
        let seconds = |at: Instant| at.saturating_duration_since(since).as_secs_f32();
        let condition = |name: &str, time: Option<Instant>, left: String| match time {
            Some(time) => format!("{name} at {:.2} s", seconds(time)),
            None => format!("{name}: not met{left}"),
        };
        format!(
            "distant land: ready (arrived) in {:.2} s{}{}; {}, {}, {}",
            seconds(now),
            match serial {
                0 => String::new(),
                serial => format!(", wait {serial}"),
            },
            match decision {
                Decision::AtTheLimit => ", at the limit",
                _ => "",
            },
            condition("the statics", self.statics, String::new()),
            condition("the list", self.list, format!(" ({} pipelines left)", facts.ready.list_jobs)),
            condition("the chains", self.chains, format!(" ({} jobs left)", facts.ready.chain_jobs)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: Duration = Duration::from_secs(10);

    fn facts(settled: bool, list_jobs: usize, chain_jobs: usize) -> Facts {
        Facts {
            loaded: true,
            frames: 2,
            settled,
            ready: HostReadiness { list_jobs, chain_jobs },
        }
    }

    /// Every condition met: ARRIVED.
    #[test]
    fn a_wait_with_every_condition_met_arrives() {
        assert_eq!(decide(&facts(true, 0, 0), Duration::ZERO, LIMIT), Decision::Arrived);
    }

    /// The list's pipelines left: the wait goes on before the limit, and the statics alone are not
    /// enough.
    #[test]
    fn the_list_left_holds_the_wait_before_the_limit() {
        assert_eq!(decide(&facts(true, 37, 0), Duration::from_secs(9), LIMIT), Decision::Wait);
    }

    /// At the limit, ARRIVED goes with the conditions not met, and the line names them.
    #[test]
    fn at_the_limit_the_wait_arrives_and_names_what_is_not_met() {
        let state = facts(true, 37, 0);
        assert_eq!(decide(&state, LIMIT, LIMIT), Decision::AtTheLimit);
        let since = Instant::now();
        let mut times = Times::default();
        times.note(since + Duration::from_millis(5340), &state);
        assert_eq!(
            times.line(since, since + LIMIT, 1, Decision::AtTheLimit, &state),
            "distant land: ready (arrived) in 10.00 s, wait 1, at the limit; the statics at 5.34 s, the list: not met (37 pipelines left), the chains at 5.34 s"
        );
    }

    /// The mip chains left hold the wait as the list does.
    #[test]
    fn the_chains_left_hold_the_wait() {
        assert_eq!(decide(&facts(true, 0, 4), Duration::from_secs(1), LIMIT), Decision::Wait);
    }

    /// With nothing left in the host from the start, the wait is as before W640 part 3: the statics
    /// and the two frames, and the limit for statics not settled. The load of the data and the second
    /// frame come first, as before.
    #[test]
    fn with_nothing_left_in_the_host_the_wait_is_as_before() {
        assert_eq!(decide(&facts(true, 0, 0), Duration::from_secs(3), LIMIT), Decision::Arrived);
        assert_eq!(decide(&facts(false, 0, 0), Duration::from_secs(3), LIMIT), Decision::Wait);
        assert_eq!(decide(&facts(false, 0, 0), LIMIT, LIMIT), Decision::AtTheLimit);
        let one_frame = Facts {
            frames: 1,
            ..facts(true, 0, 0)
        };
        assert_eq!(decide(&one_frame, LIMIT, LIMIT), Decision::Wait);
        let loading = Facts {
            loaded: false,
            ..facts(true, 0, 0)
        };
        assert_eq!(decide(&loading, LIMIT, LIMIT), Decision::Wait);
    }

    /// The line names each condition's time from the end of the load of the data; a condition met
    /// before it is at 0, and one that is false again starts again.
    #[test]
    fn the_line_names_the_time_of_each_condition() {
        let since = Instant::now();
        let at = |ms: u64| since + Duration::from_millis(ms);
        let mut times = Times::default();
        times.note(since - Duration::from_millis(500), &facts(false, 0, 0));
        times.note(at(1000), &facts(true, 0, 0));
        times.note(at(2000), &facts(false, 0, 0));
        times.note(at(5340), &facts(true, 0, 0));
        assert_eq!(
            times.line(since, at(5340), 0, Decision::Arrived, &facts(true, 0, 0)),
            "distant land: ready (arrived) in 5.34 s; the statics at 5.34 s, the list at 0.00 s, the chains at 0.00 s"
        );
    }
}
