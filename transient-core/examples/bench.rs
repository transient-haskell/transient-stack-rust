//! Cost of `tdo!` / `bind` and of the rest of the core compared with plain Rust.
//! Run with: cargo run --release --example bench
//!
//! Each measure is the best of 7 batches (less system noise). Memory allocations per
//! iteration are also counted with a global allocator that counts them.

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;
use transient_core::*;

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(l.size(), Ordering::Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Runs `f` `iters` times, 7 batches, and gives (ns per iteration of the best batch, allocations per iteration)
fn measure(iters: u64, mut f: impl FnMut()) -> (f64, f64) {
    for _ in 0..(iters / 10).max(1) {
        f(); // warm-up
    }
    let mut best = f64::MAX;
    let mut allocs = 0.0;
    for _ in 0..7 {
        let a0 = ALLOCS.load(Ordering::Relaxed);
        let t = Instant::now();
        for _ in 0..iters {
            f();
        }
        let ns = t.elapsed().as_nanos() as f64 / iters as f64;
        if ns < best {
            best = ns;
            allocs = (ALLOCS.load(Ordering::Relaxed) - a0) as f64 / iters as f64;
        }
    }
    (best, allocs)
}

fn row(name: &str, steps: u32, (ns, allocs): (f64, f64), base: Option<f64>) {
    let per = ns / steps as f64;
    let ratio = base.map_or(String::new(), |b| format!("   x{:.1} vs base", ns / b));
    println!("  {name:<50} {ns:>10.1} ns  {per:>8.1} ns/paso  {allocs:>6.1} reservas{ratio}");
}

fn section(t: &str) {
    println!("\n== {t}");
}

fn main() {
    // ------------------------------------------------------------------------------------------------
    section("1. synchronous chain of 8 dependent steps");
    let x = AtomicU64::new(1);
    let seed = || black_box(x.load(Ordering::Relaxed));

    let plain = measure(2_000_000, || {
        let a = black_box(seed() + 1);
        let b = black_box(a + 1);
        let c = black_box(b + 1);
        let d = black_box(c + 1);
        let e = black_box(d + 1);
        let f = black_box(e + 1);
        let g = black_box(f + 1);
        black_box(g + 1);
    });
    row("plain Rust (straight code)", 8, plain, None);

    let fs: Vec<Box<dyn Fn(u64) -> u64>> = (0..8).map(|_| Box::new(|a: u64| black_box(a + 1)) as Box<dyn Fn(u64) -> u64>).collect();
    let dynchain = measure(2_000_000, || {
        black_box(fs.iter().fold(seed(), |a, f| f(a)));
    });
    row("cadena de Box<dyn Fn> (llamada indirecta)", 8, dynchain, None);

    let t = tdo! {
        let a = lift_io(|| black_box(1u64) + 1);
        let b = lift_io(move || black_box(a) + 1);
        let c = lift_io(move || black_box(b) + 1);
        let d = lift_io(move || black_box(c) + 1);
        let e = lift_io(move || black_box(d) + 1);
        let f = lift_io(move || black_box(e) + 1);
        let g = lift_io(move || black_box(f) + 1);
        lift_io(move || black_box(g) + 1)
    };
    let mut ctx = Ctx::new();
    let chain = measure(500_000, || {
        black_box(t.run(&mut ctx));
    });
    row("tdo! de 8 pasos (Trans, solo run)", 8, chain, None);

    // ------------------------------------------------------------------------------------------------
    section("2. un solo paso: pure / lift_io / bind / map");
    let p = pure(1u64);
    let mut ctx = Ctx::new();
    row("pure(1).run", 1, measure(2_000_000, || { black_box(p.run(&mut ctx)); }), None);
    let b1 = tdo! { let x = pure(1u64); pure(x + 1) };
    row("tdo!{ let x = pure(1); pure(x+1) }.run", 1, measure(2_000_000, || { black_box(b1.run(&mut ctx)); }), None);
    let m1 = pure(1u64).map(|x| x + 1);
    row("pure.map(|x| x+1).run", 1, measure(2_000_000, || { black_box(m1.run(&mut ctx)); }), None);

    // ------------------------------------------------------------------------------------------------
    section("3. alternative and applicative, synchronous terms");
    let o = or(pure(1u64), pure(2u64));
    row("pure | pure  (first term gives a value)", 1, measure(1_000_000, || { black_box(o.run(&mut ctx)); }), None);
    let o2 = or(empty::<u64>(), pure(2u64));
    row("empty | pure (cae a la segunda)", 1, measure(1_000_000, || { black_box(o2.run(&mut ctx)); }), None);
    let ap = lift2(|a: u64, b: u64| a + b, pure(1u64), pure(2u64));
    let apc = measure(300_000, || { black_box(ap.run(&mut ctx)); });
    row("lift2(+, pure, pure)  (a + b)", 1, apc, None);

    // ------------------------------------------------------------------------------------------------
    section("4. hilos: abduce frente a std::thread::spawn");
    let spawn_join = measure(20_000, || {
        std::thread::spawn(|| black_box(1u64)).join().unwrap();
    });
    row("std::thread::spawn + join", 1, spawn_join, None);
    let t0 = threads(0, tdo! { abduce(); lift_io(|| black_box(1u64)) });
    let mut ctx0 = Ctx::new();
    row("threads(0): abduce en el mismo hilo (sin hilo)", 1, measure(300_000, || { black_box(t0.run(&mut ctx0)); }), None);
    let t1 = tdo! { abduce(); lift_io(|| black_box(1u64)) };
    let abd = measure(20_000, || {
        black_box(keep_all(t1.clone()));
    });
    row("keep_all(abduce >> lift_io): one thread + tree + wait", 1, abd, Some(spawn_join.0));

    // ------------------------------------------------------------------------------------------------
    section("5. coste fijo de keep_all sin hilos");
    let k = pure(1u64);
    row("keep_all(pure(1))", 1, measure(50_000, || { black_box(keep_all(k.clone())); }), None);

    // ------------------------------------------------------------------------------------------------
    section("6. flujo de 10 000 eventos (parallel) frente a un bucle");
    let events = 10_000u64;
    let plain_loop = measure(200, || {
        let mut s = 0u64;
        for i in 0..events {
            s = s.wrapping_add(black_box(i));
        }
        black_box(s);
    });
    row("bucle normal de 10 000 iteraciones", events as u32, plain_loop, None);
    let sync_stream = measure(20, || {
        let n = AtomicU64::new(0);
        let t = threads(
            0,
            tdo! {
                let i = parallel(move || if n.fetch_add(1, Ordering::Relaxed) < events { StreamData::More(1u64) } else { StreamData::Done });
                lift_io(move || match i { StreamData::More(v) => black_box(v), _ => 0 })
            },
        );
        black_box(keep_all(t));
    });
    row("parallel + threads(0): cada evento en el mismo hilo", events as u32, sync_stream, None);
    let plain_threads_ns = {
        let (ns, _) = measure(3, || {
            let hs: Vec<_> = (0..events).map(|_| std::thread::spawn(|| black_box(1u64))).collect();
            hs.into_iter().for_each(|h| { h.join().unwrap(); });
        });
        ns
    };
    let thread_stream = measure(3, || {
        let n = AtomicU64::new(0);
        let t = tdo! {
            let i = parallel(move || if n.fetch_add(1, Ordering::Relaxed) < events { StreamData::More(1u64) } else { StreamData::Done });
            lift_io(move || match i { StreamData::More(v) => black_box(v), _ => 0 })
        };
        black_box(keep_all(t));
    });
    row("parallel normal: un hilo por evento", events as u32, thread_stream, Some(plain_threads_ns));
    row("std::thread::spawn por evento (referencia)", events as u32, (plain_threads_ns, 0.0), None);

    // ------------------------------------------------------------------------------------------------
    section("7. EVar: una escritura con un lector");
    let ev = EVar::<u64>::new();
    let hits = std::sync::Arc::new(AtomicU64::new(0));
    let h2 = hits.clone();
    let reader = tdo! { [h2] let v = read_evar(&ev); lift_io(move || { h2.fetch_add(v, Ordering::Relaxed); }) };
    let mut ctxr = Ctx::new();
    reader.run(&mut ctxr);
    row("EVar::write with 1 reader (inline continuation)", 1, measure(200_000, || ev.write(black_box(1))), None);
    black_box(hits.load(Ordering::Relaxed));
    let cb: Box<dyn Fn(u64) + Send + Sync> = Box::new(|v| { black_box(v); });
    row("referencia: llamar a una Box<dyn Fn>", 1, measure(5_000_000, || cb(black_box(1))), None);

    // ------------------------------------------------------------------------------------------------
    section("8. overhead of a tdo! step depending on the work done by that step");
    let step = chain.0 / 8.0;
    println!("  (coste medido por paso: {step:.0} ns)");
    for work_ns in [10.0, 100.0, 1_000.0, 10_000.0, 100_000.0] {
        println!("  paso que trabaja {work_ns:>9.0} ns -> sobrecoste de tdo! {:>6.2} %", 100.0 * step / work_ns);
    }
}
