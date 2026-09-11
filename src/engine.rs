//! Client accounts and the transaction state machine.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use crate::model::{ClientId, Kind, RejectReason, Transaction, TxId};
use crate::money::{Money, Total};

/// Where a deposit stands in the dispute lifecycle.
///
/// Three states rather than a flag, so "charged back" cannot be mistaken for
/// "not currently disputed".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisputeState {
    /// Never disputed, or disputed and since resolved.
    Undisputed,
    /// Under dispute; its amount is part of the account's held funds.
    Disputed,
    /// Reversed. Unobservable while the account stays frozen, but marking it
    /// `Undisputed` would be wrong.
    ChargedBack,
}

/// A deposit, retained so a later dispute can find its amount.
///
/// Only deposits are disputable. Keeping withdrawals too would double the one
/// unbounded map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DepositRecord {
    amount: Money,
    client: ClientId,
    state: DisputeState,
}

/// A client's single asset account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// Funds the client can trade, stake or withdraw.
    pub available: Money,
    /// Funds withheld pending the outcome of a dispute.
    pub held: Money,
    /// Whether a chargeback has frozen the account.
    pub locked: bool,
}

/// Adds, reporting overflow as a rejection rather than an [`Option`].
fn add(left: Money, right: Money) -> Result<Money, RejectReason> {
    left.checked_add(right).ok_or(RejectReason::Overflow)
}

/// Subtracts, reporting overflow as a rejection rather than an [`Option`].
fn sub(left: Money, right: Money) -> Result<Money, RejectReason> {
    left.checked_sub(right).ok_or(RejectReason::Overflow)
}

impl Account {
    /// Returns `available + held`.
    #[must_use]
    pub fn total(&self) -> Total {
        self.available.widening_add(self.held)
    }

    /// Credits the account.
    fn deposit(&mut self, amount: Money) -> Result<(), RejectReason> {
        self.available = add(self.available, amount)?;
        Ok(())
    }

    /// Debits the account, if it holds enough available funds.
    fn withdraw(&mut self, amount: Money) -> Result<(), RejectReason> {
        if self.available < amount {
            return Err(RejectReason::InsufficientFunds);
        }
        self.available = sub(self.available, amount)?;
        Ok(())
    }

    /// Moves funds from available to held.
    ///
    /// Both balances are computed before either is stored, so a rejection
    /// cannot half-update the account. `available` may go negative: a client
    /// who withdrew a deposit and then disputed it owes the money back.
    fn hold(&mut self, amount: Money) -> Result<(), RejectReason> {
        let available = sub(self.available, amount)?;
        let held = add(self.held, amount)?;
        self.available = available;
        self.held = held;
        Ok(())
    }

    /// Moves funds from held back to available.
    fn release(&mut self, amount: Money) -> Result<(), RejectReason> {
        let available = add(self.available, amount)?;
        let held = sub(self.held, amount)?;
        self.available = available;
        self.held = held;
        Ok(())
    }

    /// Removes held funds from the account and freezes it.
    fn reverse(&mut self, amount: Money) -> Result<(), RejectReason> {
        self.held = sub(self.held, amount)?;
        self.locked = true;
        Ok(())
    }
}

/// Applies transactions to client accounts.
///
/// Takes one [`Transaction`] at a time and owns no I/O, so it works the same on
/// a file or a socket. Memory is one entry per client, at most 65,536 of them,
/// plus one record per deposit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Engine {
    accounts: HashMap<ClientId, Account>,
    deposits: HashMap<TxId, DepositRecord>,
}

impl Engine {
    /// Creates an engine with no accounts.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the accounts touched so far.
    #[must_use]
    pub fn accounts(&self) -> &HashMap<ClientId, Account> {
        &self.accounts
    }

    /// Applies one transaction, or reports why it was ignored.
    ///
    /// A rejection moves no money, but still creates the client record it
    /// names, as the specification asks.
    pub fn apply(&mut self, transaction: Transaction) -> Result<(), RejectReason> {
        let Transaction { client, tx, kind } = transaction;
        let account = self.accounts.entry(client).or_default();
        if account.locked {
            return Err(RejectReason::AccountLocked);
        }

        match kind {
            Kind::Deposit(amount) => match self.deposits.entry(tx) {
                // Ids are unique. A repeat could rewrite the amount of a
                // deposit that is already under dispute.
                Entry::Occupied(_) => Err(RejectReason::DuplicateTx),
                Entry::Vacant(slot) => {
                    account.deposit(amount)?;
                    slot.insert(DepositRecord {
                        amount,
                        client,
                        state: DisputeState::Undisputed,
                    });
                    Ok(())
                }
            },
            Kind::Withdrawal(amount) => account.withdraw(amount),
            Kind::Dispute => {
                let record = find_deposit(&mut self.deposits, client, tx)?;
                if record.state != DisputeState::Undisputed {
                    return Err(RejectReason::TxNotDisputable);
                }
                account.hold(record.amount)?;
                record.state = DisputeState::Disputed;
                Ok(())
            }
            Kind::Resolve => {
                let record = find_deposit(&mut self.deposits, client, tx)?;
                if record.state != DisputeState::Disputed {
                    return Err(RejectReason::TxNotDisputed);
                }
                account.release(record.amount)?;
                record.state = DisputeState::Undisputed;
                Ok(())
            }
            Kind::Chargeback => {
                let record = find_deposit(&mut self.deposits, client, tx)?;
                if record.state != DisputeState::Disputed {
                    return Err(RejectReason::TxNotDisputed);
                }
                account.reverse(record.amount)?;
                record.state = DisputeState::ChargedBack;
                Ok(())
            }
        }
    }
}

