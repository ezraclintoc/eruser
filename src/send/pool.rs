//! Spreading a run across several sending accounts.
//!
//! Gmail stops accepting around 500 messages a day. A 764-broker run through
//! one mailbox therefore takes two days; through three it takes one pass.
//! The pool holds every account a person may send through, hands out the
//! next one with allowance left, and rolls over when one is spent.

use std::sync::Arc;

use crate::email::{self, Sender};
use crate::history::AccountCapacity;

/// One account, ready to send, with what is left of its allowance today.
pub struct PoolEntry {
    /// The account row this came from. `None` for a one-off sender that is
    /// not backed by a stored account, such as a dry run.
    pub account_id: Option<i64>,
    /// What to call it in progress output.
    pub label: String,
    /// The address messages go out from.
    pub from: String,
    pub sender: Arc<dyn Sender>,
    remaining: usize,
}

impl std::fmt::Debug for PoolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolEntry")
            .field("account_id", &self.account_id)
            .field("from", &self.from)
            .field("remaining", &self.remaining)
            .finish()
    }
}

impl PoolEntry {
    pub fn remaining(&self) -> usize {
        self.remaining
    }
}

/// The accounts a run may send through, in the order it will use them.
#[derive(Debug, Default)]
pub struct SenderPool {
    entries: Vec<PoolEntry>,
    /// Where the last send came from, so a run stays on one account until
    /// it is spent rather than alternating between them.
    cursor: usize,
}

impl SenderPool {
    /// A pool of exactly one sender with no per-account cap.
    ///
    /// Used for a dry run, for sending to a single broker, and by anything
    /// that has a transport but no stored account behind it.
    pub fn single(sender: Arc<dyn Sender>, from: String) -> Self {
        Self {
            entries: vec![PoolEntry {
                account_id: None,
                label: from.clone(),
                from,
                sender,
                remaining: usize::MAX,
            }],
            cursor: 0,
        }
    }

    /// Build a pool from what each account has left today.
    ///
    /// An account that is disabled, spent, or cannot be turned into a working
    /// transport is left out with a warning rather than failing the run: one
    /// mailbox with a stale password should not stop the other two.
    pub fn from_capacity(capacity: &[AccountCapacity]) -> Self {
        let mut entries = Vec::new();

        for available in capacity.iter().filter(|c| c.is_available()) {
            let account = &available.account;

            match email::new_sender(&account.email_config()) {
                Ok(sender) => entries.push(PoolEntry {
                    account_id: Some(account.id),
                    label: account.display_name(),
                    from: account.from_address.clone(),
                    sender: Arc::from(sender),
                    remaining: available.remaining.max(0) as usize,
                }),
                Err(error) => tracing::warn!(
                    account = %account.display_name(),
                    %error,
                    "skipping an account that cannot send"
                ),
            }
        }

        Self { entries, cursor: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// How many more messages this pool can send today.
    pub fn remaining(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.remaining)
            .fold(0usize, usize::saturating_add)
    }

    /// The accounts in the pool, for reporting.
    pub fn entries(&self) -> &[PoolEntry] {
        &self.entries
    }

    /// Take one send's worth of allowance from the next usable account.
    ///
    /// Returns what to send with, or `None` when every account is spent.
    /// The allowance is taken before the send rather than after it: a
    /// message that fails has usually still been counted by the provider,
    /// and over-counting risks a delay where under-counting risks the
    /// account being blocked.
    pub fn take(&mut self) -> Option<Reservation> {
        // Stay on the current account while it has room, so a run does not
        // alternate between mailboxes for no reason.
        let index = (self.cursor..self.entries.len())
            .chain(0..self.cursor)
            .find(|index| self.entries[*index].remaining > 0)?;

        self.cursor = index;
        let entry = &mut self.entries[index];
        entry.remaining -= 1;

        Some(Reservation {
            account_id: entry.account_id,
            label: entry.label.clone(),
            from: entry.from.clone(),
            sender: entry.sender.clone(),
        })
    }
}

/// One send's worth of a particular account.
#[derive(Clone)]
pub struct Reservation {
    pub account_id: Option<i64>,
    pub label: String,
    pub from: String,
    pub sender: Arc<dyn Sender>,
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation")
            .field("account_id", &self.account_id)
            .field("from", &self.from)
            .finish()
    }
}

#[cfg(test)]
mod tests;
