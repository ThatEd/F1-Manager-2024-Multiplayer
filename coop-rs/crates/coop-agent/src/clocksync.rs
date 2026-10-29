//! Acting on the other player's pause state.
//!
//! `coordination::ClockGate` already knows the rule - the union of both players' reasons - and it is
//! tested. This module does not restate the rule; it feeds the gate from the right places and turns its
//! answer into something the front-end can act on.
//!
//! ## Why the agent and not the front-end
//!
//! The UI knows its *own* reasons (it sees `SimPauseChange`). It does not know the other player's: those
//! arrive on the supervisor socket, which only the agent owns. Keeping both halves here means the union is
//! computed once, in one place, by the tested code - two independent unions in two languages would drift.
//!
//! ## Why a sequence number rather than a flag
//!
//! The front-end polls. If the endpoint reported only "stopped: true", a paused world would keep being
//! re-paused on every poll, and the pause would never end because nothing distinguishes "still stopped" from
//! "stop again". The sequence advances only when the *answer changes*, and the front-end acts on a new
//! sequence only - the same reasoning as `ClockAction::NoChange`, one layer up.

use coop_proto::coordination::{ClockAction, ClockGate, PauseReason};
use coop_proto::Side;
use std::collections::BTreeSet;

/// Decodes a wire reason list into the gate's set.
///
/// An unknown byte is mapped to [`PauseReason::Resync`] rather than dropped. Dropping it would be the
/// dangerous direction: a peer that stopped the world for a reason this build does not know would read as
/// "not stopped", and the world would run while the other machine believes it is frozen. `Resync` keeps it
/// stopped and is a reason we can name in a log.
pub fn decode_reasons(bytes: &[u8]) -> BTreeSet<PauseReason> {
    bytes
        .iter()
        .map(|b| PauseReason::from_u8(*b).unwrap_or(PauseReason::Resync))
        .collect()
}

/// Encodes a set back to wire bytes, for the status endpoint and the log.
pub fn encode_reasons(set: &BTreeSet<PauseReason>) -> Vec<u8> {
    set.iter().map(|r| r.as_u8()).collect()
}

/// What the front-end should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// Advances only when the answer changes, so the caller can act exactly once per change.
    pub seq: u64,
    /// `"stop"`, `"run"` or `"nochange"`.
    pub action: &'static str,
    /// Whether the world must be held, as of this view.
    pub stopped: bool,
    /// This player's reasons, as bytes.
    pub local: Vec<u8>,
    /// The other player's reasons, as bytes.
    pub remote: Vec<u8>,
}

/// The two-sided clock state for one machine.
#[derive(Debug)]
pub struct ClockSync {
    gate: ClockGate,
    my: Side,
    other: Side,
    seq: u64,
    last: ClockAction,
    local: BTreeSet<PauseReason>,
    remote: BTreeSet<PauseReason>,
}

impl ClockSync {
    pub fn new(my: Side) -> Self {
        ClockSync {
            gate: ClockGate::new(),
            my,
            other: match my {
                Side::Host => Side::Client,
                Side::Client => Side::Host,
            },
            // Starts at 0 with NoChange, so a fresh session publishes nothing to do until something
            // actually pauses. A distinct starting value would make the front-end pause the game on boot.
            seq: 0,
            last: ClockAction::NoChange,
            local: BTreeSet::new(),
            remote: BTreeSet::new(),
        }
    }

    /// Records this player's own pause reasons. Returns the new sequence.
    ///
    /// Called from the front-end via `/coop/coordinate/clock`, which is also what gets sent to the peer -
    /// so this is the same set the other machine sees, not a re-derivation.
    pub fn set_local(&mut self, reasons: &BTreeSet<PauseReason>) -> u64 {
        self.local = reasons.clone();
        let action = self.gate.apply(self.my, reasons.clone());
        self.advance(action)
    }

    /// Records the other player's reasons, as received on the supervisor socket.
    pub fn set_remote(&mut self, reasons: &BTreeSet<PauseReason>) -> u64 {
        self.remote = reasons.clone();
        let action = self.gate.apply(self.other, reasons.clone());
        self.advance(action)
    }

    /// Convenience for the byte-oriented callers.
    pub fn set_local_bytes(&mut self, bytes: &[u8]) -> u64 {
        self.set_local(&decode_reasons(bytes))
    }

    pub fn set_remote_bytes(&mut self, bytes: &[u8]) -> u64 {
        self.set_remote(&decode_reasons(bytes))
    }

    fn advance(&mut self, action: ClockAction) -> u64 {
        self.last = action;
        // Only a real transition is worth a new sequence. `NoChange` is the common case - a repeated
        // "still paused" from a heartbeat - and bumping the sequence for it would make the front-end call
        // into the engine on every poll.
        if action != ClockAction::NoChange {
            self.seq = self.seq.saturating_add(1);
        }
        self.seq
    }

