//! Holding the race start until both players are actually present.
//!
//! The requirement, in the user's words: the lights must hold until the second player is ready, and the
//! host's cars must be held too. So this is not a client-side freeze - both sims are pinned to one
//! captured grid until a release condition is met.
//!
//! ## Why the host cannot simply "wait"
//!
//! There is no clock setter in this sim (`SetSimTime`, `SetRaceTime`, `SetCurrentLap` all return 0
//! occurrences in the binary - RACESIM-HIJACK §5.1). A host cannot pause and then resume the simulation;
//! it can only *overwrite* per-car state. So a hold is not a pause: it is the repeated application of one
//! fixed snapshot, which makes the simulation's own progress irrelevant because every frame restores the
//! same place.
//!
//! That distinction matters for correctness. A hold written as "stop sending updates" would let the sims
//! drift, and drift during a hold is invisible until the release, when the two worlds differ by however
//! long the hold lasted. Re-applying a capture is a fixed point: 5 seconds of hold and 50 seconds of hold
//! produce the same grid.
//!
//! ## When it engages
//!
//! `flags & WAITING_FOR_LIGHTS_OUT` - bit 8, `0x100` (see `coop-proto::layout`). This is the sim's own
//! pre-start state, so the hold needs no cooperation from the UI and no knowledge of when the lights
//! sequence begins.
//!
//! ## When it releases
//!
//! Three independent ways, because a hold that cannot end is worse than no hold:
//!
//! 1. the peer reports ready with evidence that it is in a session;
//! 2. the sim leaves the pre-start state on its own (the host's lights ran out) - the hold follows the
//!    sim rather than fighting it;
//! 3. a timeout. A peer that never reports ready must not wedge the race forever.
//!
//! Release is deliberately *not* "both sides pressed go". The host's sim is the authority for the lights;
//! this module only keeps the grid still until the other player can see it.

use coop_proto::{Car, RcState};

/// `RcState::flags` bit 8: the sim is waiting for the lights to go out.
pub const WAITING_FOR_LIGHTS_OUT: u32 = 0x100;

/// How long a hold may last before it releases anyway.
///
/// Long enough for a client that is loading a save or a level (observed at 20-40 s), short enough that a
/// client which never arrives does not cost the whole race. On expiry the hold releases and logs; it does
/// not fail, because a released hold is simply an unsynchronised start, which is what the project had
/// before this existed.
pub const HOLD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Why a hold released, for the log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// The peer said it was ready, with evidence.
    PeerReady,
    /// The sim left the lights-out state on its own.
    SimStarted,
    /// Nothing arrived in time.
    Timeout,
    /// The peer's ready arrived but did not prove it was in a session.
    PeerNotUsable,
}

impl Release {
    pub fn as_str(self) -> &'static str {
        match self {
            Release::PeerReady => "peer ready",
            Release::SimStarted => "sim left the pre-start state",
            Release::Timeout => "hold timed out",
            Release::PeerNotUsable => "peer ready was not usable",
        }
    }
}

/// State of one race start.
#[derive(Debug)]
pub struct StartHold {
    /// The captured grid, once the hold has engaged.
    grid: Option<coop_proto::GridHold>,
    /// When the hold engaged.
    since: Option<std::time::Instant>,
    /// Released, with the reason. `None` means still holding (or never engaged).
    released: Option<Release>,
    /// Whether a usable ready has been seen from the peer.
    peer_ready: bool,
    /// Ticks spent holding, so the log can say how long and how much was re-applied.
    applied: u64,
}

impl Default for StartHold {
    fn default() -> Self {
        Self::new()
    }
}

impl StartHold {
    pub fn new() -> Self {
        StartHold {
            grid: None,
            since: None,
            released: None,
            peer_ready: false,
            applied: 0,
        }
    }

    /// True when the sim is in the pre-start state this hold is for.
    pub fn is_pre_start(rc: &RcState) -> bool {
        rc.flags & WAITING_FOR_LIGHTS_OUT != 0
    }

