//! What the engine and the CSV layer must never do, over any input.

use payments_engine::csv_io::TransactionReader;
use payments_engine::engine::Engine;
use payments_engine::model::{ClientId, Kind, RejectReason, Transaction, TxId};
use payments_engine::money::Money;
use proptest::prelude::*;

/// Few clients and ids, so random sequences collide often enough to exercise
/// disputes instead of wandering off into unrelated ids.
fn transaction() -> impl Strategy<Value = Transaction> {
    let kind = prop_oneof![
        (0i64..1_000_000).prop_map(|units| Kind::Deposit(Money::from_units(units))),
        (0i64..1_000_000).prop_map(|units| Kind::Withdrawal(Money::from_units(units))),
        Just(Kind::Dispute),
        Just(Kind::Resolve),
        Just(Kind::Chargeback),
    ];
    (0..4 as ClientId, 0..8 as TxId, kind).prop_map(|(client, tx, kind)| Transaction {
        client,
        tx,
        kind,
    })
}

fn transactions() -> impl Strategy<Value = Vec<Transaction>> {
    prop::collection::vec(transaction(), 0..64)
}

/// Every row of `input`, as the reader yields them. Panics only on a fatal
/// error, which these inputs are built not to produce.
fn rows_of(input: &str) -> Vec<Result<Transaction, RejectReason>> {
    let mut reader = TransactionReader::new(input.as_bytes()).expect("header names all columns");
    let mut rows = Vec::new();
    while let Some(row) = reader.read().expect("input is well-formed CSV") {
        rows.push(row);
    }
    rows
}

/// The four fields of a well-formed row, as the text a partner would send.
fn row() -> impl Strategy<Value = [String; 4]> {
    (
        prop::sample::select(vec![
            "deposit",
            "withdrawal",
            "dispute",
            "resolve",
            "chargeback",
        ]),
        any::<ClientId>(),
        any::<TxId>(),
        0i64..1_000_000_000,
    )
        .prop_map(|(kind, client, tx, units)| {
            [
                kind.to_string(),
                client.to_string(),
                tx.to_string(),
                Money::from_units(units).to_string(),
            ]
        })
}

