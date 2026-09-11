//! Command line entry point: `payments-engine <transactions.csv>`.
//!
//! Balances go to stdout as CSV, diagnostics to stderr, so the output stays
//! usable as a file.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{self, Write};
use std::process::ExitCode;

use payments_engine::csv_io::{self, TransactionReader};
use payments_engine::engine::Engine;
use payments_engine::model::RejectReason;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Processes the input file named by the first argument.
///
/// Declined rows are counted, not propagated: the specification asks for
/// erroneous rows to be ignored. Only a failure that invalidates the run - no
/// input file, an unreadable stream, a header missing a column - stops
/// processing and sets a non-zero exit status.
fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let path = arguments
        .next()
        .ok_or("usage: payments-engine <transactions.csv>")?;
    if arguments.next().is_some() {
        eprintln!("warning: extra arguments ignored, only the first is read");
    }

    let file = File::open(&path).map_err(|error| format!("{}: {error}", path.to_string_lossy()))?;
    let mut reader = TransactionReader::new(file)?;
    let mut engine = Engine::new();
    let mut ignored = Ignored::default();

    while let Some(row) = reader.read()? {
        // Parsing and applying fail the same way, so they report the same way.
        if let Err(reason) = row.and_then(|transaction| engine.apply(transaction)) {
            ignored.record(reason);
        }
    }

    csv_io::write_accounts(io::stdout().lock(), engine.accounts())?;
    ignored.report(io::stderr().lock())?;
    Ok(())
}

/// Counts ignored rows by reason, so a bad input cannot produce a line of log
/// per transaction.
#[derive(Default)]
struct Ignored(BTreeMap<RejectReason, u64>);

impl Ignored {
    fn record(&mut self, reason: RejectReason) {
        let count = self.0.entry(reason).or_default();
        *count = count.saturating_add(1);
    }

    fn report(&self, mut sink: impl Write) -> io::Result<()> {
        let total = self.0.values().copied().fold(0, u64::saturating_add);
        if total == 0 {
            return Ok(());
        }
        writeln!(sink, "ignored {total} row(s):")?;
        for (reason, count) in &self.0 {
            writeln!(sink, "  {count:>10}  {reason}")?;
        }
        Ok(())
    }
}
