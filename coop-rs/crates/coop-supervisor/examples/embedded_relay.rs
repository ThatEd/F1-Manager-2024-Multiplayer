// Proves the relay can be embedded: a library caller binds it on 0.0.0.0 exactly as the agent does,
// then two peers exchange a frame through it. No coop-supervisor.exe involved.
use std::io::Write;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn main() {
    let relay = coop_supervisor::serve("0.0.0.0", 0).expect("bind");
    println!("relay bound on {} (in-process)", relay.addr);

    // 0.0.0.0 is a bind address, not a destination: connect over loopback, which is what a peer on this
    // machine does. A peer on the other machine uses the LAN address.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], relay.addr.port()));
    let mut a = TcpStream::connect(addr).expect("peer a");
    let mut b = TcpStream::connect(addr).expect("peer b");
    a.set_nodelay(true).ok();
    b.set_nodelay(true).ok();
    b.set_read_timeout(Some(Duration::from_secs(3))).ok();
    thread::sleep(Duration::from_millis(150));

    a.write_all(&coop_proto::encode(&coop_proto::Msg::Hello { side: coop_proto::Side::Host, cars_owned: 22 })).unwrap();
    a.write_all(&coop_proto::encode(&coop_proto::Msg::Bye)).unwrap();
    a.flush().unwrap();
    // The first frame registers the peer; the second is what gets relayed.
    b.write_all(&coop_proto::encode(&coop_proto::Msg::Hello { side: coop_proto::Side::Client, cars_owned: 11 })).unwrap();
    b.flush().unwrap();
    thread::sleep(Duration::from_millis(200));
    a.write_all(&coop_proto::encode(&coop_proto::Msg::Bye)).unwrap();
    a.flush().unwrap();

    use std::io::Read;
    let mut buf = [0u8; 1024];
    match b.read(&mut buf) {
        Ok(n) if n > 0 => println!("PASS: client received {n} bytes relayed by the in-process relay"),
        other => println!("FAIL: read returned {other:?}"),
    }
}
