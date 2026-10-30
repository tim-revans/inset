//! Measurements quoted in `docs/language/runtime.md`. They print rather than
//! assert, and are ignored by default. Run them with:
//!
//! ```sh
//! cargo test --release -p inset-host --test measure -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use std::time::{Duration, Instant};

use common::{compiled, fake, load, text};
use inset_host::Program;

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

#[test]
#[ignore]
fn starting_a_built_program() {
    let path = compiled("examples/hello-http");
    let (_, uncached) = timed(|| Program::load_uncached(&path).unwrap());
    let (_, warm) = timed(|| Program::load(&path).unwrap());
    let (program, cached) = timed(|| Program::load(&path).unwrap());
    let (mut running, started) = timed(|| program.start(&fake()).unwrap());
    let (_, first) = timed(|| text(&mut running, "GET", "/hello/Ada"));
    let n = 10_000;
    let (_, many) = timed(|| {
        for _ in 0..n {
            text(&mut running, "GET", "/hello/Ada");
        }
    });
    println!("load, compiling:          {uncached:?}");
    println!("load, filling the cache:  {warm:?}");
    println!("load, from the cache:     {cached:?}");
    println!("start:                    {started:?}");
    println!("first request:            {first:?}");
    println!("each later request:       {:?}", many / n);
}

#[test]
#[ignore]
fn lending_a_value_across_a_boundary() {
    let program = load("fixture-spike");
    let mut running = program.start(&fake()).unwrap();
    let mut each = |route: &str, bytes: usize, times: u32| {
        let path = format!("/{route}/{bytes}/{times}");
        let (reply, took) = timed(|| text(&mut running, "POST", &path));
        assert_eq!(reply.0, 200);
        took / times
    };
    println!(
        "{:>9} {:>12} {:>12} {:>12}",
        "bytes", "moved", "lent", "copy"
    );
    for (bytes, times) in [
        (0, 100_000),
        (16, 100_000),
        (1 << 10, 100_000),
        (64 << 10, 10_000),
        (1 << 20, 1_000),
    ] {
        let moved = each("move", bytes, times);
        let lent = each("lend", bytes, times);
        println!(
            "{bytes:>9} {moved:>12?} {lent:>12?} {:>12?}",
            lent.saturating_sub(moved)
        );
    }
}
