//! The coordination sender: the half of the coordination protocol that was missing.
//!
//! `coop-proto` defines `Clock`, `Contract` and `Vote`; `coop-supervisor::arbitrate` enforces them. Both
//! were exercised only by tests, because **nothing on either machine produced those frames**. This module
//! is that producer, and it lives in the agent because the agent is the only component resident inside the
//! game process, next to the front-end that knows when a player paused, offered a contract or voted.
//!
//! ## Why this cannot be a second protocol connection
//!
//! The supervisor binds one tunnel per peer and keys the side - and therefore the arbitration identity -
//! off the `Hello` on that socket: a frame arriving on a socket that has not sent `Hello` is dropped by
//! name (`sent data before Hello; dropped`). The agent's socket is exactly such a socket: it announces
//! itself with `Msg::Agent` and never sends `Hello`, because the peer slot belongs to the game DLL
//! (`coop-link`), which does send `Hello`.
//!
//! So a coordination frame must leave the machine **over the agent's own socket**, and the supervisor is
//! what stamps the sender identity onto it. That is also the safer split: the front-end can say *"this
//! player paused"* but can never claim to be the host, because it does not label the frame at all.

use coop_proto::coordination::{PauseReason, TeamId, PersonId, Day};
use coop_proto::{encode, Msg, Side};

/// Which peer this agent speaks for, derived from the lobby's own role vocabulary (`role=host` /
/// `role=client`, as `lobby.js` sends it when a team is claimed).
///
/// Defaulting to `Host` rather than erroring is deliberate: `parse_side` is only used for a frame the
/// caller has already decided to send, and a solo install must behave like a host, not go silent.
pub fn parse_side(s: &str) -> Side {
    match s.trim().to_ascii_lowercase().as_str() {
        "client" | "guest" | "join" => Side::Client,
        _ => Side::Host,
    }
}

/// Why a coordination frame was rejected before it could be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// No `PauseReason` with that number.
    ///
    /// The enum is closed and the wire carries bytes (`Clock.reasons` is widened to `u8` so the protocol
    /// module does not depend on `coordination`), so an out-of-range byte would otherwise reach the far
    /// side as a reason nobody can name - and a clock that can never be cleared.
    UnknownPauseReason,
    /// A pause frame referenced no valid reason at all.
    EmptyClock,
    /// A vote option outside `0x00..=0xFE`.
    ///
    /// `0xFF` is the supervisors' *resolved tie* sentinel (`encode_vote_result`), so letting a real vote
    /// use it would read to both peers as "the vote was tied".
    ReservedVoteOption,
    /// A contract frame with a kind the supervisor does not define.
    UnknownContractKind,
    /// A day of zero or below: the game's calendar is 1-based and the day rule is compared, not counted.
    BadDay,
    /// A required number was missing or not a number.
    ///
    /// Distinct from the field-specific errors because the fix is different: this one is a malformed
    /// request, not a value the game would reject. It exists because defaulting an unparseable field to
    /// zero is how `option=` would silently cast a vote for the first option on the list.
    BadNumber(&'static str),
}

impl Reject {
    /// The line the operator sees. Mirrors the supervisor's refusal wording so one `log` filter shows both
    /// the local rejection and the remote one.
    pub fn message(self) -> &'static str {
        match self {
            Reject::UnknownPauseReason => "unknown pause reason",
            Reject::EmptyClock => "a clock frame needs at least one reason",
            Reject::ReservedVoteOption => "0xFF is the resolved-tie sentinel, not a vote",
            Reject::UnknownContractKind => "unknown contract kind",
            Reject::BadDay => "day must be 1 or later",
            Reject::BadNumber(_) => "a required number was missing or not a number",
        }
    }
}

/// A required numeric query field.
///
/// Strict on purpose. The previous shape defaulted every field to zero, which turns `?option=abc` into a
/// vote for option 0 - a real vote, counted, for something the player did not choose. Missing and
/// malformed are treated the same because neither has a defensible default.
fn require_num(
    q: &std::collections::HashMap<String, String>,
    k: &'static str,
) -> Result<i64, Reject> {
    match q.get(k) {
        Some(v) => v.trim().parse::<i64>().map_err(|_| Reject::BadNumber(k)),
        None => Err(Reject::BadNumber(k)),
    }
}

