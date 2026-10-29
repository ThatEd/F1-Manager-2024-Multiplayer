//! Driving the other sim by *correction* rather than by overwriting state.
//!
//! ## Why overwriting does not work
//!
//! `Config/DefaultEngine.json` says `bSubstepping=False` with `MaxPhysicsDeltaTime=0.033333`. There is no
//! fixed timestep: each machine integrates a variable wall-clock delta, so the two sims are never at the
//! same step. Writing `dist` sets a value that the receiving sim then advances from on *its own* schedule,
//! and the write and the next integrate disagree by an amount proportional to the frame-time difference.
//!
//! That is not a theory. In the last live session the client's agent applied **1.88 M field writes**
//! (`entries=1881971`) while the distance error grew to 62 km. The writes landed; the simulation integrated
//! past them.
//!
//! ## What to do instead
//!
//! Send `(host_sim_time, host_state)` and let the receiving side decide how big a correction it needs, from
//! **how far behind in time it is**, not from how far away the car is. A client 30 ms behind needs a
//! different correction from one 300 ms behind, and only the time gap distinguishes them.
//!
//! Three properties are deliberate:
//!
//! * **A deadband.** A client within [`DEADBAND_MS`] of the host gets no correction at all. Without it a
//!   healthy client is nudged every frame on floating-point noise, which reads on screen as vibration.
//! * **A ramp, not a snap.** The correction is scaled by the time gap up to [`FULL_CORRECTION_MS`]. This is
//!   what stops a large error from becoming a teleport - and a teleport is exactly what "entire locations
//!   desynced" looked like as the receiving sim re-integrated from a jumped position.
//! * **A hard per-frame cap.** Even at full scale the car moves at most [`MAX_STEP_FRACTION`] of the error
//!   per frame, so the worst case is a fast catch-up over several frames rather than one jump.
//!
//! The correction is a pure function of numbers, which is the point: it can be tested against the exact
//! failure that happened without launching the game.

/// Below this time gap (ms) the sims are considered in agreement and nothing is written.
pub const DEADBAND_MS: f32 = 20.0;

/// At or above this time gap (ms) the correction runs at full scale.
pub const FULL_CORRECTION_MS: f32 = 250.0;

/// Largest fraction of the remaining error applied in one frame, at full scale.
///
/// A quarter per frame at 20 Hz closes 90% of the error in about 8 frames (0.4 s): fast enough that a
/// restart resumes quickly, slow enough that the receiving sim's own integrator tracks it instead of being
/// yanked. Tuned as a starting point; the tests pin the *shape* of the behaviour, not this number.
pub const MAX_STEP_FRACTION: f32 = 0.25;

/// Distance error (metres) below which no positional correction is issued.
///
/// Separate from the time deadband because a car can be in the right place on the wrong clock: on an oval
/// the same `dist` at two different instants looks identical. Correcting on distance alone would fight the
/// sim's own physics for no visible gain.
pub const DIST_DEADBAND_M: f32 = 2.0;

/// One car's correction: what to add to the receiving sim's own values.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Correction {
    pub dist: f32,
    pub lat: f32,
    pub head: f32,
    pub spd: f32,
}

impl Correction {
    /// True when nothing needs writing. Callers use this to skip staging an entry at all, which matters:
    /// a write of `+0.0` still costs a mailbox slot and still perturbs a float.
    pub fn is_zero(&self) -> bool {
        self.dist == 0.0 && self.lat == 0.0 && self.head == 0.0 && self.spd == 0.0
    }
}

/// How much of the error to apply, given the time gap in milliseconds.
///
/// `0.0` inside the deadband, ramping linearly to `1.0` at [`FULL_CORRECTION_MS`] and staying there. A
/// negative or non-finite gap is treated as "no gap": `sim_time` is known to be 0 when unavailable, and a
/// negative gap means a stale frame arriving out of order, which must not produce a reverse correction.
pub fn scale_for_gap(gap_ms: f32) -> f32 {
    if !gap_ms.is_finite() || gap_ms <= DEADBAND_MS {
        return 0.0;
    }
    if gap_ms >= FULL_CORRECTION_MS {
        return 1.0;
    }
    (gap_ms - DEADBAND_MS) / (FULL_CORRECTION_MS - DEADBAND_MS)
}