    /// A ready from the peer. Returns true when it was usable.
    ///
    /// Usable means the peer claims a session and can see cars moving. A client that reports ready from a
    /// menu is the exact failure this check exists to prevent: holding the race for someone who is not
    /// there is indistinguishable, from the host's side, from holding it for someone who is.
    pub fn on_peer_ready(&mut self, session: u8, moving_cars: u8) -> bool {
        let usable = session != 0 && moving_cars > 0;
        self.peer_ready = usable;
        usable
    }

    /// Feeds one frame of sim state.
    ///
    /// Returns the cars to write, when the hold is active and the sim should be pinned to the captured
    /// grid; `None` means "let the sim do what it wants".
    ///
    /// Taking `engine_cars` every tick rather than only at capture is what makes the hold robust: the
    /// capture has to happen the first time the pre-start state is seen, which is the only moment the cars
    /// are reliably on their grid marks.
    pub fn step(
        &mut self,
        rc: &RcState,
        engine_cars: &[Car],
        race_time: f32,
        now: std::time::Instant,
    ) -> Option<&[Car]> {
        if self.released.is_some() {
            return None;
        }

        if !Self::is_pre_start(rc) {
            // The sim started on its own. Follow it rather than holding a race that is already running.
            if self.grid.is_some() {
                self.released = Some(Release::SimStarted);
            }
            return None;
        }

        // Pre-start: capture once, then keep re-applying that same capture.
        if self.grid.is_none() {
            let candidate = coop_proto::GridHold {
                cars: engine_cars.to_vec(),
                race_time,
            };
            if !candidate.is_usable() {
                // Nothing on the grid yet - an object that has not initialised. Do not capture, because a
                // hold built from zeros would plant every car on the start line.
                return None;
            }
            self.grid = Some(candidate);
            self.since = Some(now);
        }

        // Release conditions, checked in order of trustworthiness.
        if self.peer_ready {
            self.released = Some(Release::PeerReady);
            return None;
        }
        if let Some(since) = self.since {
            if now.duration_since(since) >= HOLD_TIMEOUT {
                self.released = Some(Release::Timeout);
                return None;
            }
        }

        self.applied += 1;
        self.grid.as_ref().map(|g| g.cars.as_slice())
    }

    /// The captured grid, if any.
    pub fn grid(&self) -> Option<&coop_proto::GridHold> {
        self.grid.as_ref()
    }

    /// `(holding, released, ticks applied)` for the log.
    pub fn status(&self) -> (bool, Option<Release>, u64) {
        (self.grid.is_some() && self.released.is_none(), self.released, self.applied)
    }

