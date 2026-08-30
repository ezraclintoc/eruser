use super::*;

use crate::email::tests::RecordingSender;
use crate::history::{AccountCapacity, AccountScope, SenderAccount};

fn account(id: i64, address: &str, provider: &str) -> SenderAccount {
    SenderAccount {
        id,
        user_id: 1,
        label: address.split('@').next().unwrap_or_default().to_string(),
        scope: AccountScope::Personal,
        provider: provider.to_string(),
        from_address: address.to_string(),
        smtp: crate::config::SmtpConfig {
            host: "smtp.example.com".into(),
            port: 465,
            username: address.to_string(),
            password: "app-password".into(),
            use_tls: true,
        },
        api_key: "re_abc".into(),
        daily_limit: 250,
        enabled: true,
        priority: 0,
        created_at: None,
    }
}

fn capacity(account: SenderAccount, remaining: i64) -> AccountCapacity {
    AccountCapacity {
        sent_today: account.daily_limit - remaining,
        remaining,
        account,
    }
}

fn recording() -> Arc<dyn Sender> {
    Arc::new(RecordingSender::default())
}

// -------------------------------------------------------------------
// A single sender
// -------------------------------------------------------------------

#[test]
fn a_single_sender_has_no_cap_of_its_own() {
    let mut pool = SenderPool::single(recording(), "jane@example.com".into());

    assert_eq!(pool.len(), 1);
    for _ in 0..1000 {
        let reservation = pool.take().expect("a single sender should not run out");
        assert_eq!(reservation.from, "jane@example.com");
        assert!(reservation.account_id.is_none());
    }
}

// -------------------------------------------------------------------
// Building from stored accounts
// -------------------------------------------------------------------

#[tokio::test]
async fn a_pool_is_built_from_the_accounts_with_allowance_left() {
    let pool = SenderPool::from_capacity(&[
        capacity(account(1, "one@gmail.com", "smtp"), 100),
        capacity(account(2, "two@gmail.com", "smtp"), 50),
    ]);

    assert_eq!(pool.len(), 2);
    assert_eq!(pool.remaining(), 150);
}

#[tokio::test]
async fn a_spent_account_is_left_out() {
    let mut pool = SenderPool::from_capacity(&[
        capacity(account(1, "spent@gmail.com", "smtp"), 0),
        capacity(account(2, "fresh@gmail.com", "smtp"), 40),
    ]);

    assert_eq!(pool.len(), 1);
    assert_eq!(pool.take().unwrap().from, "fresh@gmail.com");
}

#[tokio::test]
async fn a_disabled_account_is_left_out() {
    let disabled = SenderAccount {
        enabled: false,
        ..account(1, "off@gmail.com", "smtp")
    };

    let pool = SenderPool::from_capacity(&[capacity(disabled, 100)]);
    assert!(pool.is_empty());
}

/// One mailbox with a stale password should not stop the other two.
#[tokio::test]
async fn an_account_that_cannot_send_is_skipped_rather_than_failing_the_run() {
    let broken = SenderAccount {
        // A key-based provider with no key cannot build a transport.
        provider: "resend".into(),
        api_key: String::new(),
        ..account(1, "broken@example.com", "resend")
    };

    let mut pool = SenderPool::from_capacity(&[
        capacity(broken, 100),
        capacity(account(2, "working@gmail.com", "smtp"), 100),
    ]);

    assert_eq!(pool.len(), 1, "the working account should still be there");
    assert_eq!(pool.take().unwrap().from, "working@gmail.com");
}

#[tokio::test]
async fn a_pool_with_nothing_usable_is_empty() {
    let pool = SenderPool::from_capacity(&[capacity(account(1, "spent@gmail.com", "smtp"), 0)]);

    assert!(pool.is_empty());
    assert_eq!(pool.remaining(), 0);
}

// -------------------------------------------------------------------
// Rolling over
// -------------------------------------------------------------------

