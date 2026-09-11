//! Generates a large, deterministic input file for measuring throughput.
//!
//! ```text
//! cargo run --release --example gen_input -- 10000000 > large.csv
//! ```
//!
//! Chargebacks are left out: they freeze accounts, and with far more
//! transactions than clients the run would measure the locked-account early
//! return instead of the work.

use std::env;
use std::io::{self, BufWriter, Write};

/// Deterministic, so the same row count always produces the same file.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }
}

fn main() -> io::Result<()> {
    let rows: u32 = env::args()
        .nth(1)
        .and_then(|argument| argument.parse().ok())
        .unwrap_or(1_000_000);

    let mut rng = Rng(0x5eed);
    let mut out = BufWriter::new(io::stdout().lock());
    writeln!(out, "type,client,tx,amount")?;

    // Owning an id by client puts disputes on the right account without
    // tracking state, so the run measures the hold and release paths.
    // Truncating to `u16` spreads them over the client id space.
    let owner = |tx: u32| tx as u16;

    for tx in 0..rows {
        // An earlier id, so disputes land on deposits that exist.
        let past = rng.next() as u32 % tx.max(1);
        match rng.next() % 100 {
            0..=64 => writeln!(
                out,
                "deposit,{},{tx},{}.{:04}",
                owner(tx),
                rng.next() % 1_000,
                rng.next() % 10_000
            )?,
            65..=89 => writeln!(
                out,
                "withdrawal,{},{tx},{}.{:04}",
                owner(tx),
                rng.next() % 1_000,
                rng.next() % 10_000
            )?,
            90..=96 => writeln!(out, "dispute,{},{past}", owner(past))?,
            _ => writeln!(out, "resolve,{},{past}", owner(past))?,
        }
    }

    out.flush()
}