/// Parses a comma-separated list of `PauseReason` numbers, as the front-end produces from its own pause
/// flags (`"0"`, `"0,2"`, `""`).
///
/// A list, not a bitmask, because `Clock` is documented as full state rather than a delta and
/// `ClockGate` stores a `BTreeSet`; sending the set directly means the far side cannot disagree about
/// bit order.
pub fn parse_reasons(csv: &str) -> Result<Vec<u8>, Reject> {
    let mut out: Vec<u8> = Vec::new();
    for part in csv.split(',') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        let n: u8 = match t.parse() {
            Ok(n) => n,
            // A non-numeric member is a bug in the caller, and silently dropping it would turn "paused"
            // into "running" - the one direction that must never be guessed.
            Err(_) => return Err(Reject::UnknownPauseReason),
        };
        if PauseReason::from_u8(n).is_none() {
            return Err(Reject::UnknownPauseReason);
        }
        if !out.contains(&n) {
            out.push(n);
        }
    }
    if out.is_empty() {
        return Err(Reject::EmptyClock);
    }
    out.sort_unstable();
    Ok(out)
}

/// Numbers for a reason set, for logging and for the status endpoint.
pub fn reasons_csv(reasons: &[u8]) -> String {
    reasons
        .iter()
        .map(|r| PauseReason::from_u8(*r).map(|p| p.as_u8()).unwrap_or(*r).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Builds a `Clock` frame. `tick` is the sender's own counter; the supervisor does not compare it (the
/// peers do, so a late frame cannot resurrect an older pause state), which is why it is passed rather
/// than read from the game here - the agent cannot see the game's sim clock from this thread.
pub fn clock_frame(reasons: &[u8], tick: u32) -> Vec<u8> {
    encode(&Msg::Clock { reasons: reasons.to_vec(), tick })
}

/// Contract kinds, named because `Msg::Contract.kind` is a bare `u8` on the wire and a wrong number is
/// silently accepted by the supervisor's `_ => Ruling::Pass` arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractKind {
    /// I am negotiating with this person; nobody else may sign them.
    Claim,
    /// I stopped negotiating; free them.
    Release,
    /// The signing landed. The day rule is checked again here.
    Confirm,
    /// The offer was refused or withdrawn.
    Refused,
}

impl ContractKind {
    pub fn as_u8(self) -> u8 {
        match self {
            ContractKind::Claim => 0,
            ContractKind::Release => 1,
            ContractKind::Confirm => 2,
            ContractKind::Refused => 3,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "claim" | "0" => ContractKind::Claim,
            "release" | "1" => ContractKind::Release,
            "confirm" | "sign" | "2" => ContractKind::Confirm,
            "refused" | "withdraw" | "declined" | "3" => ContractKind::Refused,
            _ => return None,
        })
    }
}

/// Builds a `Contract` frame, refusing values the supervisor would mis-handle.
///
/// The validation is here rather than in the supervisor because a refused frame costs a round trip and,
/// worse, the supervisor's refusal arm writes `Msg::Bye` to the *game's* socket - which is the peer that
/// did nothing wrong. Catching it locally keeps a typo in the front-end from disturbing the relay.
pub fn contract_frame(person: i32, team: TeamId, day: Day, kind: ContractKind) -> Result<Vec<u8>, Reject> {
    if day < 1 {
        return Err(Reject::BadDay);
    }
    Ok(encode(&Msg::Contract { person, team, day, kind: kind.as_u8() }))
}

/// Builds a `Vote` frame, refusing the sentinel.
pub fn vote_frame(proposal: u16, option: u8) -> Result<Vec<u8>, Reject> {
    if option == 0xFF {
        return Err(Reject::ReservedVoteOption);
    }
    Ok(encode(&Msg::Vote { proposal, option }))
}

/// Which coordination family a request named, so the HTTP layer stays a switch rather than a parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Clock { reasons: Vec<u8> },
    Contract { person: i32, team: TeamId, day: Day, kind: ContractKind },
    Vote { proposal: u16, option: u8 },
}