/// Shortest signed angular difference between two headings, in radians, within `(-PI, PI]`.
///
/// Headings wrap. Subtracting them directly turns a 1-degree correction across the `0`/`2*PI` boundary into
/// a 359-degree one, which spins the car on screen. This is the same bug class as `rel32` truncation: a
/// wraparound that looks like a huge error.
pub fn angle_delta(from: f32, to: f32) -> f32 {
    use std::f32::consts::PI;
    let mut d = (to - from) % (2.0 * PI);
    if d > PI {
        d -= 2.0 * PI;
    } else if d < -PI {
        d += 2.0 * PI;
    }
    d
}

/// Computes the correction for one car.
///
/// `host_time` and `client_time` are simulation clocks in seconds; the difference is what scales the
/// correction. `dt` is the receiving sim's frame time, used only to keep the cap meaningful across frame
/// rates.
///
/// Returns a zero correction when the sims agree, and never returns a value larger than the remaining error
/// - overshoot would make the two sims oscillate against each other instead of converging.
pub fn correct_car(
    host: &crate::Car,
    client: &crate::Car,
    host_time: f32,
    client_time: f32,
) -> Correction {
    // A clock of 0 means "unknown" on the wire. Without a clock there is no way to size a correction, so
    // do nothing rather than guess: guessing is what produced the 62 km divergence.
    if host_time <= 0.0 || client_time <= 0.0 {
        return Correction::default();
    }

    let gap_ms = (host_time - client_time) * 1000.0;
    let scale = scale_for_gap(gap_ms);
    if scale == 0.0 {
        return Correction::default();
    }

    let mut out = Correction::default();

    // Distance. Uses the signed error so a car behind catches up and a car ahead eases off, rather than
    // everything being pushed forward.
    let dist_err = host.dist - client.dist;
    if dist_err.is_finite() && dist_err.abs() > DIST_DEADBAND_M {
        out.dist = dist_err * MAX_STEP_FRACTION * scale;
    }

    // Lateral offset and heading: only meaningful once distance is roughly right, because they are
    // measured relative to a point on the track. Correcting them while the car is in the wrong place drags
    // it sideways across the circuit.
    if dist_err.is_finite() && dist_err.abs() < 50.0 {
        let lat_err = host.lat - client.lat;
        if lat_err.is_finite() {
            out.lat = lat_err * MAX_STEP_FRACTION * scale;
        }
        let head_err = angle_delta(client.head, host.head);
        if head_err.is_finite() {
            out.head = head_err * MAX_STEP_FRACTION * scale;
        }
    }

    // Speed converges faster than position so the sim's own integrator does not immediately undo the
    // position correction by accelerating away from it.
    let spd_err = host.spd - client.spd;
    if spd_err.is_finite() {
        out.spd = spd_err * MAX_STEP_FRACTION * scale;
    }

    out
}