/// Looks up the deposit a dispute, resolve or chargeback refers to.
///
/// The row's client must own it, or a partner could move funds on any account
/// by quoting someone else's id. This also lets clients be processed
/// independently.
fn find_deposit(
    deposits: &mut HashMap<TxId, DepositRecord>,
    client: ClientId,
    tx: TxId,
) -> Result<&mut DepositRecord, RejectReason> {
    let record = deposits.get_mut(&tx).ok_or(RejectReason::UnknownTx)?;
    if record.client == client {
        Ok(record)
    } else {
        Err(RejectReason::ClientMismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: ClientId = 7;

    /// Parses a test amount, allowing a leading `-` that the input format does
    /// not.
    fn money(s: &str) -> Money {
        let (sign, magnitude) = match s.strip_prefix('-') {
            Some(rest) => (-1, rest),
            None => (1, s),
        };
        let units = magnitude
            .parse::<Money>()
            .expect("test amount should parse")
            .units();
        Money::from_units(sign * units)
    }

    /// Applies `kind` for [`CLIENT`], to keep test bodies short.
    fn tx(engine: &mut Engine, tx: TxId, kind: Kind) -> Result<(), RejectReason> {
        engine.apply(Transaction {
            client: CLIENT,
            tx,
            kind,
        })
    }

    fn deposit(engine: &mut Engine, id: TxId, amount: &str) -> Result<(), RejectReason> {
        tx(engine, id, Kind::Deposit(money(amount)))
    }

    fn withdraw(engine: &mut Engine, id: TxId, amount: &str) -> Result<(), RejectReason> {
        tx(engine, id, Kind::Withdrawal(money(amount)))
    }

    #[track_caller]
    fn assert_rejects(outcome: Result<(), RejectReason>, reason: RejectReason) {
        assert_eq!(outcome, Err(reason));
    }

    #[track_caller]
    fn assert_balances(engine: &Engine, available: &str, held: &str, locked: bool) {
        let account = engine.accounts()[&CLIENT];
        assert_eq!(account.available, money(available), "available");
        assert_eq!(account.held, money(held), "held");
        assert_eq!(account.locked, locked, "locked");
        // Against the values this call expects. Comparing `total()` to the
        // fields it is computed from would be an assertion that cannot fail.
        assert_eq!(
            account.total(),
            money(available).widening_add(money(held)),
            "total"
        );
    }

    #[test]
    fn deposits_and_withdrawals_move_available_funds() {
        let mut engine = Engine::new();
        assert_eq!(deposit(&mut engine, 1, "1.0"), Ok(()));
        assert_eq!(deposit(&mut engine, 2, "2.0"), Ok(()));
        assert_eq!(withdraw(&mut engine, 3, "1.5"), Ok(()));
        assert_balances(&engine, "1.5", "0", false);
    }

    #[test]
    fn withdrawal_beyond_available_funds_is_ignored() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "1.0").unwrap();
        assert_rejects(
            withdraw(&mut engine, 2, "1.0001"),
            RejectReason::InsufficientFunds,
        );
        assert_balances(&engine, "1.0", "0", false);
    }

    #[test]
    fn an_ignored_transaction_still_creates_the_client() {
        let mut engine = Engine::new();
        assert_rejects(
            withdraw(&mut engine, 1, "1.0"),
            RejectReason::InsufficientFunds,
        );
        assert_balances(&engine, "0", "0", false);
    }

    #[test]
    fn dispute_holds_funds_without_changing_the_total() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        assert_eq!(tx(&mut engine, 1, Kind::Dispute), Ok(()));
        assert_balances(&engine, "0", "10.0", false);
    }

    #[test]
    fn dispute_after_the_funds_left_drives_available_negative() {
        // Deposit, move the money out, then reverse the deposit.
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        withdraw(&mut engine, 2, "10.0").unwrap();
        assert_eq!(tx(&mut engine, 1, Kind::Dispute), Ok(()));
        assert_balances(&engine, "-10.0", "10.0", false);

        assert_eq!(tx(&mut engine, 1, Kind::Chargeback), Ok(()));
        assert_balances(&engine, "-10.0", "0", true);
    }

    #[test]
    fn resolve_returns_held_funds() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        tx(&mut engine, 1, Kind::Dispute).unwrap();
        assert_eq!(tx(&mut engine, 1, Kind::Resolve), Ok(()));
        assert_balances(&engine, "10.0", "0", false);
    }

    #[test]
    fn chargeback_removes_held_funds_and_freezes_the_account() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        tx(&mut engine, 1, Kind::Dispute).unwrap();
        assert_eq!(tx(&mut engine, 1, Kind::Chargeback), Ok(()));
        assert_balances(&engine, "0", "0", true);
    }

    #[test]
    fn a_frozen_account_ignores_everything_afterwards() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        deposit(&mut engine, 2, "5.0").unwrap();
        tx(&mut engine, 1, Kind::Dispute).unwrap();
        tx(&mut engine, 1, Kind::Chargeback).unwrap();

        for attempt in [
            deposit(&mut engine, 3, "1.0"),
            withdraw(&mut engine, 4, "1.0"),
            tx(&mut engine, 2, Kind::Dispute),
            tx(&mut engine, 2, Kind::Resolve),
            tx(&mut engine, 2, Kind::Chargeback),
        ] {
            assert_rejects(attempt, RejectReason::AccountLocked);
        }
        assert_balances(&engine, "5.0", "0", true);
    }

    #[test]
    fn a_chargeback_leaves_other_disputes_held() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        deposit(&mut engine, 2, "4.0").unwrap();
        tx(&mut engine, 1, Kind::Dispute).unwrap();
        tx(&mut engine, 2, Kind::Dispute).unwrap();

        tx(&mut engine, 1, Kind::Chargeback).unwrap();

        // Freezing does not release the surviving dispute.
        assert_balances(&engine, "0", "4.0", true);
    }

    #[test]
    fn a_deposit_can_be_disputed_again_after_being_resolved() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        tx(&mut engine, 1, Kind::Dispute).unwrap();
        tx(&mut engine, 1, Kind::Resolve).unwrap();
        assert_eq!(tx(&mut engine, 1, Kind::Dispute), Ok(()));
        assert_eq!(tx(&mut engine, 1, Kind::Chargeback), Ok(()));
        assert_balances(&engine, "0", "0", true);
    }

    #[test]
    fn a_dispute_needs_an_undisputed_deposit() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();

        assert_rejects(tx(&mut engine, 99, Kind::Dispute), RejectReason::UnknownTx);
        assert_rejects(
            engine.apply(Transaction {
                client: CLIENT + 1,
                tx: 1,
                kind: Kind::Dispute,
            }),
            RejectReason::ClientMismatch,
        );

        tx(&mut engine, 1, Kind::Dispute).unwrap();
        assert_rejects(
            tx(&mut engine, 1, Kind::Dispute),
            RejectReason::TxNotDisputable,
        );
        assert_balances(&engine, "0", "10.0", false);
    }

    #[test]
    fn a_withdrawal_is_not_disputable() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        withdraw(&mut engine, 2, "4.0").unwrap();
        assert_rejects(tx(&mut engine, 2, Kind::Dispute), RejectReason::UnknownTx);
        assert_balances(&engine, "6.0", "0", false);
    }

    #[test]
    fn resolve_and_chargeback_need_an_open_dispute() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();

        assert_rejects(
            tx(&mut engine, 1, Kind::Resolve),
            RejectReason::TxNotDisputed,
        );
        assert_rejects(
            tx(&mut engine, 1, Kind::Chargeback),
            RejectReason::TxNotDisputed,
        );
        assert_rejects(tx(&mut engine, 9, Kind::Resolve), RejectReason::UnknownTx);
        assert_balances(&engine, "10.0", "0", false);
    }

    #[test]
    fn a_repeated_transaction_id_cannot_rewrite_a_deposit() {
        let mut engine = Engine::new();
        deposit(&mut engine, 1, "10.0").unwrap();
        assert_rejects(deposit(&mut engine, 1, "999.0"), RejectReason::DuplicateTx);
        assert_balances(&engine, "10.0", "0", false);

        // The original amount is held, not the replacement.
        tx(&mut engine, 1, Kind::Dispute).unwrap();
        assert_balances(&engine, "0", "10.0", false);
    }

    /// Deposits the largest representable amount, to set up an overflow.
    fn deposit_max(engine: &mut Engine, id: TxId) {
        tx(engine, id, Kind::Deposit(Money::from_units(i64::MAX))).unwrap();
    }

    #[test]
    fn an_overflowing_deposit_is_rejected_and_changes_nothing() {
        let mut engine = Engine::new();
        deposit_max(&mut engine, 1);
        let before = engine.clone();

        assert_rejects(deposit(&mut engine, 2, "0.0001"), RejectReason::Overflow);
        assert_eq!(engine, before);
    }

    #[test]
    fn an_overflowing_resolve_leaves_both_balances_alone() {
        // A resolve moves two balances at once. If it wrote the first before
        // discovering the second overflows, the account would be left claiming
        // funds it does not have.
        let mut engine = Engine::new();
        deposit_max(&mut engine, 1);
        tx(&mut engine, 1, Kind::Dispute).unwrap();
        deposit_max(&mut engine, 2);
        let before = engine.clone();

        assert_rejects(tx(&mut engine, 1, Kind::Resolve), RejectReason::Overflow);
        assert_eq!(engine, before);
    }
}
