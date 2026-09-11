# AI assistant prompts

An AI assistant was used throughout this exercise, for planning, implementation, review and testing.

**These are five selected prompts, not the complete record.** They are reproduced as written, typos included, with the assistant's reply abridged. The full session transcript is available on request. The exercise brief is redacted where it was quoted, since the brief must not be committed.

---

## 1. 2026-09-10 18:14:45

*Setting the bar before any code was written.*

### Prompt

> List me all the principles a world top software engineer should follow, list all the critical ones please considering everything.
> 
> from my side I expect low cyclomatic complexity, low time complexity, low memory complexity, proper unit tests for advanced and basic cases (follow take home description where required). I prefer simpler code which achieves desired behavior, google staff engineer level stuff and google guidelines....
> 
> First we just plan don't code anything!
> 
> [exercise brief omitted: the brief must not be committed]

### Reply

**A. Principles (the ones that actually get graded)**
**Correctness first**
1. Make illegal states unrepresentable — encode invariants in types (newtypes, enums, private fields with invariant-preserving methods) so reviewers don't have to trust prose.
2. Parse, don't validate: convert untrusted CSV rows into a strict domain enum once, at the boundary; downstream code handles no malformed input.
3. Never use floating point for money. Fixed-point integer or a decimal crate, exact by construction.
4. Total functions: every arithmetic op that can overflow is explicitly checked; no silent wraparound, no `unwrap` on data-dependent paths.
5. Define the spec's gaps yourself, write them down, and be consistent. Ambiguity resolved silently is a defect.

**Failure handling**
6. Distinguish *fatal* (bad CLI args, unreadable file) from *per-record recoverable* (malformed row, unknown tx, insufficient funds). Fatal → exit non-zero with a message on stderr. Recoverable → skip, never poison the output on stdout.
7. Errors are values (`Result`, `thiserror`), never panics, never `process::exit` from library code. `main` is the only place that decides exit codes.
8. `#![forbid(unsafe_code)]`. If you'd need unsafe, you've picked the wrong design.