    pub fn view(&self) -> View {
        View {
            seq: self.seq,
            action: match self.last {
                ClockAction::Stop => "stop",
                ClockAction::Run => "run",
                ClockAction::NoChange => "nochange",
            },
            stopped: self.gate.is_stopped(),
            local: encode_reasons(&self.local),
            remote: encode_reasons(&self.remote),
        }
    }

    /// The union of every reason holding the world, for a log line.
    pub fn reasons(&self) -> Vec<u8> {
        encode_reasons(&self.gate.reasons())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[PauseReason]) -> BTreeSet<PauseReason> {
        names.iter().copied().collect()
    }

    #[test]
    fn a_fresh_session_asks_for_nothing() {
        let c = ClockSync::new(Side::Host);
        let v = c.view();
        assert_eq!(v.action, "nochange");
        assert!(!v.stopped);
        assert_eq!(v.seq, 0, "booting must not ask the front-end to pause the game");
    }

    #[test]
    fn the_local_player_pausing_stops_the_world_once() {
        let mut c = ClockSync::new(Side::Host);
        let s1 = c.set_local(&set(&[PauseReason::UserPause]));
        assert_eq!(s1, 1);
        assert_eq!(c.view().action, "stop");
        assert!(c.view().stopped);

        // The same set again is not a new instruction. Without this the front-end would cross into the
        // engine on every poll, resetting the user's playback rate (see ClockAction::NoChange).
        let s2 = c.set_local(&set(&[PauseReason::UserPause]));
        assert_eq!(s2, 1, "a repeated report must not advance the sequence");
        assert_eq!(c.view().action, "nochange");
    }

    #[test]
    fn the_other_players_pause_holds_this_machines_world() {
        let mut c = ClockSync::new(Side::Host);
        c.set_remote(&set(&[PauseReason::Notifications]));
        let v = c.view();
        assert_eq!(v.action, "stop", "the requirement: their clock stopping stops ours");
        assert!(v.stopped);
        assert_eq!(v.remote, vec![2]);
        assert!(v.local.is_empty());
    }

    #[test]
    fn releasing_one_side_does_not_resume_while_the_other_still_holds() {
        // The union rule, exercised through this module rather than only through ClockGate.
        let mut c = ClockSync::new(Side::Client);
        c.set_remote(&set(&[PauseReason::UserPause]));
        c.set_local(&set(&[PauseReason::Tutorial]));
        assert!(c.view().stopped);

        c.set_remote(&BTreeSet::new());
        assert!(c.view().stopped, "the local tutorial still holds the world");
        assert_eq!(c.view().action, "nochange", "nothing changed for the front-end to do");

        c.set_local(&BTreeSet::new());
        assert!(!c.view().stopped);
        assert_eq!(c.view().action, "run", "both released: the world runs again");
    }

    #[test]
    fn the_sequence_advances_once_per_transition() {
        let mut c = ClockSync::new(Side::Host);
        c.set_remote(&set(&[PauseReason::UserPause]));
        c.set_local(&set(&[PauseReason::UserPause]));
        assert_eq!(c.view().seq, 1, "the second hold does not change the answer");
        c.set_remote(&BTreeSet::new());
        c.set_local(&BTreeSet::new());
        assert_eq!(c.view().seq, 2, "the release is one transition");
    }

    #[test]
    fn each_side_feeds_the_gate_under_its_own_identity() {
        // The two machines must agree on which set is whose. Swapping them would still "work" in a
        // symmetric test and break the moment one side says something the other does not.
        for my in [Side::Host, Side::Client] {
            let mut c = ClockSync::new(my);
            c.set_local(&set(&[PauseReason::UserPause]));
            assert_eq!(c.view().local, vec![0], "local must be the {my:?}'s own reasons");
            assert!(c.view().remote.is_empty());
        }
    }

    #[test]
    fn an_unknown_reason_holds_the_world_rather_than_being_dropped() {
        // Dropping it would read as "not stopped" and run the world while the peer believes it is frozen.
        let mut c = ClockSync::new(Side::Host);
        c.set_remote_bytes(&[200]);
        assert!(c.view().stopped);
        assert_eq!(c.reasons(), vec![PauseReason::Resync.as_u8()]);
    }

    #[test]
    fn reasons_round_trip_through_the_wire_form() {
        let input = [0u8, 3, 4];
        let decoded = decode_reasons(&input);
        assert_eq!(encode_reasons(&decoded), Vec::<u8>::from(input));
    }

    #[test]
    fn an_empty_remote_set_clears_only_the_remote_side() {
        let mut c = ClockSync::new(Side::Host);
        c.set_local(&set(&[PauseReason::Notifications]));
        c.set_remote_bytes(&[]);
        assert!(c.view().stopped, "the local notification still holds");
        assert!(c.view().remote.is_empty());
    }
}
