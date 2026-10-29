//! Cross-player coordination: the rules that make two copies of the game behave like one session.
//!
//! Four requirements drive this module, each a distinct rule:
//!
//! 1. **A stopped clock stops both.** If one player pauses, the other must pause too, and the merge
//!    that follows is authoritative per team: whatever the AI did on a player's team while the clocks
//!    disagreed is discarded, not merged.
//! 2. **Team authority beats local drift.** Each team has exactly one owner. Any row for a team that is
//!    not yours is replaced by the owner's value, regardless of what the local sim computed.
//! 3. **One instant contract per person per day.** Two players must not sign the same person on the same
//!    day; the first accepted offer wins and the second is refused with a reason.
//! 4. **Both votes count.** A regulation vote is world-level state, so it cannot simply be "host wins":
//!    every player's vote is recorded and the result computed from all of them.
//!
//! Everything here is pure: no I/O, no game, no wall clock. These are the rules that are hardest to get
//! right and easiest to get wrong silently, so they are testable in isolation. The agent and the link
//! call into this rather than re-implementing it.
//!
//! Two design choices worth stating:
//!
//! * Time is an integer day/tick passed in, not read from a clock. The machines do not share a clock, and
//!   a rule that depends on wall time is not reproducible.
//! * "Authoritative" belongs to the *team*, not to the message. A host relaying a client's row for a team
//!   the client does not own is still rejected, so a confused client cannot award itself an advantage by
//!   sending through the host.

use std::collections::{BTreeMap, BTreeSet};

/// Which side of the session a peer is.
///
/// Re-exported from the wire module rather than declared here: two `Side` enums would compile but not
/// interoperate, and a caller converting between them by hand is how "host" quietly becomes "client" in
/// an ownership check.
pub use crate::Side;

/// A team identifier, as the game uses it (`Teams.<id>`).
pub type TeamId = u8;

/// A person's identifier, as the game uses it (`Staff.<id>`).
pub type PersonId = i32;

/// A calendar day, monotonic within a season.
pub type Day = i32;

// ---------------------------------------------------------------------------------------------
// 1. Clocks
// ---------------------------------------------------------------------------------------------

/// Why a clock is stopped.
///
/// Kept distinct because unpausing has to be safe: the sim stays stopped while *any* reason is active.
/// Collapsing these into one bool is how a player gets unpaused out of a menu because the other player
/// pressed play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PauseReason {
    UserPause,
    UserTimeControl,
    Notifications,
    Tutorial,
    Resync,
}

impl PauseReason {
    pub fn as_u8(self) -> u8 {
        match self {
            PauseReason::UserPause => 0,
            PauseReason::UserTimeControl => 1,
            PauseReason::Notifications => 2,
            PauseReason::Tutorial => 3,
            PauseReason::Resync => 4,
        }
    }

    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => PauseReason::UserPause,
            1 => PauseReason::UserTimeControl,
            2 => PauseReason::Notifications,
            3 => PauseReason::Tutorial,
            4 => PauseReason::Resync,
            _ => return None,
        })
    }
}

/// What the local sim should do after a pause update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockAction {
    Stop,
    Run,
    /// No change. The common case, and it must not generate work: touching `SimPauseChange`
    /// unnecessarily crosses into the engine and resets the user's playback rate.
    NoChange,
}

/// The union of every peer's pause reasons.
///
/// "When one player's clock stops the other's does too" is a set union, not a flag: two players can stop
/// for different reasons, and whichever clears first must not start the world.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClockGate {
    host: BTreeSet<PauseReason>,
    client: BTreeSet<PauseReason>,
}

impl ClockGate {
    pub fn new() -> Self { Self::default() }

    /// Replace one side's reasons wholesale.
    ///
    /// A peer sends its full current set rather than deltas. A lost delta is far worse than a redundant
    /// message: a lost "unpause" leaves the world stopped with no way to notice why.
    pub fn set(&mut self, side: Side, reasons: BTreeSet<PauseReason>) {
        match side {
            Side::Host => self.host = reasons,
            Side::Client => self.client = reasons,
        }
    }

    /// True when the world must not advance.
    pub fn is_stopped(&self) -> bool {
        !self.host.is_empty() || !self.client.is_empty()
    }

    /// All reasons currently holding the world, for diagnostics.
    pub fn reasons(&self) -> BTreeSet<PauseReason> {
        self.host.union(&self.client).copied().collect()
    }

