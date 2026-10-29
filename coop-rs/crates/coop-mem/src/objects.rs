//! Locate the RaceSimManager through `GUObjectArray` - read-only, no code patching.
//!
//! UE4SS logs the array address at startup (`[PS] Found GUObjectArray: 0x…`), which is
//! `base + RVA`, so it can be rebuilt every launch.
//!
//! The internal layout (`FChunkedFixedUObjectArray` and `FUObjectItem`) is version specific and the
//! header that documents it lives in a private submodule, so nothing here is hard-coded from
//! memory. Instead the layout is *derived* from the live process and accepted only if it scores
//! well, which matters because a wrong stride silently produces plausible-looking garbage.
//!
//! Identification does not need FName resolution either: the RaceSimManager's class has a property
//! size of `0xA132F0` (measured by the Lua prototype), and `UClass + 0x58` holds exactly that. So we
//! look for the object whose class size matches, which is far stronger than matching a name string.

use crate::Process;

/// Offsets inside `FUObjectArray` where `ObjObjects` may begin.
const ARRAY_OFFSETS: [usize; 3] = [0x10, 0x00, 0x08];
/// `FUObjectItem` sizes seen across UE4/UE5.
const ITEM_SIZES: [usize; 2] = [16, 24];
/// `UClass + 0x58` = `UStruct::PropertiesSize`.
const CLASS_SIZE_OFF: usize = 0x58;
/// `UObject + 0x10` = `UClass*`.
const OBJECT_CLASS_OFF: usize = 0x10;

#[derive(Debug, Clone, Copy)]
pub struct ObjectArray {
    /// pointer to the array-of-chunks (or array-of-items) allocation
    pub objects: usize,
    pub max_elements: i32,
    pub num_elements: i32,
    pub item_size: usize,
    pub chunk_bytes: usize,
    pub array_offset: usize,
}

impl ObjectArray {
    pub fn items_per_chunk(&self) -> usize {
        self.chunk_bytes / self.item_size
    }
}

fn plausible_ptr(p: usize) -> bool {
    // Windows user-space heap/committed range; 64 KB aligned rejections catch small integers read as
    // pointers, which is the usual way a wrong offset masquerades as a valid one.
    p >= 0x1_0000 && p < 0x7FFF_FFFF_FFFF && p % 8 == 0
}

fn read_ptr(proc: &Process, at: usize) -> Option<usize> {
    let b = proc.read(at, 8).ok()?;
    Some(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as usize)
}