/// The reason the pool exists: Gmail stops at around 500 a day, so a
/// 764-broker run has to continue on the next account rather than stopping.
#[tokio::test]
async fn a_run_rolls_over_to_the_next_account_when_one_is_spent() {
    let mut pool = SenderPool::from_capacity(&[
        capacity(account(1, "one@gmail.com", "smtp"), 2),
        capacity(account(2, "two@gmail.com", "smtp"), 3),
    ]);

    let used: Vec<String> = (0..5)
        .map(|_| pool.take().expect("there is allowance left").from)
        .collect();

    assert_eq!(
        used,
        [
            "one@gmail.com",
            "one@gmail.com",
            "two@gmail.com",
            "two@gmail.com",
            "two@gmail.com",
        ]
    );
    assert!(pool.take().is_none(), "everything is spent");
}

/// A run should stay on one mailbox until it is spent, not alternate between
/// them for no reason.
#[tokio::test]
async fn a_run_stays_on_one_account_while_it_has_room() {
    let mut pool = SenderPool::from_capacity(&[
        capacity(account(1, "one@gmail.com", "smtp"), 10),
        capacity(account(2, "two@gmail.com", "smtp"), 10),
    ]);

    for _ in 0..5 {
        assert_eq!(pool.take().unwrap().from, "one@gmail.com");
    }
}

#[tokio::test]
async fn taking_reduces_what_is_left() {
    let mut pool = SenderPool::from_capacity(&[capacity(account(1, "one@gmail.com", "smtp"), 3)]);

    assert_eq!(pool.remaining(), 3);
    pool.take();
    assert_eq!(pool.remaining(), 2);
    pool.take();
    pool.take();
    assert_eq!(pool.remaining(), 0);
    assert!(pool.take().is_none());
}

#[tokio::test]
async fn each_reservation_carries_the_account_that_will_send_it() {
    let mut pool = SenderPool::from_capacity(&[
        capacity(account(7, "seven@gmail.com", "smtp"), 1),
        capacity(account(9, "nine@gmail.com", "smtp"), 1),
    ]);

    let first = pool.take().unwrap();
    assert_eq!(first.account_id, Some(7));
    assert_eq!(first.from, "seven@gmail.com");

    let second = pool.take().unwrap();
    assert_eq!(second.account_id, Some(9));
    assert_eq!(second.from, "nine@gmail.com");
}

/// Accounts come out of the store already ordered by priority, and the pool
/// must not reorder them: a free mailbox should be spent before a paid one.
#[tokio::test]
async fn the_order_the_accounts_arrive_in_is_the_order_they_are_used() {
    let pool = SenderPool::from_capacity(&[
        capacity(account(1, "free@gmail.com", "smtp"), 5),
        capacity(account(2, "paid@resend.example", "smtp"), 5),
    ]);

    let addresses: Vec<&str> = pool
        .entries()
        .iter()
        .map(|entry| entry.from.as_str())
        .collect();
    assert_eq!(addresses, ["free@gmail.com", "paid@resend.example"]);
}

#[tokio::test]
async fn an_empty_pool_hands_out_nothing() {
    let mut pool = SenderPool::default();

    assert!(pool.is_empty());
    assert_eq!(pool.remaining(), 0);
    assert!(pool.take().is_none());
}

/// The remaining figure is what the UI shows before a run, so it has to
/// reflect what has already been taken.
#[tokio::test]
async fn the_remaining_figure_tracks_what_has_been_taken() {
    let mut pool = SenderPool::from_capacity(&[
        capacity(account(1, "one@gmail.com", "smtp"), 2),
        capacity(account(2, "two@gmail.com", "smtp"), 2),
    ]);

    assert_eq!(pool.remaining(), 4);
    pool.take();
    pool.take();
    pool.take();
    assert_eq!(pool.remaining(), 1);
}

/// Debug output reaches logs, and these accounts hold mailbox passwords.
// Building an SMTP transport needs a tokio reactor, so this is async even
// though nothing here awaits.
#[tokio::test]
async fn debug_output_does_not_leak_credentials() {
    let pool = SenderPool::from_capacity(&[capacity(account(1, "one@gmail.com", "smtp"), 5)]);
    let debug = format!("{pool:?}");

    assert!(!debug.contains("app-password"), "{debug}");
    assert!(!debug.contains("re_abc"), "{debug}");
}