    /// Apply a peer's reasons and say what the local sim should do.
    pub fn apply(&mut self, side: Side, reasons: BTreeSet<PauseReason>) -> ClockAction {
        let was = self.is_stopped();
        self.set(side, reasons);
        let now = self.is_stopped();
        match (was, now) {
            (false, true) => ClockAction::Stop,
            (true, false) => ClockAction::Run,
            _ => ClockAction::NoChange,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 2. Team authority
// ---------------------------------------------------------------------------------------------

/// Who owns which team.
///
/// Every team is owned by exactly one player or by nobody (AI). Unowned teams are simulated locally by
/// each peer and never merged - the AI is allowed to differ, the same way weather noise is.
#[derive(Debug, Clone, Default)]
pub struct TeamOwners {
    owner: BTreeMap<TeamId, Side>,
}

impl TeamOwners {
    pub fn new() -> Self { Self::default() }

    /// Assign a team. One team, one owner.
    ///
    /// Only the host may create or change an assignment; a client asking for a team it already owns is a
    /// harmless no-op (it re-joined), while a client asking for someone else's team is refused outright -
    /// a client reassigning teams would be a privilege escalation.
    pub fn assign(&mut self, team: TeamId, side: Side, by: Side) -> bool {
        let existing = self.owner.get(&team).copied();
        match (by, existing) {
            // The host decides.
            (Side::Host, _) => { self.owner.insert(team, side); true }
            // A client may confirm its own team, and is refused anything else.
            (Side::Client, Some(current)) => current == side,
            (Side::Client, None) => false,
        }
    }

    pub fn owner_of(&self, team: TeamId) -> Option<Side> {
        self.owner.get(&team).copied()
    }

    pub fn owns(&self, team: TeamId, side: Side) -> bool {
        self.owner.get(&team).copied() == Some(side)
    }

    /// Teams owned by a side, for diagnostics and for the AI-disable pass.
    pub fn teams_of(&self, side: Side) -> Vec<TeamId> {
        self.owner.iter().filter(|(_, s)| **s == side).map(|(t, _)| *t).collect()
    }
}

/// One team's row value in a merge, with the tick that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamRow {
    pub team: TeamId,
    /// Opaque payload, compared only for equality.
    pub value: Vec<u8>,
    /// The sim tick this value was produced at. Recorded for diagnostics; it deliberately does **not**
    /// decide the merge - ownership does.
    pub tick: u32,
}

/// Result of merging one team's rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The value to keep, and whether it differs from what was local.
    Keep { value: Vec<u8>, changed: bool },
    /// No owner for this team, so the local value stands (AI is allowed to diverge).
    NoOwner,
}

/// Merge one team's row from both peers, keeping only the owner's value.
///
/// The rule behind "the teams being authoritative so even if the AI somehow ran things on the other
/// player's team, we change that": the peer that does not own the team has its value thrown away even if
/// that value is newer. Arrival order and tick age deliberately do not decide, because the non-owner's
/// value is exactly the thing that must not survive.
pub fn merge_team_row(
    owners: &TeamOwners,
    team: TeamId,
    local: Option<&TeamRow>,
    remote: Option<&TeamRow>,
    local_side: Side,
) -> MergeOutcome {
    let owner = match owners.owner_of(team) {
        Some(o) => o,
        None => return MergeOutcome::NoOwner,
    };

    let authoritative = if owner == local_side { local } else { remote };
    let other = if owner == local_side { remote } else { local };

    match authoritative {
        Some(a) => {
            let changed = match other {
                Some(o) => o.value != a.value,
                None => true,
            };
            MergeOutcome::Keep { value: a.value.clone(), changed }
        }
        // The non-owner's value must never be promoted, so an absent authoritative value yields nothing
        // rather than quietly falling back to the wrong side.
        None => match other {
            Some(_) => MergeOutcome::Keep { value: Vec::new(), changed: true },
            None => MergeOutcome::Keep { value: Vec::new(), changed: false },
        },
    }
}

/// Merge a whole field of team rows, returning the teams whose value changed and must be written.
pub fn merge_field(
    owners: &TeamOwners,
    local_side: Side,
    local: &BTreeMap<TeamId, TeamRow>,
    remote: &BTreeMap<TeamId, TeamRow>,
) -> BTreeMap<TeamId, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut teams: BTreeSet<TeamId> = local.keys().copied().collect();
    teams.extend(remote.keys().copied());
    for team in teams {
        if let MergeOutcome::Keep { value, changed } =
            merge_team_row(owners, team, local.get(&team), remote.get(&team), local_side)
        {
            if changed {
                out.insert(team, value);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 3. Instant contracts
// ---------------------------------------------------------------------------------------------

/// Why a contract claim was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimDenied {
    AlreadySignedToday,
    ClaimedByOther,
    DuplicateFromSamePlayer,
}

impl ClaimDenied {
    /// What the player is told. Written out because a bare "refused" is indistinguishable from a bug,
    /// and this is the rule players will hit by accident.
    pub fn message(self) -> &'static str {
        match self {
            ClaimDenied::AlreadySignedToday => "that person was already signed today",
            ClaimDenied::ClaimedByOther => "the other player is negotiating with that person",
            ClaimDenied::DuplicateFromSamePlayer => "your offer is already in progress",
        }
    }
}

/// A claim on a person, held while an offer is being negotiated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub person: PersonId,
    pub by: Side,
    pub team: TeamId,
    pub day: Day,
}

/// Who signed whom and when.
///
/// Claims and signings are separate on purpose: a claim expires (the player cancels), but a signing does
/// not, and the rule that matters - "not the same person twice on the same day" - has to survive the
/// claim being released.
#[derive(Debug, Clone, Default)]
pub struct ContractLedger {
    claims: BTreeMap<PersonId, Claim>,
    signed: BTreeMap<PersonId, Day>,
    history: Vec<Claim>,
}

impl ContractLedger {
    pub fn new() -> Self { Self::default() }

