use super::*;

use crate::history::{DEFAULT_USER_ID, NewRecord};

async fn store() -> Store {
    Store::open_in_memory().await.expect("an in-memory store")
}

const GOOD_PASSWORD: &str = "correct-horse-battery";

// -------------------------------------------------------------------
// A fresh install
// -------------------------------------------------------------------

/// A new install, and one upgraded from the single-user version, both start
/// with a user row that has no password.
#[tokio::test]
async fn a_fresh_install_has_nobody_who_can_sign_in() {
    let store = store().await;

    assert!(!store.has_any_password().await.unwrap());
    assert_eq!(store.users().await.unwrap().len(), 1);
    assert!(!store.users().await.unwrap()[0].has_password);
}

/// The rows already belong to user 1, so claiming that account has to keep
/// the history rather than starting an empty second one beside it.
#[tokio::test]
async fn claiming_the_first_account_keeps_the_existing_history() {
    let store = store().await;
    store
        .add_record(&NewRecord::sent(
            "acme",
            "Acme",
            "privacy@acme.example",
            "generic",
            "<id@x>",
        ))
        .await
        .unwrap();

    let user = store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert_eq!(
        user.id, DEFAULT_USER_ID,
        "it should be the existing account"
    );
    assert_eq!(user.username, "ezra");
    assert!(user.has_password);
    assert_eq!(
        store.stats(DEFAULT_USER_ID).await.unwrap().total,
        1,
        "the history should still be there"
    );
}

/// Otherwise anyone reaching the interface could take over an account that
/// already exists.
#[tokio::test]
async fn the_first_account_cannot_be_claimed_twice() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(matches!(
        store.claim_first_user("someone-else", GOOD_PASSWORD).await,
        Err(Error::Account(AccountError::UsernameTaken))
    ));
}

#[tokio::test]
async fn once_someone_has_a_password_the_instance_reports_it() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(store.has_any_password().await.unwrap());
}

// -------------------------------------------------------------------
// Creating accounts
// -------------------------------------------------------------------

#[tokio::test]
async fn an_account_can_be_created_and_signed_into() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    let sam = store.create_user("sam", "another-password").await.unwrap();
    assert_ne!(sam.id, DEFAULT_USER_ID);

    let signed_in = store
        .verify_password("sam", "another-password")
        .await
        .unwrap();
    assert_eq!(signed_in.id, sam.id);
}

/// Everything downstream assumes these rows exist.
#[tokio::test]
async fn a_new_account_gets_its_profile_and_settings_rows() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();
    let sam = store.create_user("sam", "another-password").await.unwrap();

    assert_eq!(
        store.profile(sam.id).await.unwrap(),
        crate::config::Profile::default()
    );
    assert_eq!(
        store.settings(sam.id).await.unwrap().0.template,
        crate::config::DEFAULT_TEMPLATE
    );

    // And saving works, which it would not if the row were missing.
    store
        .save_profile(
            sam.id,
            &crate::config::Profile {
                first_name: "Sam".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(store.profile(sam.id).await.unwrap().first_name, "Sam");
}

#[tokio::test]
async fn a_name_cannot_be_taken_twice() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(matches!(
        store.create_user("ezra", "another-password").await,
        Err(Error::Account(AccountError::UsernameTaken))
    ));
}

/// Two accounts that look identical in the interface would be a mess, so
/// names are compared without case.
#[tokio::test]
async fn names_do_not_differ_only_by_case() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(matches!(
        store.create_user("EZRA", "another-password").await,
        Err(Error::Account(AccountError::UsernameTaken))
    ));

    // And signing in works whichever case is typed.
    assert!(store.verify_password("EzRa", GOOD_PASSWORD).await.is_ok());
}

#[tokio::test]
async fn a_name_is_trimmed_and_checked() {
    let store = store().await;

    assert!(matches!(
        store.claim_first_user("   ", GOOD_PASSWORD).await,
        Err(Error::Account(AccountError::MissingUsername))
    ));
    assert!(matches!(
        store.claim_first_user("has spaces", GOOD_PASSWORD).await,
        Err(Error::Account(AccountError::InvalidUsername))
    ));
    assert!(matches!(
        store.claim_first_user("<script>", GOOD_PASSWORD).await,
        Err(Error::Account(AccountError::InvalidUsername))
    ));

    let user = store
        .claim_first_user("  ezra  ", GOOD_PASSWORD)
        .await
        .unwrap();
    assert_eq!(user.username, "ezra");
}

#[tokio::test]
async fn a_short_password_is_refused() {
    let store = store().await;

    assert!(matches!(
        store.claim_first_user("ezra", "short").await,
        Err(Error::Account(AccountError::PasswordTooShort))
    ));
    assert!(
        !store.has_any_password().await.unwrap(),
        "nothing was created"
    );
}

// -------------------------------------------------------------------
// Signing in
// -------------------------------------------------------------------

#[tokio::test]
async fn the_wrong_password_is_refused() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(matches!(
        store.verify_password("ezra", "not-the-password").await,
        Err(Error::Account(AccountError::WrongPassword))
    ));
}

/// An unknown name and a wrong password must be indistinguishable, or the
/// login form becomes a way to find out which accounts exist.
#[tokio::test]
async fn an_unknown_name_gives_the_same_answer_as_a_wrong_password() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    let unknown = store.verify_password("nobody", GOOD_PASSWORD).await;
    let wrong = store.verify_password("ezra", "not-the-password").await;

    assert_eq!(
        unknown.unwrap_err().to_string(),
        wrong.unwrap_err().to_string()
    );
}