/// Turns the query string of a `/coop/coordinate/...` request into a frame.
///
/// Kept free of I/O and of the HTTP server so the whole grammar is testable without a socket. `kind` is
/// the path segment (`clock`/`contract`/`vote`); `q` is the already-decoded query map.
pub fn parse_request(
    kind: &str,
    q: &std::collections::HashMap<String, String>,
) -> Result<Request, Reject> {
    match kind {
        "clock" => {
            let csv = q.get("reasons").cloned().unwrap_or_default();
            Ok(Request::Clock { reasons: parse_reasons(&csv)? })
        }
        "contract" => {
            let kind = match q.get("kind").and_then(|k| ContractKind::parse(k)) {
                Some(k) => k,
                None => return Err(Reject::UnknownContractKind),
            };
            let person = require_num(q, "person")?;
            let team = require_num(q, "team")?;
            let day = require_num(q, "day")?;
            if !(i32::MIN as i64..=i32::MAX as i64).contains(&person) {
                return Err(Reject::BadNumber("person"));
            }
            if !(0..=u8::MAX as i64).contains(&team) {
                return Err(Reject::BadNumber("team"));
            }
            Ok(Request::Contract {
                person: person as PersonId,
                team: team as TeamId,
                day: day.clamp(i32::MIN as i64, i32::MAX as i64) as Day,
                kind,
            })
        }
        "vote" => {
            let option = require_num(q, "option")?;
            let proposal = require_num(q, "proposal")?;
            if !(0..=0xFF).contains(&option) {
                return Err(Reject::ReservedVoteOption);
            }
            // Rejected here as well as in `vote_frame`: the parser is what the HTTP layer reports from,
            // so accepting the sentinel here would make the endpoint answer 202 for a vote it then
            // refuses to build.
            if option == 0xFF {
                return Err(Reject::ReservedVoteOption);
            }
            if !(0..=u16::MAX as i64).contains(&proposal) {
                return Err(Reject::BadNumber("proposal"));
            }
            Ok(Request::Vote { proposal: proposal as u16, option: option as u8 })
        }
        _ => Err(Reject::UnknownContractKind),
    }
}

