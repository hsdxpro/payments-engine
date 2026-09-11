//! The validated form of an input row, and the reasons a row can be ignored.

use std::fmt;

use crate::money::Money;

/// Identifies a client account.
pub type ClientId = u16;

/// Identifies a transaction. Globally unique across all clients.
pub type TxId = u32;

/// A transaction, after validation.
///
/// The amount lives in [`Kind`] because only two kinds have one, so the engine
/// cannot ask a dispute for an amount it lacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transaction {
    /// The account the transaction applies to.
    pub client: ClientId,
    /// The transaction this row creates, or refers back to.
    pub tx: TxId,
    /// What the transaction does.
    pub kind: Kind,
}

/// What a [`Transaction`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Credits the account.
    Deposit(Money),
    /// Debits the account, if the funds are available.
    Withdrawal(Money),
    /// Claims a past deposit was erroneous and holds its funds.
    Dispute,
    /// Ends a dispute, releasing the held funds back to the client.
    Resolve,
    /// Ends a dispute by reversing the deposit and freezing the account.
    Chargeback,
}

/// Why a row was ignored.
///
/// A declined row is a normal outcome, not a failure of this program. One
/// variant per thing a partner would have to fix. Ordered by declaration, which
/// is the order the end-of-run summary prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RejectReason {
    /// A client or transaction id was missing or not a valid integer.
    MalformedRow,
    /// The transaction type was not one of the five known types.
    UnknownType,
    /// A deposit or withdrawal had a missing or unrepresentable amount.
    AmountShape,
    /// The account is frozen after a chargeback and accepts nothing further.
    AccountLocked,
    /// The withdrawal exceeded the available funds.
    InsufficientFunds,
    /// The transaction id has already been used by an earlier deposit.
    DuplicateTx,
    /// No deposit exists with the referenced transaction id.
    UnknownTx,
    /// The referenced deposit belongs to a different client.
    ClientMismatch,
    /// The referenced deposit is already disputed, or already charged back.
    TxNotDisputable,
    /// The referenced deposit is not currently under dispute.
    TxNotDisputed,
    /// Applying the transaction would have overflowed a balance.
    Overflow,
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MalformedRow => "malformed row",
            Self::UnknownType => "unknown transaction type",
            Self::AmountShape => "missing or invalid amount",
            Self::AccountLocked => "account locked",
            Self::InsufficientFunds => "insufficient funds",
            Self::DuplicateTx => "duplicate transaction id",
            Self::UnknownTx => "unknown transaction id",
            Self::ClientMismatch => "transaction belongs to another client",
            Self::TxNotDisputable => "transaction is not disputable",
            Self::TxNotDisputed => "transaction is not under dispute",
            Self::Overflow => "balance would overflow",
        })
    }
}