    /// Resets for a new session, so a second race is held as well.
    pub fn reset(&mut self) {
        *self = StartHold::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn grid_cars() -> Vec<Car> {
        (0..22u8)
            .map(|id| Car {
                id,
                dist: 100.0 + id as f32,
                lat: 0.0,
                head: 0.0,
                spd: 0.0,
                ..Default::default()
            })
            .collect()
    }

    fn pre_start() -> RcState {
        RcState { flags: WAITING_FOR_LIGHTS_OUT, rc_state: 1, rf_state: 0 }
    }

    fn racing() -> RcState {
        RcState { flags: 0x400, rc_state: 1, rf_state: 0 }
    }

    #[test]
    fn nothing_is_held_before_the_pre_start_state() {
        let mut hold = StartHold::new();
        let now = Instant::now();
        assert!(hold.step(&racing(), &grid_cars(), 1.0, now).is_none());
        assert!(!hold.status().0);
    }

    /// The core property: while holding, every frame is pinned to the same capture, so a long hold cannot
    /// walk the cars off their marks.
    #[test]
    fn a_hold_reapplies_one_fixed_capture() {
        let mut hold = StartHold::new();
        let now = Instant::now();
        let grid = grid_cars();

        let first = hold.step(&pre_start(), &grid, 1.0, now).unwrap().to_vec();
        // The sim "moves" while held; the hold must ignore that.
        let moved: Vec<Car> = grid
            .iter()
            .map(|c| Car { dist: c.dist + 500.0, ..*c })
            .collect();
        let second = hold
            .step(&pre_start(), &moved, 5.0, now + Duration::from_secs(4))
            .unwrap()
            .to_vec();

        assert_eq!(first, second, "the hold must restore the same grid every frame");
        assert_eq!(first, grid, "and that grid is the one captured at the start");
    }

    #[test]
    fn a_hold_releases_when_the_peer_reports_ready() {
        let mut hold = StartHold::new();
        let now = Instant::now();
        assert!(hold.step(&pre_start(), &grid_cars(), 1.0, now).is_some());
        assert!(hold.status().0, "should be holding");

        assert!(hold.on_peer_ready(1, 22), "a session with cars is usable");
        assert!(
            hold.step(&pre_start(), &grid_cars(), 2.0, now + Duration::from_secs(1)).is_none(),
            "a ready peer releases the hold"
        );
        assert_eq!(hold.status().1, Some(Release::PeerReady));
    }

    /// A ready from a client sitting in a menu must not release the hold - that is the failure the whole
    /// module exists to prevent.
    #[test]
    fn an_unusable_ready_does_not_release_the_hold() {
        let mut hold = StartHold::new();
        let now = Instant::now();
        hold.step(&pre_start(), &grid_cars(), 1.0, now);

        assert!(!hold.on_peer_ready(0, 0), "no session, no moving cars");
        assert!(
            hold.step(&pre_start(), &grid_cars(), 2.0, now + Duration::from_secs(1)).is_some(),
            "still holding"
        );
        assert!(!hold.on_peer_ready(1, 0), "a session but no moving cars is not proof of anything");
        assert!(hold.step(&pre_start(), &grid_cars(), 3.0, now + Duration::from_secs(2)).is_some());
    }

    #[test]
    fn a_hold_releases_when_the_sim_starts_anyway() {
        let mut hold = StartHold::new();
        let now = Instant::now();
        hold.step(&pre_start(), &grid_cars(), 1.0, now);
        // The lights ran out while we were still waiting: follow the sim, do not fight it.
        assert!(hold.step(&racing(), &grid_cars(), 30.0, now + Duration::from_secs(29)).is_none());
        assert_eq!(hold.status().1, Some(Release::SimStarted));
    }

    #[test]
    fn a_hold_times_out_rather_than_wedging_the_race() {
        let mut hold = StartHold::new();
        let now = Instant::now();
        hold.step(&pre_start(), &grid_cars(), 1.0, now);
        let late = now + HOLD_TIMEOUT + Duration::from_secs(1);
        assert!(hold.step(&pre_start(), &grid_cars(), 1.0, late).is_none());
        assert_eq!(hold.status().1, Some(Release::Timeout));
    }

    /// A capture of all-zero distances is not a grid, and holding it would plant every car on the line.
    #[test]
    fn an_uninitialised_object_is_never_captured() {
        let mut hold = StartHold::new();
        let zeros: Vec<Car> = (0..22u8).map(|id| Car { id, ..Default::default() }).collect();
        let now = Instant::now();
        assert!(
            hold.step(&pre_start(), &zeros, 0.0, now).is_none(),
            "must not capture zeros"
        );
        assert!(hold.grid().is_none());
        // and once real data arrives, it captures normally
        assert!(hold.step(&pre_start(), &grid_cars(), 1.0, now).is_some());
    }

    #[test]
    fn reset_lets_a_second_race_be_held() {
        let mut hold = StartHold::new();
        let now = Instant::now();
        hold.step(&pre_start(), &grid_cars(), 1.0, now);
        hold.on_peer_ready(1, 22);
        hold.step(&pre_start(), &grid_cars(), 2.0, now);
        assert!(hold.status().1.is_some());

        hold.reset();
        assert!(hold.status().1.is_none());
        assert!(hold.grid().is_none());
        assert!(hold.step(&pre_start(), &grid_cars(), 1.0, now).is_some(), "holds again");
    }
}
