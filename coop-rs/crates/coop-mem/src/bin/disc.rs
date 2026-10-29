use coop_mem::Process;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pid: u32 = args.iter().position(|a| a == "--pid")
        .and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok())
        .expect("usage: coop-disc --pid <pid> [--wait N]");
    let wait: u64 = args.iter().position(|a| a == "--wait")
        .and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(6);

    let p = Process::open(pid).expect("open");
    let base = p.module_base("F1Manager24.exe").expect("module base");
    println!("module base 0x{base:X}");
    let cands = coop_mem::objects::find_sim_candidates(&p, base);
    println!("{} candidate(s)", cands.len());
    println!();
    for &c in &cands {
        match p.read_cars(c) {
            Ok(cars) => {
                let max = cars.iter().map(|x| x.dist).filter(|d| d.is_finite()).fold(0.0f32, f32::max);
                let nz = cars.iter().filter(|x| x.dist.abs() > 0.001).count();
                let sim = p.read(c + coop_proto::layout::off::SIM_TIME, 4).ok()
                    .map(|b| f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))).unwrap_or(0.0);
                println!("0x{c:X}: max_dist={max:.1} nonzero={nz} simtime={sim:.2} car0.dist={:.3}",
                    cars.first().map(|c| c.dist).unwrap_or(0.0));
            }
            Err(e) => println!("0x{c:X}: unreadable ({e})"),
        }
    }
    println!();
    println!("sampling for {wait}s to find which candidate STEPS...");
    let mut first: Vec<(usize, f32)> = Vec::new();
    for &c in &cands {
        if let Ok(cars) = p.read_cars(c) {
            first.push((c, cars.iter().map(|x| x.dist).fold(f32::MIN, f32::max)));
        }
    }
    std::thread::sleep(std::time::Duration::from_secs(wait));
    for (c, was) in first {
        if let Ok(cars) = p.read_cars(c) {
            let now = cars.iter().map(|x| x.dist).fold(f32::MIN, f32::max);
            let moved = now != was;
            println!("0x{c:X}: max_dist {was:.1} -> {now:.1} changed={moved}");
        }
    }
}