fn read_i32(proc: &Process, at: usize) -> Option<i32> {
    let b = proc.read(at, 4).ok()?;
    Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Address of `FUObjectItem[i].Object` for a given layout.
pub fn object_of(proc: &Process, arr: &ObjectArray, index: usize) -> Option<usize> {
    let per_chunk = arr.items_per_chunk();
    if per_chunk == 0 {
        return None;
    }
    let chunk = index / per_chunk;
    let within = index % per_chunk;
    let chunk_base = read_ptr(proc, arr.objects + chunk * 8)?;
    if !plausible_ptr(chunk_base) {
        return None;
    }
    read_ptr(proc, chunk_base + within * arr.item_size)
}

/// Score a candidate layout by how many sampled entries dereference to plausible objects with a
/// plausible class pointer.
fn score(proc: &Process, arr: &ObjectArray) -> usize {
    let n = arr.num_elements.max(1) as usize;
    let sample = 32.min(n);
    let mut good = 0;
    for k in 0..sample {
        let idx = if n <= sample { k } else { k * (n / sample) };
        if let Some(obj) = object_of(proc, arr, idx) {
            if plausible_ptr(obj) {
                if let Some(class) = read_ptr(proc, obj + OBJECT_CLASS_OFF) {
                    if plausible_ptr(class) {
                        good += 1;
                    }
                }
            }
        }
    }
    good
}

/// Works out the object-array layout from the live process.
pub fn discover(proc: &Process, gua: usize) -> Result<ObjectArray, String> {
    let mut best: Option<(usize, ObjectArray)> = None;
    for &off in &ARRAY_OFFSETS {
        let Some(objects) = read_ptr(proc, gua + off) else { continue };
        if !plausible_ptr(objects) {
            continue;
        }
        let Some(max_elements) = read_i32(proc, gua + off + 0x10) else { continue };
        let Some(num_elements) = read_i32(proc, gua + off + 0x14) else { continue };
        if num_elements <= 0 || max_elements < num_elements || num_elements > 5_000_000 {
            continue;
        }
        // Chunks are equal-sized contiguous allocations, so two chunk pointers give the stride
        // without knowing the chunk size as a constant.
        let (c0, c1) = (read_ptr(proc, objects), read_ptr(proc, objects + 8));
        let chunk_bytes = match (c0, c1) {
            (Some(a), Some(b)) if plausible_ptr(a) && plausible_ptr(b) && b > a => b - a,
            _ => 0,
        };
        for &item_size in &ITEM_SIZES {
            for &chunk in &[chunk_bytes, 0x1_0000, 0x2_0000, 0x1_0000 * item_size] {
                if chunk == 0 {
                    continue;
                }
                let cand = ObjectArray {
                    objects,
                    max_elements,
                    num_elements,
                    item_size,
                    chunk_bytes: chunk,
                    array_offset: off,
                };
                let s = score(proc, &cand);
                if s >= 24 && best.map(|(bs, _)| s > bs).unwrap_or(true) {
                    best = Some((s, cand));
                }
            }
        }
    }
    match best {
        Some((s, arr)) => {
            eprintln!(
                "layout: array_offset=0x{:X} item_size={} chunk=0x{:X} num={} max={} score={}/32",
                arr.array_offset, arr.item_size, arr.chunk_bytes, arr.num_elements, arr.max_elements, s
            );
            Ok(arr)
        }
        None => Err("no layout scored high enough; refusing to guess".into()),
    }
}

/// Every layout that is merely *plausible*, best score first.
///
/// A fixed score threshold was the wrong idea: the number of live UObjects depends on where the game
/// is, and a loaded save had about 6 000 more objects than the session before it, which turned a
/// comfortable score into a false negative. The probe score says "this decodes as an object array",
/// not "this is the right one". Validity is judged by the caller, from something it actually cares
/// about — here, whether the RaceSimManager class size can be reached through the array.
pub fn discover_ranked(proc: &Process, gua: usize) -> Vec<ObjectArray> {
    let mut out: Vec<(usize, ObjectArray)> = Vec::new();
    for &off in &ARRAY_OFFSETS {
        let Some(objects) = read_ptr(proc, gua + off) else { continue };
        if !plausible_ptr(objects) {
            continue;
        }
        let Some(max_elements) = read_i32(proc, gua + off + 0x10) else { continue };
        let Some(num_elements) = read_i32(proc, gua + off + 0x14) else { continue };
        if num_elements <= 0 || max_elements < num_elements || num_elements > 5_000_000 {
            continue;
        }
        let (c0, c1) = (read_ptr(proc, objects), read_ptr(proc, objects + 8));
        let chunk_bytes = match (c0, c1) {
            (Some(a), Some(b)) if plausible_ptr(a) && plausible_ptr(b) && b > a => b - a,
            _ => 0,
        };
        for &item_size in &ITEM_SIZES {
            for &chunk in &[chunk_bytes, 0x1_0000, 0x2_0000, 0x1_0000 * item_size] {
                if chunk == 0 {
                    continue;
                }
                let cand = ObjectArray {
                    objects,
                    max_elements,
                    num_elements,
                    item_size,
                    chunk_bytes: chunk,
                    array_offset: off,
                };
                let s = score(proc, &cand);
                if s > 0 {
                    out.push((s, cand));
                }
            }
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, a)| a).collect()
}

/// Every object whose class reports `class_size` at `UClass + 0x58`.
///
/// There is normally more than one: UE creates a class default object per class, so the live
/// instance and its CDO both match. The caller has to tell them apart by content, not by order.
pub fn find_all_by_class_size(proc: &Process, arr: &ObjectArray, class_size: i32) -> Vec<usize> {
    let n = arr.num_elements.max(0) as usize;
    let mut hits = Vec::new();
    for i in 0..n {
        let Some(obj) = object_of(proc, arr, i) else { continue };
        if !plausible_ptr(obj) {
            continue;
        }
        let Some(class) = read_ptr(proc, obj + OBJECT_CLASS_OFF) else { continue };
        if !plausible_ptr(class) {
            continue;
        }
        if read_i32(proc, class + CLASS_SIZE_OFF) == Some(class_size) {
            hits.push(obj);
        }
    }
    hits
}

/// Picks the live instance out of the candidates.
///
/// A class default object has zeroed car records. The live one has either a non-zero race distance
/// or a non-zero sim clock. If neither can be told apart (menu, nothing loaded) this returns None
/// rather than picking one and being wrong.
/// Scores a candidate and returns the most plausible one, rather than demanding exactly one.
///
/// Two earlier rules made a clean start impossible:
///
///  * `with_data.len() == 1` refused to attach whenever two candidates looked live. A real game often
///    has the class-default object alongside the live instance, so the ambiguous case is the normal
///    case, and "not guessing" meant never attaching at all.
///  * liveness was `dist != 0 || simtime > 0`. At a session start the cars sit on the grid with
///    `dist = 0` and the clock at 0, so the single moment worth attaching to was judged dead. The
///    publisher then joined mid-race, which guarantees an unsynchronised start.
///
/// So candidates are now ranked and the best is taken even when its score is 0. Being attached to a
/// not-yet-started sim is correct: it is the only way the client sees frame 1.
pub fn pick_live(proc: &Process, candidates: &[usize]) -> Option<usize> {
    use coop_proto::layout;

    let mut best: Option<(i32, usize)> = None;
    for &obj in candidates {
        let mut score = 0i32;

        // A live sim has a readable car block with a spread of distances once running. Distinct
        // values beat "non-zero" because they also hold mid-session.
        let mut dists = Vec::new();
        for car in 0..layout::CAR_MAX.min(22) {
            let at = layout::car_start(car) + layout::off::DIST;
            if let Ok(b) = proc.read(obj + at, 4) {
                dists.push(f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            }
        }
        let readable = dists.len() >= 20;
        if readable {
            score += 10;
        }
        let finite = dists.iter().filter(|d| d.is_finite()).count();
        if finite >= 20 {
            score += 10;
        }
        let distinct = {
            let mut v: Vec<u32> = dists.iter().map(|d| d.to_bits()).collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        if distinct > 3 {
            score += 20;
        }
        if dists.iter().any(|d| *d > 1.0) {
            score += 20;
        }

        let sim = proc
            .read(obj + layout::off::SIM_TIME, 4)
            .ok()
            .map(|b| f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            .unwrap_or(0.0);
        if sim.is_finite() {
            score += 10;
        }
        if sim > 0.0 {
            score += 20;
        }

        // A pre-start sim scores low but is still the right thing to hold: it is readable and it is
        // the only candidate that will step. Record it and keep looking for a better one.
        eprintln!(
            "candidate 0x{obj:X}: score={score} readable={readable} distinct_dists={distinct} \
             simtime={sim:.1}"
        );

        let better = match best {
            None => true,
            Some((b, _)) => score > b,
        };
        if better {
            best = Some((score, obj));
        }
    }

    match best {
        Some((score, obj)) => {
            eprintln!("[objects] chose 0x{obj:X} (score {score})");
            Some(obj)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plausible_pointer_rejects_small_integers() {
        assert!(!plausible_ptr(0));
        assert!(!plausible_ptr(0x40));
        assert!(!plausible_ptr(0x1234)); // not 8-aligned
        assert!(plausible_ptr(0x1_4000_0000));
        assert!(plausible_ptr(0x1_750E_0000));
    }

    #[test]
    fn items_per_chunk_is_derived_not_assumed() {
        let arr = ObjectArray {
            objects: 0,
            max_elements: 100,
            num_elements: 10,
            item_size: 16,
            chunk_bytes: 0x1_0000,
            array_offset: 0x10,
        };
        assert_eq!(arr.items_per_chunk(), 0x1000);
        let arr24 = ObjectArray { item_size: 24, ..arr };
        assert!(arr24.items_per_chunk() < arr.items_per_chunk());
    }
}

/// Full discovery starting from the main module base: locate GUObjectArray, derive its layout,
/// then pick the live RaceSimManager out of the CDO/live pair.
/// Every address that could be the race sim object, cheap enough to call every tick.
///
/// The publisher needs the whole set, not one pick: at a pre-start the live instance and a
/// class-default are indistinguishable (all car distances zero, clock at zero), so choosing one up
/// front is a coin toss. Holding the set and watching which one starts stepping makes the choice
/// correct at exactly the moment it becomes knowable, which is the session start.
pub fn find_sim_candidates(proc: &Process, module_base: usize) -> Vec<usize> {
    let Ok((_gua, arr)) = discover_anywhere(proc, module_base) else {
        return Vec::new();
    };
    find_all_by_class_size(proc, &arr, coop_proto::layout::OBJECT_SIZE as i32)
}

pub fn find_sim(proc: &Process, module_base: usize) -> Result<usize, String> {
    let (_gua, arr) = discover_anywhere(proc, module_base)?;
    let candidates = find_all_by_class_size(proc, &arr, coop_proto::layout::OBJECT_SIZE as i32);
    if candidates.is_empty() {
        return Err(format!("no object has class size 0x{:X}", coop_proto::layout::OBJECT_SIZE));
    }
    pick_live(proc, &candidates).ok_or_else(|| {
        format!(
            "{} candidate(s) but none looked live (menu?); not guessing",
            candidates.len()
        )
    })
}

/// Locates `GUObjectArray` without a fixed address, by its `(max, num)` shape.
///
/// There is no usable static address for it: `module_base + 0x7821450` is inside `.reloc` in the file,
/// so the bytes there are relocation data, and the value that worked for one session stopped working
/// after a save load. Rather than chase an RVA, scan the image's committed writable regions for the
/// pair of integers that `TUObjectArray` keeps at `+0x10`: an `int32 max` followed by an `int32 num`,
/// with `0 < num <= max` and `max` a round chunk count (a power of two, or a power of two times 65536).
///
/// The shape alone is weak, so each candidate is confirmed by reading its chunk array and requiring at
/// least one chunk to be a readable region full of object-like pointers. Returns candidates, most
/// plausible first, so the caller can keep trying discovery against each.
pub fn find_candidates(proc: &Process, module_base: usize) -> Vec<usize> {
    let Ok((base, size)) = proc.image_range(module_base) else {
        return Vec::new();
    };
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for region in proc.regions(base, size) {
        // Only writable data can hold the live object array; skip code and read-only tables.
        let writable = matches!(region.protect & 0xFF, 0x04 | 0x08 | 0x40 | 0x80);
        if !writable {
            continue;
        }
        let len = region.region_size.min(0x2000_0000);
        let Ok(bytes) = proc.read(region.base_address, len) else {
            continue;
        };
        for i in (0..bytes.len().saturating_sub(24)).step_by(8) {
            let max = u32::from_le_bytes([bytes[i + 0x10], bytes[i + 0x11], bytes[i + 0x12], bytes[i + 0x13]]) as usize;
            let num = u32::from_le_bytes([bytes[i + 0x14], bytes[i + 0x15], bytes[i + 0x16], bytes[i + 0x17]]) as usize;
            if !plausible_counts(max, num) {
                continue;
            }
            let objects = usize::from_le_bytes([
                bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3],
                bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7],
            ]);
            // A real chunk array lives on the heap. Small values are integers that happen to sit here.
            if objects < 0x1_0000 || objects % 8 != 0 {
                continue;
            }
            let score = chunk_score(proc, objects);
            if score > 0 {
                hits.push((score, region.base_address + i));
            }
        }
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().map(|(_, at)| at).collect()
}

/// True for the `(max_elements, num_elements)` pair of a UE object array.
fn plausible_counts(max: usize, num: usize) -> bool {
    if num == 0 || max < num || max > 0x40_0000 || num > 0x40_0000 {
        return false;
    }
    // UE grows the array in whole chunks, so the maximum is items-per-chunk times a chunk count.
    max.is_power_of_two() || (max % 0x1_0000 == 0 && (max / 0x1_0000).is_power_of_two())
}

/// How many of the first few chunk pointers point at readable memory that looks like an object array.
fn chunk_score(proc: &Process, objects: usize) -> usize {
    let mut score = 0;
    for i in 0..4 {
        let Some(chunk) = read_ptr(proc, objects + i * 8) else { break };
        if !plausible_ptr(chunk) {
            continue;
        }
        // The first few entries of a chunk are object pointers, all in the same heap area.
        let mut inner = 0;
        for j in 0..4 {
            if let Some(obj) = read_ptr(proc, chunk + j * 24) {
                if plausible_ptr(obj) {
                    inner += 1;
                }
            }
        }
        if inner >= 2 {
            score += inner;
        }
    }
    score
}

/// Finds a `GUObjectArray` whose contents actually reach the `RaceSimManager`.
///
/// Two things were wrong with the previous approach and both are fixed here. Address: the fixed RVA
/// is not a genuine static - it lands inside `.reloc` in the file - so it is a hint only, tried first
/// because it is free when it works, with a shape-based scan as the fallback. Validity: a probe score
/// threshold was fragile, because the live object count changes with game state, so the score is now
/// only used for ordering and the real test is whether the RaceSimManager class size can be found
/// through the array.
pub fn discover_anywhere(proc: &Process, module_base: usize) -> Result<(usize, ObjectArray), String> {
    let target = coop_proto::layout::OBJECT_SIZE as i32;

    let mut addresses: Vec<usize> = Vec::new();
    if module_base != 0 {
        addresses.push(module_base + coop_proto::layout::GUA_RVA);
    }
    addresses.extend(find_candidates(proc, module_base));
    addresses.dedup();

    if addresses.len() > 1 {
        eprintln!("[objects] {} candidate address(es) to try", addresses.len());
    }
    let mut usable = 0;
    for gua in addresses.iter().take(8) {
        let ranked = discover_ranked(proc, *gua);
        if ranked.is_empty() {
            continue;
        }
        usable += 1;
        for arr in ranked.iter().take(3) {
            if !find_all_by_class_size(proc, arr, target).is_empty() {
                eprintln!(
                    "[objects] GUObjectArray 0x{gua:X} offset=0x{:X} item={} chunk=0x{:X} num={}",
                    arr.array_offset, arr.item_size, arr.chunk_bytes, arr.num_elements
                );
                return Ok((*gua, *arr));
            }
        }
    }
    Err(format!(
        "no candidate array reached the RaceSimManager class (tried {} affordable address(es))",
        usable
    ))
}