**Efficiency**
9. Stream, don't slurp — bounded per-record memory; the only growth is state you genuinely must retain (accounts, disputable txs).
10. Know your true memory bound and state it: worst case is O(#clients + #deposits), so make the retained per-tx record as small as possible.
11. Choose the cheapest data structure that meets the access pattern; O(1) lookups on the hot path; no allocation per row (reuse the record buffer, avoid `String` for the type tag).
12. Design for concurrency without implementing speculative machinery: the core engine takes an iterator of transactions and owns no I/O, so it drops into a per-stream task or a sharded-by-client actor unchanged. Say this in the README rather than building it.

**Maintainability**
13. Small modules with one reason to change; the state machine lives in one place, I/O at the edges (hexagonal: parse → engine → serialize).
14. Low cyclomatic complexity: exhaustive `match` over the transaction enum, early returns, no nested `if let` pyramids. If a function needs a comment to explain its branches, split it.
15. Names carry the domain (`available`, `held`, `dispute`), not the mechanism.
16. Comments explain *why* and document assumptions; the code explains *what*.
17. Public API documented with `///`; the README states assumptions, testing strategy, and safety reasoning — it's

*[reply abridged]*

---

## 2. 2026-09-11 05:56:13

*Review pass: minimal code, and a check for anything hacky.*

### Prompt

> Ok good.
> 
> All done?
> 
> Also code least amount of lines achieving intended behavior? As I don't want big code base. Want simple to understand one.
> 
> Also make sure any text or comment doesn't feel LLM like (long, unnecessary words, inefficient)
> 
> And find any logical error or issue or bug or hacky stuff. You proud of entire code?
> 
> 
> If a human looks at all the code files and structure etc, nothing stupid?

### Reply

One real bug found — a latent panic on a path I'd previously described as panic-free.

**The reject tally indexed a fixed array by enum discriminant.** `Ignored([u64; RejectReason::ALL.len()])` with `self.0[reason as usize] += 1`. The array length came from `ALL`, and the test only checked that `ALL`'s entries matched their own discriminants. Adding a twelfth variant and forgetting to extend `ALL` **compiles, passes every test, and then panics with an index-out-of-bounds** the first time that reason occurs — on a data-dependent path, which is exactly what I claimed the code doesn't have.

The fix removes the coupling instead of guarding it: the tally is now a `BTreeMap<RejectReason, u64>`, keyed by the reason itself. Derived `Ord` on a fieldless enum orders by discriminant, so the summary still prints in declaration order — byte-identical output, verified against the same fixture. Gone with it: the `ALL` constant (13 lines), the test that existed only to keep `ALL` in step (8 lines), and the index arithmetic. A new variant now needs no maintenance anywhere except its `Display` arm, which the compiler enforces.

**Net: 495 lines of code**, down from 511. One fewer test, because the deleted one was only checking an invariant that no longer exists.

**Am I proud of it?** Yes — more so than an hour ago, because that was a genuine defect and the fix made the code smaller rather than larger. The README's "nothing panics on input-dependent paths" is now true rather than aspirational.

Verified at `e0ee6f3`: 50/50 tests, fmt and clippy clean, tree clean, local only, no remotes.

---

## 3. 2026-09-11 06:45:40

*The complexity budget, restated.*

### Prompt

> You sure no tiny bug or hacky weird stuff in code? doing only whats required, low cyclomatic complexity, low kolmogorov complexity, most minimal code achieving the intended behavior

### Reply

One thing was genuinely dead. Found it by checking which public items are never called outside tests.

**`Total::units()` was unused in non-test code** and had exactly one test caller. Removed. Its caller — the total assertion in `assert_balances` — now compares `Total` to `Total` directly instead of unwrapping both sides to `i128`:

```rust
assert_eq!(account.total(), money(available).widening_add(money(held)), "total");
```

Shorter, and still a real check: the expectation comes from the test's own inputs, not from the account's fields. Re-ran the mutation check to prove it — breaking `total()` fails 11 of 15 engine tests.

`Money` keeps `units()` because its test helper genuinely uses it; `Total` doesn't need one. That asymmetry is the point: `Money` is a value type you construct and do arithmetic on, `Total` is a result type you only compare and print. It's now documented as such.

**On the two places that look duplicated, deliberately left alone:**

The three dispute-family arms in `apply` each repeat a four-line shape. Collapsing them into a table of `(required_state, reject_reason, account_method, next_state)` would save ~6 lines but needs a function-pointer tuple and a catch-all `_ =>` arm — which silently swallows any future variant instead of failing to compile. Lower Kolmogorov complexity, worse safety and worse reading. Three explicit arms is the right trade.

The five-branch `eq_ignore_ascii_case` chain in `parse` is flat, allocation-free, and has no nesting. A `match` on a lowercased string would allocate per row on the hot path.

**497 lines of code.** Verified at `fb02e79`: 51/51 tests, fmt and clippy clean, tree clean, local only.

---

## 4. 2026-09-11 11:09:52

*Pushing on edge cases. This one surfaced a parser bug.*

### Prompt

> Ok tests good, more advanced weird ones, no corner cases right? there cant be any fault

### Reply

Confirmed: `0.` and `0.0000` parse as zero, but `.0` and `.0000` are rejected — the same value accepted or refused depending on how it's written. Let me keep hunting at the CSV and engine layers.

Everything else is correct — including the withdrawal/deposit id collision and the negative-total chargeback (that's the fraud case from the problem statement, behaving right).

So one genuine fault: the parser contradicts its own documented rule. README says "anything four decimals hold exactly is accepted," but `.0` and `.0000` are refused while `0.` and `0.0000` are accepted. The cause is in [money.rs](src/money.rs) — the "has any digits" guard runs *after* trailing zeros are trimmed, so `.0000` collapses to the same empty state as a lone `.`.

You were right to push — I found one real fault.

**The bug:** `.0` and `.0000` were rejected while `0.` and `0.0000` were accepted. Same value, different spelling, different answer. That directly contradicted the README's stated rule ("anything four decimals hold exactly is accepted"). Cause: the "has any digits" guard ran *after* trailing zeros were trimmed, so `.0000` collapsed to the same empty state as a lone `.`. Fixed by checking before the trim — 11 lines including three new test assertions.

**What I probed that came back clean:**
- Duplicate `amount` columns, rows with extra fields, quoted fields
- Max `client` (65535) and max `tx` (4294967295)
- `i64::MAX` amount, and overflow on a second deposit (rejected, first preserved)
- Leading `+` on ids — accepted by Rust's `FromStr`, consistent on both the deposit and dispute side, so it matches correctly
- A withdrawal reusing a deposit's tx id, then disputed — resolves to the deposit, exactly as documented
- Chargeback driving **total** negative (−10.0000, locked) — the fraud scenario from the problem statement, correct
- `1.00005`, `.00001`, `-1`, `abc`, `.`, empty — all still rejected

CI green on the fix, 54 tests pass, fmt and clippy clean. Repo now at 10 commits: **https://github.com/hsdxpro/payments-engine**

---

## 5. 2026-09-11 11:19:46

*Asking why the bug got through, which changed the test strategy.*

### Prompt

> Why do we keep getting bugs? code base supposed to be good and such thing should never happen, refactor as needed

### Reply

Fair challenge. Let me answer it honestly rather than defensively.

**Why this one got through:** the parser is the only function in the codebase doing string manipulation with interacting guards, and it was tested with *hand-picked examples* rather than *invariants*. The existing property test (`parsing_is_total`) only asserted two things: parsing never panics, and a successful parse round-trips through display. Neither of those catches "the same value written two ways gives two different answers." The bug lived precisely in the gap between the prose rule in the doc comment and the imperative guard ordering that implemented it.

So the real defect wasn't the three lines — it was a missing property. Let me prove that by writing the property that would have caught it, then checking it actually fails against the old code.

The property passed against the buggy code — so it doesn't actually catch the bug. That's a more useful finding than a green check.

The reason: it only triggers when `whole` is empty *and* `frac` is all zeros *and* padding is non-zero. With uniform random digit strings that combination comes up roughly once per 300 cases, against proptest's default 256. It passed by luck, not by coverage.

Let me fix the generator to target the structural cases deliberately.

Direct answer to your question: **one** bug was found this session, not a stream of them. But your instinct is right that it shouldn't have survived, and the reason is worth stating plainly.

**Why it survived:** the parser was tested with hand-picked examples while every other module was tested with invariants. The engine has property tests saying *held is never negative* and *a rejected transaction moves no money* — statements about all possible inputs. The parser only had a list of strings someone thought of. `.0` wasn't on the list, so nothing caught that it disagreed with `0.`.

**I did not refactor the parser**, and that was deliberate. The implementation is correct now; rewriting working, tested arithmetic two days before submission is how you trade one small bug for a worse one. The defect was in the *test strategy*, so that's what I changed.

**What I added**, and the part that actually matters: a property asserting that padding an amount with insignificant zeros never changes its value. I then verified it properly — reverted the parser to the buggy version and confirmed the property **fails**, shrinking to exactly `.0` vs `0.0`, then restored the fix and confirmed it passes.

That verification caught a second problem. My first version of the property passed against the bu

*[reply abridged]*

---