proptest! {
    // Counterexamples are reported in full in the failure message; persisting
    // them would write files into the source tree.
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    /// Formatting splits a value in two and pads the halves back out, which is
    /// where an off-by-one would hide.
    #[test]
    fn display_and_parse_round_trip(units in 0..=i64::MAX) {
        let amount = Money::from_units(units);
        prop_assert_eq!(amount.to_string().parse(), Ok(amount));
    }

    /// The parser is the only place arbitrary bytes reach arithmetic, so every
    /// input must yield an amount or an error, never a panic.
    #[test]
    fn parsing_is_total(text in "[0-9]{0,6}[.]?[0-9]{0,6}|[-+0-9.eE ]{0,12}") {
        if let Ok(amount) = text.parse::<Money>() {
            prop_assert_eq!(amount.to_string().parse(), Ok(amount), "{}", text);
        }
    }

    /// Leading zeros on the whole part and trailing zeros on the fraction are
    /// decoration, so padding an amount with them must not change what it
    /// parses to.
    ///
    /// Each half is drawn from an empty, an all-zero and an arbitrary case
    /// rather than one digit pattern: inputs with no significant digit are a
    /// vanishing fraction of uniform digit strings, and they are exactly where
    /// this property bites.
    #[test]
    fn padding_with_zeros_does_not_change_the_value(
        whole in prop_oneof![Just(String::new()), "0{1,4}", "[0-9]{1,6}"],
        frac in prop_oneof![Just(String::new()), "0{1,5}", "[0-9]{1,4}"],
        left in 0usize..4,
        right in 0usize..4,
    ) {
        // "." alone has no digits and is not an amount, so it is not a
        // spelling of the padded forms that do.
        prop_assume!(!whole.is_empty() || !frac.is_empty());

        let plain = format!("{whole}.{frac}");
        let padded = format!("{}{whole}.{frac}{}", "0".repeat(left), "0".repeat(right));
        prop_assert_eq!(
            plain.parse::<Money>(),
            padded.parse::<Money>(),
            "{} vs {}",
            plain,
            padded
        );
    }

    /// A field's meaning comes from the header, so moving the columns around
    /// must not change what a row means. The reader looks each name up rather
    /// than assuming a position, and this is what says so.
    #[test]
    fn column_order_does_not_change_a_row(
        values in row(),
        order in Just(vec![0usize, 1, 2, 3]).prop_shuffle(),
    ) {
        let names = ["type", "client", "tx", "amount"];
        let shuffled = format!(
            "{}\n{}\n",
            order.iter().map(|&i| names[i]).collect::<Vec<_>>().join(","),
            order.iter().map(|&i| values[i].as_str()).collect::<Vec<_>>().join(","),
        );
        let canonical = format!("{}\n{}\n", names.join(","), values.join(","));

        prop_assert_eq!(rows_of(&shuffled), rows_of(&canonical), "{}", shuffled);
    }

    /// Whitespace around a field is formatting, not data, so padding every
    /// field must leave the row meaning what it meant.
    #[test]
    fn surrounding_whitespace_does_not_change_a_row(
        values in row(),
        pad in prop::collection::vec(prop::sample::select(vec![" ", "\t", "  "]), 4),
    ) {
        let plain = format!("type,client,tx,amount\n{}\n", values.join(","));
        let padded = format!(
            "type,client,tx,amount\n{}\n",
            values
                .iter()
                .zip(&pad)
                .map(|(v, p)| format!("{p}{v}{p}"))
                .collect::<Vec<_>>()
                .join(","),
        );

        prop_assert_eq!(rows_of(&padded), rows_of(&plain), "{}", padded);
    }

    /// The reader is where arbitrary bytes enter the program, so no input may
    /// panic: every row becomes a transaction, a rejection, or a fatal error.
    #[test]
    fn reading_arbitrary_bytes_never_panics(body in prop::collection::vec(any::<u8>(), 0..300)) {
        let mut input = b"type,client,tx,amount\n".to_vec();
        input.extend(body);
        if let Ok(mut reader) = TransactionReader::new(&input[..]) {
            while let Ok(Some(_)) = reader.read() {}
        }
    }

    /// Held funds are the sum of the open disputes, so they never go below zero
    /// however the transactions interleave.
    #[test]
    fn held_funds_are_never_negative(transactions in transactions()) {
        let mut engine = Engine::new();
        for transaction in transactions {
            let _ = engine.apply(transaction);
            for (client, account) in engine.accounts() {
                prop_assert!(account.held >= Money::ZERO, "client {client}: {account:?}");
            }
        }
    }

    /// Balances move in pairs, so a half-applied rejection would break the
    /// relationship between available, held and total. A rejection may still
    /// create the client record it names.
    #[test]
    fn a_rejected_transaction_moves_no_money(transactions in transactions()) {
        let mut engine = Engine::new();
        for transaction in transactions {
            let before = engine.clone();
            if engine.apply(transaction).is_err() {
                for (client, account) in engine.accounts() {
                    let previous = before.accounts().get(client).copied().unwrap_or_default();
                    prop_assert_eq!(*account, previous, "client {}", client);
                }
            }
        }
    }

    /// Disputing and then resolving restores the account it started from.
    #[test]
    fn a_dispute_that_resolves_is_a_round_trip(transactions in transactions()) {
        let mut engine = Engine::new();
        for transaction in transactions {
            let _ = engine.apply(transaction);

            let before = engine.clone();
            let dispute = Transaction { kind: Kind::Dispute, ..transaction };
            if engine.apply(dispute).is_ok() {
                let resolve = Transaction { kind: Kind::Resolve, ..transaction };
                prop_assert!(engine.apply(resolve).is_ok());
                prop_assert_eq!(&engine, &before);
            }
        }
    }
}
