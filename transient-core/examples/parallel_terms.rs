use std::thread::sleep;
use std::time::{Duration, Instant};
use transient_core::*;

/// An asynchronous term: `abduce` moves the rest to another thread and there it takes `ms` to give `n`
fn slow(n: u64, ms: u64) -> Trans<u64> {
    tdo! {
        abduce();
        lift_io(move || { sleep(Duration::from_millis(ms)); n })
    }
}

fn main() {
    // Applicative: the three terms start at once; the result comes out when all three are there.
    // It takes ~30 ms (the slowest), not 60 ms (the sum).
    let t0 = Instant::now();
    let total = keep_all(slow(1, 30) + slow(2, 10) + slow(3, 20));
    println!("a + b + c        = {:?}   en {:?}", total, t0.elapsed());

    // Alternative: the right branch is executed because the left one is asynchronous (`empty` in this thread);
    // the results arrive as a stream, as they finish.
    let t0 = Instant::now();
    let carrera = keep_all(slow(1, 30) | slow(2, 10));
    println!("a | b            = {:?}   en {:?}", carrera, t0.elapsed());

    // Mix: two pairs in parallel and the alternative between their sums
    let t0 = Instant::now();
    let mezcla = keep_all((slow(1, 40) + slow(2, 40)) | (slow(10, 5) + slow(20, 5)));
    println!("(a+b) | (c+d)    = {:?}   en {:?}", mezcla, t0.elapsed());

    // With a function of our own: lift2 is the only primitive; the operators come out of it
    let max = keep_all(lift2(u64::max, slow(7, 20), slow(4, 10)));
    println!("lift2(max, a, b) = {:?}", max);
}