    /// Take a claim before an offer is sent.
    pub fn claim(&mut self, person: PersonId, by: Side, team: TeamId, day: Day) -> Result<(), ClaimDenied> {
        if let Some(d) = self.signed.get(&person) {
            if *d == day {
                return Err(ClaimDenied::AlreadySignedToday);
            }
        }
        match self.claims.get(&person) {
            Some(c) if c.by != by => return Err(ClaimDenied::ClaimedByOther),
            Some(c) if c.by == by => return Err(ClaimDenied::DuplicateFromSamePlayer),
            _ => {}
        }
        self.claims.insert(person, Claim { person, by, team, day });
        Ok(())
    }

    /// Release a claim, e.g. the player cancelled the negotiation.
    pub fn release(&mut self, person: PersonId, by: Side) {
        if let Some(c) = self.claims.get(&person) {
            if c.by == by {
                self.claims.remove(&person);
            }
        }
    }

    /// Confirm a signing. Consumes the claim and records the day.
    ///
    /// Re-checks the day rule, because the claim may have been granted before the day changed and a
    /// negotiation can span a day boundary. Checking only at claim time is the bug this prevents.
    pub fn confirm(&mut self, person: PersonId, by: Side, team: TeamId, day: Day) -> Result<(), ClaimDenied> {
        if let Some(c) = self.claims.get(&person) {
            if c.by != by {
                return Err(ClaimDenied::ClaimedByOther);
            }
        }
        if let Some(d) = self.signed.get(&person) {
            if *d == day {
                return Err(ClaimDenied::AlreadySignedToday);
            }
        }
        self.claims.remove(&person);
        self.signed.insert(person, day);
        self.history.push(Claim { person, by, team, day });
        Ok(())
    }

    /// Adopt a signing that happened on the other peer, so both ledgers agree.
    ///
    /// Returns false on a genuine same-day conflict, which means the peers disagree and the host has to
    /// arbitrate. Reported rather than silently overwritten, because one player has already been told
    /// they signed someone.
    pub fn observe_remote_signing(&mut self, person: PersonId, by: Side, team: TeamId, day: Day) -> bool {
        if let Some(d) = self.signed.get(&person) {
            if *d == day {
                return false;
            }
        }
        self.claims.remove(&person);
        self.signed.insert(person, day);
        self.history.push(Claim { person, by, team, day });
        true
    }

    pub fn signed_on(&self, person: PersonId) -> Option<Day> {
        self.signed.get(&person).copied()
    }

    pub fn claimed_by(&self, person: PersonId) -> Option<Side> {
        self.claims.get(&person).map(|c| c.by)
    }

