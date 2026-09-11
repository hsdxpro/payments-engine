//! The CSV boundary: rows in, accounts out.

use std::collections::HashMap;
use std::fmt;
use std::io;

use csv::{ReaderBuilder, StringRecord, Trim, Writer};

use crate::engine::Account;
use crate::model::{ClientId, Kind, RejectReason, Transaction};
use crate::money::Money;

/// Output column headings, in order.
const OUTPUT_HEADER: [&str; 5] = ["client", "available", "held", "total", "locked"];

/// A failure that stops the run. Ignored rows use [`RejectReason`] instead.
#[derive(Debug)]
pub enum Error {
    /// The input could not be read. Fatal: an I/O failure says nothing about
    /// where the stream resumes.
    Csv(csv::Error),
    /// The header does not name a column the engine needs.
    MissingColumn(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Csv(error) => write!(f, "reading CSV: {error}"),
            Self::MissingColumn(name) => write!(f, "input has no {name:?} column"),
        }
    }
}

impl std::error::Error for Error {}

impl From<csv::Error> for Error {
    fn from(error: csv::Error) -> Self {
        Self::Csv(error)
    }
}

/// Where each needed column sits in a row.
///
/// Read from the header, not assumed, so reordered or extra columns are
/// handled instead of silently misread.
#[derive(Clone, Copy, Debug)]
struct Columns {
    kind: usize,
    client: usize,
    tx: usize,
    amount: Option<usize>,
}

impl Columns {
    fn from_headers(headers: &StringRecord) -> Result<Self, Error> {
        let position = |name: &str| {
            headers
                .iter()
                .position(|heading| heading.eq_ignore_ascii_case(name))
        };
        Ok(Self {
            kind: position("type").ok_or(Error::MissingColumn("type"))?,
            client: position("client").ok_or(Error::MissingColumn("client"))?,
            tx: position("tx").ok_or(Error::MissingColumn("tx"))?,
            // Only deposits and withdrawals need it.
            amount: position("amount"),
        })
    }
}

/// Streams transactions out of a CSV source.
///
/// One row at a time, into a reused buffer, so input size costs no memory
/// here.
#[derive(Debug)]
pub struct TransactionReader<R> {
    reader: csv::Reader<R>,
    columns: Columns,
    record: StringRecord,
}

impl<R: io::Read> TransactionReader<R> {
    /// Reads the header and prepares to stream rows.
    ///
    /// Whitespace around fields is trimmed, and rows are allowed to be shorter
    /// than the header: `dispute, 1, 1` has no amount field at all.
    pub fn new(source: R) -> Result<Self, Error> {
        let mut reader = ReaderBuilder::new()
            .trim(Trim::All)
            .flexible(true)
            .from_reader(source);
        let columns = Columns::from_headers(reader.headers()?)?;
        Ok(Self {
            reader,
            columns,
            record: StringRecord::new(),
        })
    }