#[tokio::test]
async fn an_account_with_no_password_cannot_be_signed_into() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    // A second account created directly with no hash, as an upgrade leaves.
    sqlx::query("INSERT INTO users (username, password_hash) VALUES ('sam', NULL)")
        .execute(store.pool())
        .await
        .unwrap();

    assert!(matches!(
        store.verify_password("sam", GOOD_PASSWORD).await,
        Err(Error::Account(AccountError::NoPasswordSet))
    ));
}

/// The stored value must be a hash, not the password.
#[tokio::test]
async fn the_password_is_not_stored_in_readable_form() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = 1")
        .fetch_one(store.pool())
        .await
        .unwrap();

    assert!(!stored.contains(GOOD_PASSWORD), "{stored}");
    assert!(stored.starts_with("$argon2"), "{stored}");
}

/// Two people choosing the same password must not produce the same hash, or
/// one leak reveals both.
#[tokio::test]
async fn the_same_password_hashes_differently_for_two_people() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();
    store.create_user("sam", GOOD_PASSWORD).await.unwrap();

    let hashes: Vec<String> = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_all(store.pool())
        .await
        .unwrap();

    assert_ne!(hashes[0], hashes[1]);
}

/// The hash must not be reachable through the type the rest of the code
/// passes around.
#[tokio::test]
async fn the_user_record_carries_no_hash() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    let user = store.user(DEFAULT_USER_ID).await.unwrap().unwrap();
    let rendered = format!("{user:?}") + &serde_json::to_string(&user).unwrap();

    assert!(!rendered.contains("argon2"), "{rendered}");
    assert!(!rendered.contains(GOOD_PASSWORD), "{rendered}");
    assert!(rendered.contains("ezra"));
}

// -------------------------------------------------------------------
// Changing a password
// -------------------------------------------------------------------

#[tokio::test]
async fn a_password_can_be_changed() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    store
        .change_password(DEFAULT_USER_ID, GOOD_PASSWORD, "a-brand-new-password")
        .await
        .unwrap();

    assert!(
        store
            .verify_password("ezra", "a-brand-new-password")
            .await
            .is_ok()
    );
    assert!(store.verify_password("ezra", GOOD_PASSWORD).await.is_err());
}

/// Otherwise anyone who reached an open session could lock the owner out.
#[tokio::test]
async fn changing_a_password_requires_the_current_one() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(matches!(
        store
            .change_password(DEFAULT_USER_ID, "not-the-password", "a-brand-new-password")
            .await,
        Err(Error::Account(AccountError::WrongPassword))
    ));
    assert!(store.verify_password("ezra", GOOD_PASSWORD).await.is_ok());
}

#[tokio::test]
async fn a_new_password_still_has_to_be_long_enough() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(matches!(
        store
            .change_password(DEFAULT_USER_ID, GOOD_PASSWORD, "short")
            .await,
        Err(Error::Account(AccountError::PasswordTooShort))
    ));
}

// -------------------------------------------------------------------
// Removing accounts
// -------------------------------------------------------------------

#[tokio::test]
async fn removing_an_account_takes_its_data_with_it() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();
    let sam = store.create_user("sam", "another-password").await.unwrap();

    store
        .add_record(
            &NewRecord::sent("acme", "Acme", "privacy@acme.example", "generic", "<id@x>")
                .for_user(sam.id),
        )
        .await
        .unwrap();
    store
        .add_sender_account(&crate::history::NewSenderAccount {
            user_id: sam.id,
            from_address: "sam@gmail.com".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(store.delete_user(sam.id).await.unwrap());

    assert!(store.user(sam.id).await.unwrap().is_none());
    assert_eq!(store.stats(sam.id).await.unwrap().total, 0);
    assert!(store.sender_accounts(sam.id).await.unwrap().is_empty());
    // And the other person is untouched.
    assert!(store.user(DEFAULT_USER_ID).await.unwrap().is_some());
}

/// An instance with no accounts cannot be signed into, and the only way back
/// would be editing the database by hand.
#[tokio::test]
async fn the_last_account_cannot_be_removed() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert!(!store.delete_user(DEFAULT_USER_ID).await.unwrap());
    assert_eq!(store.users().await.unwrap().len(), 1);
}

#[tokio::test]
async fn removing_an_account_that_does_not_exist_reports_it() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();
    store.create_user("sam", "another-password").await.unwrap();

    assert!(!store.delete_user(9999).await.unwrap());
}

// -------------------------------------------------------------------
// Listing
// -------------------------------------------------------------------

#[tokio::test]
async fn accounts_are_listed_oldest_first() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();
    store.create_user("sam", "another-password").await.unwrap();
    store.create_user("alex", "third-password").await.unwrap();

    let names: Vec<String> = store
        .users()
        .await
        .unwrap()
        .into_iter()
        .map(|user| user.username)
        .collect();
    assert_eq!(names, ["ezra", "sam", "alex"]);
}

#[tokio::test]
async fn an_account_can_be_found_by_name() {
    let store = store().await;
    store.claim_first_user("ezra", GOOD_PASSWORD).await.unwrap();

    assert_eq!(
        store.user_by_name("ezra").await.unwrap().map(|u| u.id),
        Some(DEFAULT_USER_ID)
    );
    assert!(store.user_by_name("nobody").await.unwrap().is_none());
}
