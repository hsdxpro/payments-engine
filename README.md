# payments-engine

[![CI](https://github.com/hsdxpro/payments-engine/actions/workflows/ci.yml/badge.svg)](https://github.com/hsdxpro/payments-engine/actions/workflows/ci.yml)

Streams transactions from a CSV file, applies them to client accounts, and
writes the balances to stdout as CSV.

```sh
cargo build --release                          # build
cargo run -- transactions.csv > accounts.csv   # run
cargo test                                     # 58 tests
```

The input file is the first argument. Balances go to stdout, everything else to
stderr. To try it: `cargo run -- tests/data/disputes.csv`.

```text
type, client, tx, amount        client,available,held,total,locked
deposit, 1, 1, 1.0              1,1.5000,0.0000,1.5000,false
deposit, 2, 2, 2.0        =>    2,2.0000,0.0000,2.0000,false
deposit, 1, 3, 2.0
withdrawal, 1, 4, 1.5
withdrawal, 2, 5, 3.0
```

## Layout

| File | Contents |
| --- | --- |
| `src/money.rs` | `Money`, an exact fixed-point amount |
| `src/model.rs` | `Transaction` and `RejectReason` |
| `src/engine.rs` | `Account`, deposit records, the state machine |
| `src/csv_io.rs` | Both CSV boundaries |
| `src/main.rs` | Arguments, wiring, exit status, stderr summary |

About 470 lines of code. The engine owns no I/O and the CSV layer knows no
account rules, so each can be tested alone.

## Money

Amounts are `i64` counts of ten-thousandths. The four decimals the input allows
are the four the output prints, and the type holds the scale, so no operation
has to preserve it. Every operation is checked, so debug and release behave
identically. `i64` covers ±922,337,203,685,477.5807; beyond that a
transaction is rejected, never wrapped.

`total` is derived from `available + held` rather than stored, so the three
cannot disagree, and it is derived in `i128` so the addition cannot overflow.

Amounts parse as `digit* ('.' digit*)?` with at least one digit. Anything four
decimals hold exactly is accepted, anything else refused rather than rounded:
`1.`, `.5` and `1.50000` parse, `1.00005` does not. Signs and exponents are
rejected.

## Transaction rules

What the specification requires, plus the assumptions it does not cover. Where
it says nothing, the rule is what a bank would do.

| Situation | Behaviour | Why |
| --- | --- | --- |
| Deposit | Credits available funds | Specified |
| Withdrawal over available funds | Ignored | Specified |
| Dispute | Moves the deposit's amount from available to held | Specified |
| Resolve | Moves it back | Specified |
| Chargeback | Removes held funds, freezes the account | Specified |
| Dispute of a withdrawal | Ignored | The specified rule (available down, held up) would remove the same funds twice. Only deposits are disputable, so only deposits are stored |
| Dispute naming a different client | Ignored | Otherwise a partner could move funds on any account by quoting someone else's id |
| Dispute of a disputed or charged-back deposit | Ignored | Unspecified. A deposit can be disputed and resolved repeatedly, charged back once |
| Dispute after a resolve | Allowed | The deposit exists and is not under dispute, which is all a dispute needs |
| Any transaction on a frozen account | Ignored | "Immediately frozen" reads as frozen to everything |
| Repeated transaction id | Ignored | Ids are unique. A repeat could rewrite the amount of a deposit already under dispute |
| Amount of zero | Accepted | Rejecting it would be an invented rule |
| Unknown client | Created, even if the transaction is then ignored | The specification creates a record on first reference. A row that fails to parse creates nothing, since its client id is not trustworthy |

Available funds can go negative: deposit, withdraw the proceeds, dispute the
deposit, and a chargeback takes the total negative too. This is the attack the
specification describes, and the negative balance is what the client owes.

## Correctness

58 tests, run with `cargo test`.

- **Unit tests beside the code.** `money.rs` covers the grammar, four-decimal
  output, checked arithmetic, and the two formatting traps of fixed point:
  `-0.0500` must not print as `0.0500`, and `i64::MIN` must not be negated.
  `engine.rs` covers every transition and rejection, including double disputes,
  disputes after a resolve, a chargeback leaving the client's other dispute
  held, a repeated id failing to overwrite a deposit, and an overflowing resolve
  leaving both balances untouched. `csv_io.rs` covers whitespace, both shapes of
  a row with no amount, reordered and extra columns, a byte-order mark, CRLF,
  blank and whitespace-only lines, non-UTF-8 fields, and a missing column.
- **Property tests** (`tests/properties.rs`). Display and parse round-trip over
  the whole range; the parser is fuzzed over digits, dots, signs and exponents
  to confirm it is total, and padding an amount with insignificant zeros is
  checked not to change what it parses to. The reader survives arbitrary bytes,
  and a row's meaning is checked to follow the column names rather than the
  column order or the whitespace around a field. Over random transaction
  sequences: held funds are never negative, a rejected transaction moves no
  money (balances move in pairs, and both are computed before either is
  stored), and a dispute followed by a resolve restores the account exactly.
- **End-to-end tests** (`tests/cli.rs`) run the binary as documented and compare
  stdout against committed files in `tests/data`.
- **A second implementation.** A throwaway reference written from the
  specification was diffed against the engine over 350,000 generated and
  adversarial transactions, byte for byte. `tests/data/adversarial.csv` is a
  1,200-row sample; its expected output is the reference's, not the engine's.
- **The type system.** Only `Kind::Deposit` and `Kind::Withdrawal` carry an
  amount, so the engine cannot ask a dispute for one. A deposit's dispute state
  is a three-state enum, not a flag, so "charged back" cannot be mistaken for
  "not currently disputed".

## Errors

A declined row is a normal outcome, not a failure: the input is a partner feed
and the specification asks for erroneous rows to be ignored. They are counted by
reason and summarised on stderr at the end, which avoids a log line per row on a
large input. The exit status stays zero.

```text
ignored 7 row(s):
           1  unknown transaction type
           2  missing or invalid amount
           1  insufficient funds
           1  duplicate transaction id
           1  transaction belongs to another client
           1  transaction is not under dispute
```

That is `cargo run -- tests/data/messy.csv`, a fixture of things a partner can
get wrong.

A failure that invalidates the run exits non-zero: no argument, an unreadable
file, or a header missing a column. An I/O failure is fatal rather than per-row
because it says nothing about where the stream resumes. A non-UTF-8 field is the
opposite case: record boundaries are found by byte, so it spoils only its own
row, which is skipped and counted.

Nothing panics on input-dependent paths, there is no `unsafe` (`#![forbid]`),
and no arithmetic is unchecked.

## Efficiency

Rows stream through one reused buffer: nothing is loaded up front, and there is
no allocation per row on the steady-state path.

Measured on a 10,000,000-row, 304 MB generated file (release build, Windows):

| | |
| --- | --- |
| Wall clock | 7.9-8.4 s over three runs, about 1.2 M rows/s |
| Peak resident memory | 310 MB |

```sh
cargo run --release --example gen_input -- 10000000 > large.csv
cargo run --release -- large.csv > accounts.csv
```

Memory is one entry per client, bounded at 65,536 accounts, plus a 16-byte
record per deposit, about 48 bytes each with hash table slack. A saturated `u32`
id space would need single-digit gigabytes. That is inherent to letting any past
deposit be disputed at any time; a real system would bound it with a dispute
window and drop settled transactions. The specification has no such window, so
nothing is dropped here.

Only deposits are stored. Withdrawals are never referenced again, so keeping
them would double the unbounded map for nothing.

## If this were a server

Concurrency should shard by client, not by stream. Streams are not
client-disjoint, so per-stream engines would hold partial state for the same
client and could not be merged: a dispute in one stream against a deposit in
another would have nothing to look up. With a router dispatching by client id,
each shard owns its clients' accounts and their deposit records, and stays
self-contained because a dispute must name the client that owns the transaction
— the same rule that stops the cross-account attack. Backpressure would be a
bounded channel per shard.

Ordering is only meaningful per client. "Transactions occur chronologically in
the file" has no global analogue across thousands of streams and needs none,
since sharding by client preserves the ordering that matters.

## Trade-offs

**One dependency, `csv`.** Input rows are parsed by hand from a `StringRecord`
rather than through serde: the crate's `deserialize_any` infers a float from
anything float-shaped, losing precision before a decimal type sees it, and a
`dispute, 1, 1` row is shorter than the header, which deserialization handles
poorly. A column map read from the header handles both in about thirty lines.
On the output side serde would have wrapped five `write_record` fields and
nothing else. (`csv` uses serde internally; this crate does not.)

**Output is always four decimals**, `1.5000` rather than `1.5`, which needs no
rounding step.

**Rows are sorted by client id.** Order is not significant to the format, but
sorting makes runs byte-identical for the tests.

**A charged-back deposit's state is unobservable**, since the account is frozen.
The third state is kept because marking the record undisputed would be untrue.

**Duplicate id detection covers deposits only.** They are the only records
stored; catching a withdrawal that reuses a deposit's id would mean storing
every withdrawal.