    /// Reads the next row, or [`None`] at end of input.
    ///
    /// Two failure levels: the outer [`Error`] ends the run, the inner
    /// [`RejectReason`] skips one row.
    pub fn read(&mut self) -> Result<Option<Result<Transaction, RejectReason>>, Error> {
        loop {
            match self.reader.read_record(&mut self.record) {
                Ok(false) => return Ok(None),
                Ok(true) => {
                    // The reader drops empty lines; this drops the ones that
                    // only look empty. `as_slice` is the fields without their
                    // separators, so the check is O(1).
                    if self.record.as_slice().is_empty() {
                        continue;
                    }
                    return Ok(Some(parse(&self.record, self.columns)));
                }
                // Boundaries are found by byte and already passed, so a
                // non-UTF-8 field spoils only its own row.
                Err(error) if matches!(error.kind(), csv::ErrorKind::Utf8 { .. }) => {
                    return Ok(Some(Err(RejectReason::MalformedRow)))
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
}

/// Turns one row into a transaction, or says why it cannot be one.
fn parse(record: &StringRecord, columns: Columns) -> Result<Transaction, RejectReason> {
    let field = |index: usize| record.get(index).unwrap_or_default();

    let client: ClientId = field(columns.client)
        .parse()
        .map_err(|_| RejectReason::MalformedRow)?;
    let tx = field(columns.tx)
        .parse()
        .map_err(|_| RejectReason::MalformedRow)?;

    let name = field(columns.kind);
    let kind = if name.eq_ignore_ascii_case("deposit") {
        Kind::Deposit(amount(record, columns)?)
    } else if name.eq_ignore_ascii_case("withdrawal") {
        Kind::Withdrawal(amount(record, columns)?)
    } else if name.eq_ignore_ascii_case("dispute") {
        Kind::Dispute
    } else if name.eq_ignore_ascii_case("resolve") {
        Kind::Resolve
    } else if name.eq_ignore_ascii_case("chargeback") {
        Kind::Chargeback
    } else {
        return Err(RejectReason::UnknownType);
    };

    Ok(Transaction { client, tx, kind })
}

/// Reads the amount a deposit or withdrawal must carry.
///
/// A row can omit it by being short or by leaving the field empty; both mean
/// the same thing to a transaction that requires one.
fn amount(record: &StringRecord, columns: Columns) -> Result<Money, RejectReason> {
    columns
        .amount
        .and_then(|index| record.get(index))
        .filter(|value| !value.is_empty())
        .ok_or(RejectReason::AmountShape)?
        .parse()
        .map_err(|_| RejectReason::AmountShape)
}

/// Writes accounts as CSV, ordered by client id.
///
/// Order is not significant to the format. Sorting makes runs byte-identical,
/// which is what the tests compare against.
pub fn write_accounts<W: io::Write>(
    sink: W,
    accounts: &HashMap<ClientId, Account>,
) -> Result<(), Error> {
    let mut rows: Vec<(&ClientId, &Account)> = accounts.iter().collect();
    rows.sort_unstable_by_key(|(client, _)| **client);

    let mut writer = Writer::from_writer(sink);
    writer.write_record(OUTPUT_HEADER)?;
    for (client, account) in rows {
        writer.write_record([
            client.to_string(),
            account.available.to_string(),
            account.held.to_string(),
            account.total().to_string(),
            account.locked.to_string(),
        ])?;
    }
    writer.flush().map_err(|error| Error::Csv(error.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collects every row of `input`, which must have a usable header.
    fn rows(input: &str) -> Vec<Result<Transaction, RejectReason>> {
        let mut reader = TransactionReader::new(input.as_bytes()).expect("header should be usable");
        let mut rows = Vec::new();
        while let Some(row) = reader.read().expect("input should be well-formed CSV") {
            rows.push(row);
        }
        rows
    }

    fn money(s: &str) -> Money {
        s.parse().expect("test amount should parse")
    }

    #[test]
    fn reads_the_documented_input_shape() {
        // The last row deliberately has no trailing newline.
        let rows = rows(concat!(
            "type, client, tx, amount\n",
            "deposit, 1, 1, 1.0\n",
            "withdrawal, 2, 5, 3.0",
        ));
        assert_eq!(
            rows,
            [
                Ok(Transaction {
                    client: 1,
                    tx: 1,
                    kind: Kind::Deposit(money("1.0")),
                }),
                Ok(Transaction {
                    client: 2,
                    tx: 5,
                    kind: Kind::Withdrawal(money("3.0")),
                }),
            ]
        );
    }

    #[test]
    fn accepts_rows_with_and_without_the_amount_field() {
        // The two shapes a dispute takes: three fields, or four with the last
        // one empty.
        let rows = rows(concat!(
            "type,client,tx,amount\n",
            "dispute,1,1\n",
            "resolve,1,1,\n",
            "chargeback, 1, 1,   \n",
        ));
        assert_eq!(
            rows,
            [
                Ok(Transaction {
                    client: 1,
                    tx: 1,
                    kind: Kind::Dispute
                }),
                Ok(Transaction {
                    client: 1,
                    tx: 1,
                    kind: Kind::Resolve
                }),
                Ok(Transaction {
                    client: 1,
                    tx: 1,
                    kind: Kind::Chargeback
                }),
            ]
        );
    }

    #[test]
    fn matches_transaction_types_regardless_of_case() {
        let rows = rows("type,client,tx,amount\nDeposit,1,1,1.0\nDISPUTE,1,1,\n");
        assert!(matches!(
            rows[0],
            Ok(Transaction {
                kind: Kind::Deposit(_),
                ..
            })
        ));
        assert!(matches!(
            rows[1],
            Ok(Transaction {
                kind: Kind::Dispute,
                ..
            })
        ));
    }

    #[test]
    fn reports_why_a_row_cannot_be_used() {
        let rows = rows(concat!(
            "type,client,tx,amount\n",
            "transfer,1,1,1.0\n",
            "deposit,1,1\n",
            "deposit,1,2,-1.0\n",
            "deposit,1,3,1.00005\n",
            "deposit,,4,1.0\n",
            "deposit,70000,5,1.0\n",
            "deposit,1,4294967296,1.0\n",
        ));
        assert_eq!(
            rows,
            [
                Err(RejectReason::UnknownType),
                Err(RejectReason::AmountShape),
                Err(RejectReason::AmountShape),
                Err(RejectReason::AmountShape),
                Err(RejectReason::MalformedRow),
                Err(RejectReason::MalformedRow),
                Err(RejectReason::MalformedRow),
            ]
        );
    }

    #[test]
    fn tolerates_reordered_columns_a_bom_and_crlf() {
        let rows = rows(concat!(
            "\u{feff}note,TX,client,type,amount\r\n",
            "ignored,1,1,deposit,1.0\r\n",
        ));
        assert_eq!(
            rows,
            [Ok(Transaction {
                client: 1,
                tx: 1,
                kind: Kind::Deposit(money("1.0")),
            })]
        );
    }

    #[test]
    fn skips_blank_lines_and_accepts_a_header_only_file() {
        assert!(rows("type,client,tx,amount\n").is_empty());
        assert_eq!(rows("type,client,tx,amount\n\n\ndispute,1,1\n").len(), 1);
    }

    #[test]
    fn non_utf8_spoils_only_its_own_row() {
        let input: &[u8] =
            b"type,client,tx,amount\ndeposit,1,1,1.0\ndeposit,1,\xff,2.0\ndeposit,1,3,3.0\n";
        let mut reader = TransactionReader::new(input).expect("header should be usable");

        let mut collected = Vec::new();
        while let Some(row) = reader.read().expect("only the row itself is spoiled") {
            collected.push(row);
        }

        assert_eq!(collected.len(), 3);
        assert!(collected[0].is_ok(), "{:?}", collected[0]);
        assert_eq!(collected[1], Err(RejectReason::MalformedRow));
        assert!(collected[2].is_ok(), "{:?}", collected[2]);
    }

    #[test]
    fn skips_lines_that_only_look_empty() {
        // Whitespace and bare separators are as empty as an empty line.
        assert!(rows("type,client,tx,amount\n   \n,,,\n\t\n").is_empty());
    }

    #[test]
    fn a_header_without_the_needed_columns_is_fatal() {
        let error = TransactionReader::new(&b"type,client,amount\n"[..]).unwrap_err();
        assert!(matches!(error, Error::MissingColumn("tx")), "{error}");
    }

    #[test]
    fn an_input_with_no_header_is_fatal() {
        // An empty file, and one whose first line is already data. The first
        // line is read as the header, so a missing column is the right error.
        for input in ["", "deposit,1,1,1.0\n"] {
            let error = TransactionReader::new(input.as_bytes()).unwrap_err();
            assert!(matches!(error, Error::MissingColumn("type")), "{error}");
        }
    }

    #[test]
    fn a_missing_amount_column_only_affects_rows_that_need_one() {
        let rows = rows("type,client,tx\ndispute,1,1\ndeposit,1,2\n");
        assert_eq!(
            rows,
            [
                Ok(Transaction {
                    client: 1,
                    tx: 1,
                    kind: Kind::Dispute
                }),
                Err(RejectReason::AmountShape),
            ]
        );
    }

    #[test]
    fn writes_accounts_sorted_by_client_at_four_decimal_places() {
        let accounts = HashMap::from([
            (
                2,
                Account {
                    available: money("2.0"),
                    held: Money::ZERO,
                    locked: false,
                },
            ),
            (
                1,
                Account {
                    available: Money::from_units(-100_000),
                    held: money("10.0"),
                    locked: true,
                },
            ),
        ]);

        let mut output = Vec::new();
        write_accounts(&mut output, &accounts).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            concat!(
                "client,available,held,total,locked\n",
                "1,-10.0000,10.0000,0.0000,true\n",
                "2,2.0000,0.0000,2.0000,false\n",
            )
        );
    }

    #[test]
    fn writes_a_header_even_with_no_accounts() {
        let mut output = Vec::new();
        write_accounts(&mut output, &HashMap::new()).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "client,available,held,total,locked\n"
        );
    }
}