/// The frame for a parsed request, so `parse_request` and validation cannot drift apart.
pub fn frame_for(req: &Request) -> Result<Vec<u8>, Reject> {
    match req {
        // The tick is filled by the caller, which owns the counter; zero here means "no ordering hint",
        // which the peers treat as older than any real tick.
        Request::Clock { reasons } => Ok(clock_frame(reasons, 0)),
        Request::Contract { person, team, day, kind } => contract_frame(*person, *team, *day, *kind),
        Request::Vote { proposal, option } => vote_frame(*proposal, *option),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_side_comes_from_the_lobbys_own_role_words() {
        assert_eq!(parse_side("host"), Side::Host);
        assert_eq!(parse_side("client"), Side::Client);
        assert_eq!(parse_side("CLIENT"), Side::Client);
        // An unknown or empty role must not make the agent silent.
        assert_eq!(parse_side(""), Side::Host);
        assert_eq!(parse_side("whatever"), Side::Host);
    }

    #[test]
    fn a_reason_list_is_parsed_sorted_and_deduplicated() {
        assert_eq!(parse_reasons("2,0,0").unwrap(), vec![0, 2]);
        assert_eq!(parse_reasons("0").unwrap(), vec![0]);
        // Whitespace and empty members are the normal shape of anything built by a UI string join.
        assert_eq!(parse_reasons(" 4 , 1 ").unwrap(), vec![1, 4]);
    }

    #[test]
    fn every_real_pause_reason_is_accepted() {
        for n in 0u8..=4 {
            assert!(parse_reasons(&n.to_string()).is_ok(), "reason {n} should parse");
        }
        // 5 is outside PauseReason; a clock carrying it could never be cleared.
        assert_eq!(parse_reasons("5"), Err(Reject::UnknownPauseReason));
    }

    #[test]
    fn an_unpausable_clock_frame_is_refused_rather_than_guessed() {
        assert_eq!(parse_reasons(""), Err(Reject::EmptyClock));
        assert_eq!(parse_reasons("  , "), Err(Reject::EmptyClock));
        assert_eq!(parse_reasons("paused"), Err(Reject::UnknownPauseReason));
    }

    #[test]
    fn the_tie_sentinel_cannot_be_cast_as_a_real_vote() {
        assert_eq!(vote_frame(3, 0xFF), Err(Reject::ReservedVoteOption));
        assert!(vote_frame(3, 0xFE).is_ok());
        assert!(vote_frame(3, 0).is_ok());
    }

    #[test]
    fn a_contract_needs_a_real_day_and_a_known_kind() {
        assert_eq!(contract_frame(1, 2, 0, ContractKind::Claim), Err(Reject::BadDay));
        assert_eq!(contract_frame(1, 2, -5, ContractKind::Claim), Err(Reject::BadDay));
        assert!(contract_frame(1, 2, 1, ContractKind::Claim).is_ok());
        assert_eq!(ContractKind::parse("confirm"), Some(ContractKind::Confirm));
        assert_eq!(ContractKind::parse("nonsense"), None);
        assert_eq!(
            parse_request("contract", &q(&[("person", "4"), ("team", "1"), ("day", "9")])),
            Err(Reject::UnknownContractKind)
        );
    }

    /// A missing field must not become a vote or a claim for whatever zero means. `option=abc` reading as
    /// option 0 would cast a real, counted vote the player never made.
    #[test]
    fn a_missing_or_malformed_number_is_refused_instead_of_defaulting_to_zero() {
        assert_eq!(
            parse_request("vote", &q(&[("proposal", "1"), ("option", "abc")])),
            Err(Reject::BadNumber("option"))
        );
        assert_eq!(
            parse_request("vote", &q(&[("proposal", "1")])),
            Err(Reject::BadNumber("option"))
        );
        assert_eq!(
            parse_request("vote", &q(&[("option", "1")])),
            Err(Reject::BadNumber("proposal"))
        );
        assert_eq!(
            parse_request("contract", &q(&[("person", "4"), ("day", "9"), ("kind", "claim")])),
            Err(Reject::BadNumber("team"))
        );
        // A team number that is not a byte would wrap to a different team, so it is refused.
        assert_eq!(
            parse_request(
                "contract",
                &q(&[("person", "4"), ("team", "256"), ("day", "9"), ("kind", "claim")])
            ),
            Err(Reject::BadNumber("team"))
        );
    }

    #[test]
    fn the_request_grammar_round_trips_to_a_frame_the_supervisor_can_read() {
        let req = parse_request("clock", &q(&[("reasons", "0,3")])).unwrap();
        assert_eq!(req, Request::Clock { reasons: vec![0, 3] });
        let frame = frame_for(&req).unwrap();
        match coop_proto::decode(&frame).expect("must be a valid frame") {
            Msg::Clock { reasons, tick } => {
                assert_eq!(reasons, vec![0, 3]);
                assert_eq!(tick, 0);
            }
            other => panic!("expected a clock frame, got {other:?}"),
        }

        let req = parse_request(
            "contract",
            &q(&[("person", "42"), ("team", "3"), ("day", "11"), ("kind", "claim")]),
        )
        .unwrap();
        match coop_proto::decode(&frame_for(&req).unwrap()).unwrap() {
            Msg::Contract { person, team, day, kind } => {
                assert_eq!((person, team, day, kind), (42, 3, 11, 0));
            }
            other => panic!("expected a contract frame, got {other:?}"),
        }

        let req = parse_request("vote", &q(&[("proposal", "7"), ("option", "2")])).unwrap();
        match coop_proto::decode(&frame_for(&req).unwrap()).unwrap() {
            Msg::Vote { proposal, option } => assert_eq!((proposal, option), (7, 2)),
            other => panic!("expected a vote frame, got {other:?}"),
        }
    }

    #[test]
    fn a_vote_option_outside_a_byte_is_refused_not_truncated() {
        // Truncating 256 to 0 would silently cast a vote nobody made.
        assert_eq!(
            parse_request("vote", &q(&[("proposal", "1"), ("option", "256")])),
            Err(Reject::ReservedVoteOption)
        );
        assert_eq!(
            parse_request("vote", &q(&[("proposal", "1"), ("option", "255")])),
            Err(Reject::ReservedVoteOption)
        );
        assert_eq!(
            parse_request("vote", &q(&[("proposal", "70000"), ("option", "1")])),
            Err(Reject::BadNumber("proposal"))
        );
    }

    #[test]
    fn an_unknown_coordination_kind_is_an_error_rather_than_a_silent_pass() {
        assert!(parse_request("teleport", &q(&[])).is_err());
    }

    #[test]
    fn rejection_messages_say_what_to_fix() {
        for r in [
            Reject::UnknownPauseReason,
            Reject::EmptyClock,
            Reject::ReservedVoteOption,
            Reject::UnknownContractKind,
            Reject::BadDay,
            Reject::BadNumber("option"),
        ] {
            assert!(!r.message().is_empty(), "{r:?} needs an operator-facing message");
        }
        assert_eq!(reasons_csv(&[0, 4]), "0,4");
        assert_eq!(reasons_csv(&[]), "");
    }
}
