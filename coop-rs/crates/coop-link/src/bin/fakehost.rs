// A fake host for exercising the client's failure paths without a game.
//
// The crash being fixed was caused by the client writing into an object after the session had
// finished. Reproducing that against a real game is expensive and destructive, so this fake host
// reproduces the *signal* instead: it publishes a live clock for a while, then freezes it while
// continuing to send frames. If the client's staleness guard works, its write count stops rising.
//
// Deliberately uses the real wire format from coop-proto, so the client under test is the real one
// and nothing about the transport is mocked.

use std::io::Write;
use std::net::TcpStream;
use std::thread::sleep;
use std::time::{Duration, Instant};

use coop_proto::{encode, Car, Msg, Side};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let addr = arg(&args, "--supervisor").unwrap_or_else(|| "127.0.0.1:45990".into());
    let hz: u32 = arg(&args, "--hz").and_then(|s| s.parse().ok()).unwrap_or(20);
    // Frames of live clock before the clock freezes. Mirrors "race ends" without a real race.
    let live_frames: u32 = arg(&args, "--live-frames").and_then(|s| s.parse().ok()).unwrap_or(80);
    let total_frames: u32 = arg(&args, "--frames").and_then(|s| s.parse().ok()).unwrap_or(200);

    println!(
        "[fakehost] connecting to {addr} at {hz} Hz; live for {live_frames} frames, \
         then freezing the clock for the rest of {total_frames}"
    );

    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    stream.set_nodelay(true).ok();

    let hello = encode(&Msg::Hello { side: Side::Host, cars_owned: 22 });
    stream.write_all(&hello).map_err(|e| format!("hello: {e}"))?;

    let period = Duration::from_millis(1000 / hz.max(1) as u64);
    let mut clock = 0f32;
    let started = Instant::now();

    for tick in 0..total_frames {
        if tick < live_frames {
            clock += 1.0 / hz as f32;
        }
        // Cars advance only while the clock does, exactly as a real sim behaves.
        let cars: Vec<Car> = (0..22)
            .map(|i| Car {
                id: i as u8,
                dist: clock * 50.0 + i as f32 * 12.0,
                spd: 60.0,
                lap: (clock / 90.0) as i32,
                pos: i as u8,
                ..Default::default()
            })
            .collect();

        let frame = encode(&Msg::Snapshot {
            tick,
            sim_time: clock,
            race_time: clock,
            cars,
        });
        stream.write_all(&frame).map_err(|e| format!("write: {e}"))?;

        if tick + 1 == live_frames {
            println!("[fakehost] clock now FROZEN at {clock:.1} - session-over signal sent from here");
        }
        sleep(period);
    }

    println!("[fakehost] sent {total_frames} frames in {:?}", started.elapsed());
    Ok(())
}