    pub fn history(&self) -> &[Claim] {
        &self.history
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Regulation votes
// ---------------------------------------------------------------------------------------------

/// One player's vote on one proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vote {
    pub side: Side,
    pub proposal: u16,
    /// The option chosen. Interpretation belongs to the game; this module only counts.
    pub option: u8,
}

/// How a vote round resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteResult {
    pub proposal: u16,
    pub tally: BTreeMap<u8, u32>,
    /// The option with the most votes, or None on a tie.
    pub winner: Option<u8>,
    /// True when two options tied for highest. Surfaced rather than silently picking one: "both votes
    /// counted" and "one vote was discarded to break a tie" are different outcomes, and players notice.
    pub tied: bool,
    pub cast: usize,
    pub expected: usize,
}

impl VoteResult {
    /// True when everyone voted.
    ///
    /// The game must not resolve a proposal before this, or the second player's vote is discarded by the
    /// act of resolving.
    pub fn complete(&self) -> bool {
        self.cast >= self.expected
    }
}

/// Collects votes for regulation proposals.
///
/// Tolerant of arrival order and re-votes: a player changing their mind replaces their earlier vote
/// rather than adding a second. That is the difference between "both votes counted" and "the player who
/// clicked most counted most".
#[derive(Debug, Clone, Default)]
pub struct VoteBox {
    votes: BTreeMap<u16, BTreeMap<Side, u8>>,
    expected: usize,
}

impl VoteBox {
    pub fn new(expected: usize) -> Self {
        Self { votes: BTreeMap::new(), expected: expected.max(1) }
    }

    /// Record or replace a vote. Returns the previous option if this replaced one.
    pub fn cast(&mut self, v: Vote) -> Option<u8> {
        self.votes.entry(v.proposal).or_default().insert(v.side, v.option)
    }

    pub fn cast_count(&self, proposal: u16) -> usize {
        self.votes.get(&proposal).map(|m| m.len()).unwrap_or(0)
    }

    /// Result for a proposal. Safe before everyone has voted; check `complete()`.
    pub fn result(&self, proposal: u16) -> VoteResult {
        let mut tally: BTreeMap<u8, u32> = BTreeMap::new();
        let mut cast = 0usize;
        if let Some(per) = self.votes.get(&proposal) {
            for option in per.values() {
                *tally.entry(*option).or_insert(0) += 1;
                cast += 1;
            }
        }
        let mut winner = None;
        let mut tied = false;
        if let Some(best) = tally.values().copied().max() {
            let leaders: Vec<u8> = tally.iter().filter(|(_, c)| **c == best).map(|(o, _)| *o).collect();
            if leaders.len() == 1 {
                winner = Some(leaders[0]);
            } else {
                tied = true;
            }
        }
        VoteResult { proposal, tally, winner, tied, cast, expected: self.expected }
    }

