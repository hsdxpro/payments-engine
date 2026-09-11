//! A toy payments engine.
//!
//! Transactions stream from a CSV file into in-memory client accounts, and the
//! balances are written back out as CSV. The engine knows nothing about CSV and
//! the CSV layer knows nothing about account rules.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod model;
pub mod money;