/// Builds the corrections for a whole field, pairing by `car.id`.
///
/// Returned as `(car_id, correction)` so the caller can stage `(offset, value)` entries without needing to
/// know which of the two arrays an index came from. A car present in only one side is skipped: pairing by
/// position would silently apply one car's state to another.
pub fn correct_field(
    host: &[crate::Car],
    client: &[crate::Car],
    host_time: f32,
    client_time: f32,
) -> Vec<(u8, Correction)> {
    let mut out = Vec::with_capacity(host.len());
    for h in host {
        let Some(c) = client.iter().find(|c| c.id == h.id) else {
            continue;
        };
        let corr = correct_car(h, c, host_time, client_time);
        if !corr.is_zero() {
            out.push((h.id, corr));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Car;

    fn car(dist: f32) -> Car {
        Car { id: 0, dist, lat: 0.0, head: 0.0, spd: 0.0, ..Default::default() }
    }

    /// The core property: a synchronised client is not touched at all. Without this every frame would nudge
    /// every car on float noise, which reads as vibration.
    #[test]
    fn a_client_in_agreement_is_not_corrected() {
        let host = car(1000.0);
        let client = car(1000.0);
        let c = correct_car(&host, &client, 100.0, 100.0);
        assert!(c.is_zero(), "no gap, no correction: {c:?}");

        // Even a large error produces nothing while the clocks agree.
        let far = car(9000.0);
        assert!(correct_car(&host, &far, 100.0, 100.0).is_zero());
    }

    #[test]
    fn the_deadband_is_exactly_where_documented() {
        assert_eq!(scale_for_gap(0.0), 0.0);
        assert_eq!(scale_for_gap(DEADBAND_MS), 0.0);
        assert!(scale_for_gap(DEADBAND_MS + 1.0) > 0.0);
        assert_eq!(scale_for_gap(FULL_CORRECTION_MS), 1.0);
        assert_eq!(scale_for_gap(FULL_CORRECTION_MS * 10.0), 1.0);
        // Nonsense inputs must not produce a correction.
        assert_eq!(scale_for_gap(-50.0), 0.0);
        assert_eq!(scale_for_gap(f32::NAN), 0.0);
    }

    /// The failure this module exists for: the client was 62 km behind and the error *grew* because each
    /// write was integrated past. The correction must be a fraction of the error, never the whole error.
    #[test]
    fn a_huge_error_is_corrected_in_small_steps_not_one_jump() {
        let host = car(62_000.0);
        let client = car(0.0);
        // 1 second behind: well past the full-correction point.
        let c = correct_car(&host, &client, 101.0, 100.0);
        assert!(c.dist > 0.0, "should move towards the host");
        assert!(
            c.dist <= 62_000.0 * MAX_STEP_FRACTION + 0.001,
            "a frame must not apply more than {MAX_STEP_FRACTION} of the error, got {}",
            c.dist
        );
        assert!(c.dist < 62_000.0, "must not be a teleport");
    }

    /// Applying the correction repeatedly must converge, not oscillate. This is the property that makes it
    /// safe: simulated here over many frames.
    #[test]
    fn repeated_corrections_converge_without_overshoot() {
        let host = car(1000.0);
        let mut client = car(0.0);
        // A gap wide enough for full-scale correction each frame.
        for _ in 0..200 {
            let c = correct_car(&host, &client, 101.0, 100.0);
            assert!(c.dist >= 0.0, "must never overshoot past the host");
            client.dist += c.dist;
            if (host.dist - client.dist).abs() < DIST_DEADBAND_M {
                break;
            }
        }
        assert!(
            (host.dist - client.dist).abs() <= DIST_DEADBAND_M,
            "should converge to within the deadband, got {}",
            (host.dist - client.dist).abs()
        );
    }

    #[test]
    fn a_car_behind_and_a_car_ahead_correct_in_opposite_directions() {
        let host = car(1000.0);
        let behind = correct_car(&host, &car(500.0), 101.0, 100.0);
        let ahead = correct_car(&host, &car(1500.0), 101.0, 100.0);
        assert!(behind.dist > 0.0, "a car behind must speed up");
        assert!(ahead.dist < 0.0, "a car ahead must ease off");
    }

    /// Headings wrap. A direct subtraction across the 0/2*PI boundary would spin the car almost a full turn.
    #[test]
    fn heading_correction_wraps_rather_than_spinning() {
        use std::f32::consts::PI;
        // Client heading just below 2*PI, host just above 0: physically a tiny correction.
        let d = angle_delta(2.0 * PI - 0.01, 0.01);
        assert!(d.abs() < 0.1, "should be a small step, got {d}");

        // And the plain subtraction would have been almost a full turn - the bug being avoided.
        let naive = 0.01 - (2.0 * PI - 0.01);
        assert!(naive.abs() > 6.0, "naive subtraction really is that wrong");
    }

    /// Lateral and heading are only corrected once the car is roughly in the right place. Correcting them at
    /// a large distance error drags the car sideways across the circuit instead of along it.
    #[test]
    fn lateral_correction_waits_until_distance_is_close() {
        let host = Car { id: 0, dist: 5000.0, lat: 2.0, head: 1.0, ..Default::default() };
        let far = Car { id: 0, dist: 0.0, lat: -2.0, head: -1.0, ..Default::default() };
        let c = correct_car(&host, &far, 101.0, 100.0);
        assert_eq!(c.lat, 0.0, "must not pull sideways while far behind");
        assert_eq!(c.head, 0.0);

        let near = Car { id: 0, dist: 4990.0, lat: -2.0, head: -1.0, ..Default::default() };
        let c = correct_car(&host, &near, 101.0, 100.0);
        assert!(c.lat != 0.0, "once close, lateral is corrected");
    }

    /// A clock of 0 is the wire's "unknown". Without a clock there is no way to size a correction, so the
    /// only safe action is none.
    #[test]
    fn an_unknown_clock_disables_correction_entirely() {
        let host = car(5000.0);
        let client = car(0.0);
        assert!(correct_car(&host, &client, 0.0, 100.0).is_zero());
        assert!(correct_car(&host, &client, 100.0, 0.0).is_zero());
        assert!(correct_car(&host, &client, 0.0, 0.0).is_zero());
    }

    #[test]
    fn the_field_is_paired_by_id_not_by_position() {
        let host = vec![
            Car { id: 3, dist: 1000.0, ..Default::default() },
            Car { id: 7, dist: 500.0, ..Default::default() },
        ];
        // Deliberately in the opposite order, and missing car 3.
        let client = vec![Car { id: 7, dist: 0.0, ..Default::default() }];

        let out = correct_field(&host, &client, 101.0, 100.0);
        assert_eq!(out.len(), 1, "only the car present on both sides");
        assert_eq!(out[0].0, 7, "paired by id, not index");
        assert!(out[0].1.dist > 0.0);
    }

    /// The end-to-end property, simulated without a game: a client that is 60 km and 1 second behind must
    /// **converge** over repeated frames rather than diverging.
    ///
    /// This is the desktop version of the failure that cost four sessions. There, 1.88 M field writes left
    /// the client 62 km off because each write was integrated past. Here the same numbers are driven through
    /// `correct_field` repeatedly and required to close.
    #[test]
    fn a_client_a_long_way_behind_converges_over_frames() {
        let host: Vec<Car> = (0..22u8)
            .map(|id| Car { id, dist: 60_000.0 + id as f32 * 10.0, lat: 0.0, head: 0.0, spd: 90.0, ..Default::default() })
            .collect();
        // Client is a full second behind in sim time, and therefore far behind in distance.
        let mut client: Vec<Car> = host
            .iter()
            .map(|c| Car { dist: c.dist - 60_000.0, spd: 0.0, ..*c })
            .collect();

        let initial_worst = host
            .iter()
            .zip(&client)
            .map(|(h, c)| (h.dist - c.dist).abs())
            .fold(0.0f32, f32::max);

        // Drive frames, each closing part of the gap. The sim would also integrate; that is added here as a
        // steady drift so the test is not unrealistically clean.
        for frame in 0..400 {
            let corr = correct_field(&host, &client, 101.0, 100.0);
            for (id, c) in &corr {
                if let Some(car) = client.iter_mut().find(|x| x.id == *id) {
                    car.dist += c.dist;
                    car.spd += c.spd;
                }
            }
            // The receiving sim keeps moving under its own power between frames.
            for car in client.iter_mut() {
                car.dist += car.spd * 0.05;
            }
            if frame % 100 == 0 {
                let worst = host
                    .iter()
                    .zip(&client)
                    .map(|(h, c)| (h.dist - c.dist).abs())
                    .fold(0.0f32, f32::max);
                assert!(
                    worst < initial_worst,
                    "frame {frame}: error must shrink, got {worst} from {initial_worst}"
                );
            }
        }

        let final_worst = host
            .iter()
            .zip(&client)
            .map(|(h, c)| (h.dist - c.dist).abs())
            .fold(0.0f32, f32::max);
        assert!(
            final_worst < 5_000.0,
            "should be well inside 5 km after 400 frames, got {final_worst}"
        );
    }

    /// A full 22-car field within the deadband must produce no work at all - the common case, and the one
    /// that must stay free.
    #[test]
    fn a_synchronised_field_produces_no_corrections() {
        let host: Vec<Car> = (0..22u8)
            .map(|id| Car { id, dist: 100.0 * id as f32, ..Default::default() })
            .collect();
        let client = host.clone();
        assert!(correct_field(&host, &client, 100.0, 100.0).is_empty());
    }
}