    /// Forget a proposal once resolved.
    pub fn clear(&mut self, proposal: u16) {
        self.votes.remove(&proposal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reasons(list: &[PauseReason]) -> BTreeSet<PauseReason> {
        list.iter().copied().collect()
    }

    fn row(team: TeamId, value: &str, tick: u32) -> TeamRow {
        TeamRow { team, value: value.as_bytes().to_vec(), tick }
    }

    // ---- 1. clocks -------------------------------------------------------------------------

    #[test]
    fn one_players_pause_stops_both() {
        let mut gate = ClockGate::new();
        assert!(!gate.is_stopped());
        assert_eq!(gate.apply(Side::Client, reasons(&[PauseReason::UserPause])), ClockAction::Stop);
        assert!(gate.is_stopped(), "the host must stop when the client pauses");
    }

    #[test]
    fn the_world_stays_stopped_while_any_reason_remains() {
        // The failure this prevents: host pauses, client unpauses, and the world runs while the host is
        // still in a menu. Union of reasons, not last-writer-wins.
        let mut gate = ClockGate::new();
        gate.apply(Side::Host, reasons(&[PauseReason::UserPause]));
        gate.apply(Side::Client, reasons(&[PauseReason::Notifications]));
        assert_eq!(gate.apply(Side::Client, reasons(&[])), ClockAction::NoChange);
        assert!(gate.is_stopped(), "host is still paused, so the world must not run");
        assert_eq!(gate.apply(Side::Host, reasons(&[])), ClockAction::Run);
        assert!(!gate.is_stopped());
    }

    #[test]
    fn unpausing_only_acts_when_the_world_actually_moves() {
        // Touching SimPauseChange needlessly resets the user's playback rate, so NoChange matters.
        let mut gate = ClockGate::new();
        assert_eq!(gate.apply(Side::Host, reasons(&[])), ClockAction::NoChange);
        gate.apply(Side::Host, reasons(&[PauseReason::UserPause]));
        assert_eq!(gate.apply(Side::Host, reasons(&[PauseReason::UserPause])), ClockAction::NoChange);
        assert_eq!(gate.apply(Side::Host, reasons(&[])), ClockAction::Run);
    }

    #[test]
    fn resync_holds_the_world_even_with_no_player_pause() {
        let mut gate = ClockGate::new();
        assert_eq!(gate.apply(Side::Host, reasons(&[PauseReason::Resync])), ClockAction::Stop);
        assert!(gate.is_stopped());
    }

    #[test]
    fn reasons_are_reported_so_a_freeze_can_be_explained() {
        let mut gate = ClockGate::new();
        gate.apply(Side::Host, reasons(&[PauseReason::UserPause]));
        gate.apply(Side::Client, reasons(&[PauseReason::Tutorial]));
        let r = gate.reasons();
        assert!(r.contains(&PauseReason::UserPause));
        assert!(r.contains(&PauseReason::Tutorial));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn pause_reason_round_trips() {
        for r in [PauseReason::UserPause, PauseReason::UserTimeControl, PauseReason::Notifications,
                  PauseReason::Tutorial, PauseReason::Resync] {
            assert_eq!(PauseReason::from_u8(r.as_u8()), Some(r));
        }
        assert_eq!(PauseReason::from_u8(200), None);
    }

    // ---- 2. team authority -----------------------------------------------------------------

    #[test]
    fn owners_value_wins_even_when_it_is_older() {
        // The core rule: the AI may have advanced the other player's team further, and that value is
        // still thrown away. Tick age must not decide.
        let mut owners = TeamOwners::new();
        owners.assign(3, Side::Client, Side::Host);
        let local = row(3, "host-ai-ran-this", 999);
        let remote = row(3, "client-authoritative", 1);
        match merge_team_row(&owners, 3, Some(&local), Some(&remote), Side::Host) {
            MergeOutcome::Keep { value, changed } => {
                assert_eq!(value, b"client-authoritative".to_vec());
                assert!(changed, "the host held a different value, so it must be written");
            }
            other => panic!("expected Keep, got {other:?}"),
        }
    }

    #[test]
    fn non_owners_value_is_never_promoted_when_the_owner_has_not_sent_one() {
        let mut owners = TeamOwners::new();
        owners.assign(3, Side::Client, Side::Host);
        let local = row(3, "host-value", 5);
        assert_eq!(merge_team_row(&owners, 3, Some(&local), None, Side::Host),
                   MergeOutcome::Keep { value: Vec::new(), changed: true },
                   "must not hand back the non-owner's value");
    }

    #[test]
    fn unowned_teams_keep_the_local_simulation() {
        let owners = TeamOwners::new(); // nothing owned: pure AI
        let local = row(7, "ai-local", 3);
        assert_eq!(merge_team_row(&owners, 7, Some(&local), None, Side::Host), MergeOutcome::NoOwner);
    }

    #[test]
    fn owner_side_keeps_its_own_value_without_reporting_a_change() {
        let mut owners = TeamOwners::new();
        owners.assign(3, Side::Host, Side::Host);
        let local = row(3, "same", 1);
        let remote = row(3, "same", 1);
        assert_eq!(merge_team_row(&owners, 3, Some(&local), Some(&remote), Side::Host),
                   MergeOutcome::Keep { value: b"same".to_vec(), changed: false });
    }

    #[test]
    fn a_client_cannot_steal_a_team() {
        let mut owners = TeamOwners::new();
        // The host grants team 3 to the client.
        assert!(owners.assign(3, Side::Client, Side::Host));
        // A client re-joining and confirming its own team is a harmless no-op.
        assert!(owners.assign(3, Side::Client, Side::Client));
        assert_eq!(owners.owner_of(3), Some(Side::Client));

        // The attack that must fail: a client claiming a team the host owns.
        assert!(owners.assign(4, Side::Host, Side::Host));
        assert!(!owners.assign(4, Side::Client, Side::Client),
            "a client must not take a team the host owns");
        assert_eq!(owners.owner_of(4), Some(Side::Host));

        // And claiming an unowned team is refused too: only the host allocates.
        assert!(!owners.assign(9, Side::Client, Side::Client));
        assert_eq!(owners.owner_of(9), None);
    }

    #[test]
    fn field_merge_reports_only_changed_teams() {
        let mut owners = TeamOwners::new();
        owners.assign(1, Side::Host, Side::Host);
        owners.assign(2, Side::Client, Side::Host);
        let mut local = BTreeMap::new();
        local.insert(1, row(1, "h1", 1));
        local.insert(2, row(2, "host-guess", 2));
        let mut remote = BTreeMap::new();
        remote.insert(1, row(1, "h1", 1));
        remote.insert(2, row(2, "c2", 2));
        let changed = merge_field(&owners, Side::Host, &local, &remote);
        assert!(!changed.contains_key(&1), "team 1 already agreed");
        assert_eq!(changed.get(&2).unwrap(), &b"c2".to_vec(), "team 2 must take the client's value");
    }

    #[test]
    fn teams_of_lists_ownership_for_the_ai_disable_pass() {
        let mut owners = TeamOwners::new();
        owners.assign(1, Side::Host, Side::Host);
        owners.assign(2, Side::Client, Side::Host);
        owners.assign(3, Side::Client, Side::Host);
        assert_eq!(owners.teams_of(Side::Client), vec![2, 3]);
        assert_eq!(owners.teams_of(Side::Host), vec![1]);
    }

    // ---- 3. instant contracts --------------------------------------------------------------

    #[test]
    fn two_players_cannot_sign_the_same_person_on_the_same_day() {
        let mut ledger = ContractLedger::new();
        assert_eq!(ledger.claim(42, Side::Host, 1, 10), Ok(()));
        assert_eq!(ledger.confirm(42, Side::Host, 1, 10), Ok(()));
        assert_eq!(ledger.claim(42, Side::Client, 2, 10), Err(ClaimDenied::AlreadySignedToday));
        assert_eq!(ledger.claim(42, Side::Client, 2, 10).unwrap_err().message(),
                   "that person was already signed today");
    }

    #[test]
    fn a_live_claim_blocks_the_other_player_before_any_signing() {
        let mut ledger = ContractLedger::new();
        assert_eq!(ledger.claim(7, Side::Host, 1, 4), Ok(()));
        assert_eq!(ledger.claim(7, Side::Client, 2, 4), Err(ClaimDenied::ClaimedByOther));
        ledger.release(7, Side::Host);
        assert_eq!(ledger.claim(7, Side::Client, 2, 4), Ok(()));
    }

    #[test]
    fn a_repeat_claim_from_the_same_player_is_not_a_conflict() {
        let mut ledger = ContractLedger::new();
        ledger.claim(9, Side::Host, 1, 3).unwrap();
        assert_eq!(ledger.claim(9, Side::Host, 1, 3), Err(ClaimDenied::DuplicateFromSamePlayer));
    }

    #[test]
    fn signing_someone_on_a_later_day_is_allowed() {
        let mut ledger = ContractLedger::new();
        ledger.claim(5, Side::Host, 1, 1).unwrap();
        ledger.confirm(5, Side::Host, 1, 1).unwrap();
        assert_eq!(ledger.claim(5, Side::Client, 2, 2), Ok(()), "a different day is a different slot");
    }

    #[test]
    fn a_negotiation_spanning_midnight_still_cannot_double_sign() {
        let mut ledger = ContractLedger::new();
        ledger.claim(11, Side::Host, 1, 5).unwrap();
        ledger.confirm(11, Side::Host, 1, 5).unwrap();
        assert_eq!(ledger.confirm(11, Side::Host, 1, 5), Err(ClaimDenied::AlreadySignedToday));
    }

    #[test]
    fn a_remote_signing_is_adopted_and_a_real_conflict_is_reported() {
        let mut ledger = ContractLedger::new();
        assert!(ledger.observe_remote_signing(77, Side::Client, 2, 8));
        assert_eq!(ledger.signed_on(77), Some(8));
        // Same person, same day, from the other peer: a genuine disagreement that must be surfaced,
        // because one player has already been told it worked.
        assert!(!ledger.observe_remote_signing(77, Side::Host, 1, 8));
    }

    #[test]
    fn the_ledger_records_history_for_the_report() {
        let mut ledger = ContractLedger::new();
        ledger.claim(1, Side::Host, 1, 2).unwrap();
        ledger.confirm(1, Side::Host, 1, 2).unwrap();
        ledger.claim(2, Side::Client, 2, 2).unwrap();
        ledger.confirm(2, Side::Client, 2, 2).unwrap();
        assert_eq!(ledger.history().len(), 2);
        assert_eq!(ledger.history()[1].team, 2);
    }

    #[test]
    fn the_other_players_claim_is_visible_before_signing() {
        let mut ledger = ContractLedger::new();
        ledger.claim(3, Side::Client, 2, 1).unwrap();
        assert_eq!(ledger.claimed_by(3), Some(Side::Client));
        assert_eq!(ledger.signed_on(3), None);
    }

    // ---- 4. votes --------------------------------------------------------------------------

    #[test]
    fn both_players_votes_are_counted() {
        let mut votes = VoteBox::new(2);
        votes.cast(Vote { side: Side::Host, proposal: 1, option: 0 });
        assert!(!votes.result(1).complete(), "must not resolve before both have voted");
        votes.cast(Vote { side: Side::Client, proposal: 1, option: 0 });
        let r = votes.result(1);
        assert!(r.complete());
        assert_eq!(r.tally.get(&0), Some(&2), "both votes counted");
        assert_eq!(r.winner, Some(0));
        assert!(!r.tied);
    }

    #[test]
    fn disagreeing_votes_produce_a_tie_rather_than_a_winner() {
        // The failure this prevents: the host's vote winning because the host resolved first.
        let mut votes = VoteBox::new(2);
        votes.cast(Vote { side: Side::Host, proposal: 2, option: 1 });
        votes.cast(Vote { side: Side::Client, proposal: 2, option: 0 });
        let r = votes.result(2);
        assert!(r.complete());
        assert!(r.tied, "one vote each is a tie, not a host win");
        assert_eq!(r.winner, None);
    }

    #[test]
    fn changing_your_mind_replaces_your_vote() {
        let mut votes = VoteBox::new(2);
        votes.cast(Vote { side: Side::Host, proposal: 3, option: 1 });
        assert_eq!(votes.cast(Vote { side: Side::Host, proposal: 3, option: 2 }), Some(1));
        assert_eq!(votes.cast_count(3), 1, "one player is still one vote");
        votes.cast(Vote { side: Side::Client, proposal: 3, option: 2 });
        let r = votes.result(3);
        assert_eq!(r.winner, Some(2));
        assert!(!r.tied);
    }

    #[test]
    fn a_second_vote_from_one_player_cannot_outvote_the_other() {
        // The real risk with "the host resolves": re-casting as a way to outweigh a single opponent.
        let mut votes = VoteBox::new(2);
        votes.cast(Vote { side: Side::Client, proposal: 4, option: 0 });
        votes.cast(Vote { side: Side::Host, proposal: 4, option: 1 });
        votes.cast(Vote { side: Side::Host, proposal: 4, option: 1 });
        let r = votes.result(4);
        assert_eq!(r.cast, 2);
        assert!(r.tied, "spamming a vote must not create a majority");
    }

    #[test]
    fn proposals_are_counted_independently() {
        let mut votes = VoteBox::new(2);
        votes.cast(Vote { side: Side::Host, proposal: 1, option: 0 });
        votes.cast(Vote { side: Side::Client, proposal: 1, option: 1 });
        votes.cast(Vote { side: Side::Client, proposal: 2, option: 3 });
        assert_eq!(votes.cast_count(1), 2);
        assert_eq!(votes.cast_count(2), 1);
        assert!(votes.result(1).complete());
        assert!(!votes.result(2).complete());
    }

    #[test]
    fn clearing_a_resolved_proposal_does_not_disturb_others() {
        let mut votes = VoteBox::new(2);
        votes.cast(Vote { side: Side::Host, proposal: 1, option: 0 });
        votes.cast(Vote { side: Side::Host, proposal: 2, option: 1 });
        votes.clear(1);
        assert_eq!(votes.cast_count(1), 0);
        assert_eq!(votes.cast_count(2), 1);
    }

    #[test]
    fn an_empty_round_reports_no_winner() {
        let votes = VoteBox::new(2);
        let r = votes.result(99);
        assert!(!r.complete());
        assert_eq!(r.winner, None);
        assert!(!r.tied);
    }
}