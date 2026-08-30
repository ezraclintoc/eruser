use super::*;

use crate::history::{DEFAULT_USER_ID, NewRecord, Status};

const SAM: i64 = 2;

/// A store with a second person on it.
async fn store() -> Store {
    let store = Store::open_in_memory().await.expect("an in-memory store");
    sqlx::query("INSERT INTO users (id, username) VALUES (?, 'sam')")
        .bind(SAM)
        .execute(store.pool())
        .await
        .expect("seed a second user");
    sqlx::query("INSERT INTO user_profiles (user_id) VALUES (?)")
        .bind(SAM)
        .execute(store.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_settings (user_id) VALUES (?)")
        .bind(SAM)
        .execute(store.pool())
        .await
        .unwrap();
    store
}

fn account(user_id: i64, address: &str, scope: AccountScope) -> NewSenderAccount {
    NewSenderAccount {
        user_id,
        label: address.split('@').next().unwrap_or_default().to_string(),
        scope,
        provider: "smtp".to_string(),
        from_address: address.to_string(),
        smtp: SmtpConfig {
            host: "smtp.gmail.com".into(),
            port: 465,
            username: address.to_string(),
            password: "app-password".into(),
            use_tls: true,
        },
        ..Default::default()
    }
}

fn profile() -> Profile {
    Profile {
        first_name: "Jane".into(),
        last_name: "Doe".into(),
        email: "jane@example.com".into(),
        city: "San Francisco".into(),
        ..Default::default()
    }
}

/// Record a send through a given account, at a given time.
async fn record_send(store: &Store, user_id: i64, account_id: i64, broker: &str) {
    let id = store
        .add_record(
            &NewRecord::sent(broker, broker, "privacy@x.example", "generic", "<id@x>")
                .for_user(user_id),
        )
        .await
        .unwrap();

    sqlx::query("UPDATE removal_requests SET sender_account_id = ? WHERE id = ?")
        .bind(account_id)
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();
}

// -------------------------------------------------------------------
// Profiles
// -------------------------------------------------------------------

#[tokio::test]
async fn a_profile_reads_back_as_it_was_saved() {
    let store = store().await;
    store
        .save_profile(DEFAULT_USER_ID, &profile())
        .await
        .unwrap();

    assert_eq!(store.profile(DEFAULT_USER_ID).await.unwrap(), profile());
}

#[tokio::test]
async fn saving_a_profile_twice_updates_rather_than_duplicates() {
    let store = store().await;
    store
        .save_profile(DEFAULT_USER_ID, &profile())
        .await
        .unwrap();

    let moved = Profile {
        city: "Oakland".into(),
        ..profile()
    };
    store.save_profile(DEFAULT_USER_ID, &moved).await.unwrap();

    assert_eq!(
        store.profile(DEFAULT_USER_ID).await.unwrap().city,
        "Oakland"
    );
}

/// Two people on one instance must not see each other's details.
#[tokio::test]
async fn profiles_do_not_leak_between_people() {
    let store = store().await;
    store
        .save_profile(DEFAULT_USER_ID, &profile())
        .await
        .unwrap();

    assert_eq!(store.profile(SAM).await.unwrap(), Profile::default());
}

// -------------------------------------------------------------------
// Settings
// -------------------------------------------------------------------

#[tokio::test]
async fn settings_read_back_as_they_were_saved() {
    let store = store().await;
    let options = Options {
        template: "gdpr".into(),
        rate_limit_ms: 500,
        regions: vec!["us".into(), "eu".into()],
        excluded_brokers: vec!["spokeo".into()],
        dry_run: false,
    };
    let inbox = InboxConfig {
        enabled: true,
        provider: "gmail".into(),
        server: "imap.gmail.com".into(),
        port: 993,
        email: "jane@gmail.com".into(),
        password: "app-password".into(),
        ..Default::default()
    };

    store
        .save_settings(DEFAULT_USER_ID, &options, &inbox)
        .await
        .unwrap();

    let (stored_options, stored_inbox) = store.settings(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(stored_options.template, "gdpr");
    assert_eq!(stored_options.regions, ["us", "eu"]);
    assert_eq!(stored_options.excluded_brokers, ["spokeo"]);
    assert_eq!(stored_inbox.email, "jane@gmail.com");
    assert!(stored_inbox.enabled);
}

#[tokio::test]
async fn a_person_with_no_settings_gets_the_defaults() {
    let store = store().await;
    let (options, inbox) = store.settings(SAM).await.unwrap();

    assert_eq!(options.template, crate::config::DEFAULT_TEMPLATE);
    assert_eq!(options.rate_limit_ms, crate::config::DEFAULT_RATE_LIMIT_MS);
    assert_eq!(inbox.folder, "INBOX");
    assert!(!inbox.enabled);
}

// -------------------------------------------------------------------
// Sending accounts
// -------------------------------------------------------------------

#[tokio::test]
async fn an_account_reads_back_as_it_was_added() {
    let store = store().await;
    let id = store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();

    let stored = store
        .sender_account(DEFAULT_USER_ID, id)
        .await
        .unwrap()
        .expect("the account should be there");

    assert_eq!(stored.from_address, "jane@gmail.com");
    assert_eq!(stored.provider, "smtp");
    assert_eq!(stored.smtp.port, 465);
    assert_eq!(stored.daily_limit, DEFAULT_DAILY_LIMIT);
    assert!(stored.enabled);
    assert_eq!(stored.scope, AccountScope::Personal);
}

#[tokio::test]
async fn adding_the_same_address_twice_updates_the_one_account() {
    let store = store().await;
    let first = store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();

    let second = store
        .add_sender_account(&NewSenderAccount {
            daily_limit: 400,
            ..account(DEFAULT_USER_ID, "jane@gmail.com", AccountScope::Family)
        })
        .await
        .unwrap();

    assert_eq!(first, second, "the same row should be reused");
    let accounts = store.sender_accounts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].daily_limit, 400);
    assert_eq!(accounts[0].scope, AccountScope::Family);
}

/// Two people can each add the same household address as their own.
#[tokio::test]
async fn two_people_can_each_hold_the_same_address() {
    let store = store().await;
    store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "house@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();
    store
        .add_sender_account(&account(SAM, "house@gmail.com", AccountScope::Personal))
        .await
        .unwrap();

    assert_eq!(
        store.sender_accounts(DEFAULT_USER_ID).await.unwrap().len(),
        1
    );
    assert_eq!(store.sender_accounts(SAM).await.unwrap().len(), 1);
}

#[tokio::test]
async fn accounts_come_back_in_the_order_a_run_would_use_them() {
    let store = store().await;
    store
        .add_sender_account(&NewSenderAccount {
            priority: 10,
            ..account(
                DEFAULT_USER_ID,
                "paid@resend.example",
                AccountScope::Personal,
            )
        })
        .await
        .unwrap();
    store
        .add_sender_account(&NewSenderAccount {
            priority: 0,
            ..account(DEFAULT_USER_ID, "free@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();

    let accounts = store.sender_accounts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(
        accounts[0].from_address, "free@gmail.com",
        "the free account should be spent first"
    );
}

#[tokio::test]
async fn an_account_can_be_taken_out_of_the_rotation_without_deleting_it() {
    let store = store().await;
    let id = store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();

    assert!(
        store
            .set_sender_account_enabled(DEFAULT_USER_ID, id, false)
            .await
            .unwrap()
    );

    let stored = store
        .sender_account(DEFAULT_USER_ID, id)
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.enabled);
    assert_eq!(
        store
            .remaining_capacity_today(DEFAULT_USER_ID)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn deleting_reports_whether_the_account_existed() {
    let store = store().await;
    let id = store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();

    assert!(
        store
            .delete_sender_account(DEFAULT_USER_ID, id)
            .await
            .unwrap()
    );
    assert!(
        !store
            .delete_sender_account(DEFAULT_USER_ID, id)
            .await
            .unwrap()
    );
}

/// A guessed id must not reach into someone else's account, since it holds
/// their mailbox password.
#[tokio::test]
async fn accounts_are_not_reachable_across_people() {
    let store = store().await;
    let id = store
        .add_sender_account(&account(SAM, "sam@gmail.com", AccountScope::Personal))
        .await
        .unwrap();

    assert!(
        store
            .sender_account(DEFAULT_USER_ID, id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !store
            .delete_sender_account(DEFAULT_USER_ID, id)
            .await
            .unwrap()
    );
    assert!(
        !store
            .set_sender_account_enabled(DEFAULT_USER_ID, id, false)
            .await
            .unwrap()
    );
}

// -------------------------------------------------------------------
// Sharing
// -------------------------------------------------------------------

/// The point of a family account: one household mailbox everyone can send
/// their own removal requests through.
#[tokio::test]
async fn a_family_account_can_be_used_by_someone_else() {
    let store = store().await;
    store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "house@gmail.com",
            AccountScope::Family,
        ))
        .await
        .unwrap();

    let usable = store.usable_sender_accounts(SAM).await.unwrap();
    assert_eq!(usable.len(), 1);
    assert_eq!(usable[0].from_address, "house@gmail.com");
}

#[tokio::test]
async fn a_personal_account_stays_with_its_owner() {
    let store = store().await;
    store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();

    assert!(store.usable_sender_accounts(SAM).await.unwrap().is_empty());
    assert_eq!(
        store
            .usable_sender_accounts(DEFAULT_USER_ID)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Sharing widens who can send as you, so a value nobody understands must
/// not be read as shared.
#[tokio::test]
async fn an_unrecognised_scope_reads_as_personal() {
    let store = store().await;
    let id = store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();

    sqlx::query("UPDATE sender_accounts SET scope = 'everyone-on-earth' WHERE id = ?")
        .bind(id)
        .execute(store.pool())
        .await
        .unwrap();

    let stored = store
        .sender_account(DEFAULT_USER_ID, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.scope, AccountScope::Personal);
    assert!(store.usable_sender_accounts(SAM).await.unwrap().is_empty());
}

/// Managing accounts should show what you own, not what you may borrow.
#[tokio::test]
async fn the_management_list_shows_only_your_own_accounts() {
    let store = store().await;
    store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "house@gmail.com",
            AccountScope::Family,
        ))
        .await
        .unwrap();

    assert!(store.sender_accounts(SAM).await.unwrap().is_empty());
    assert_eq!(store.usable_sender_accounts(SAM).await.unwrap().len(), 1);
}

// -------------------------------------------------------------------
// Daily capacity
// -------------------------------------------------------------------

#[tokio::test]
async fn a_fresh_account_has_its_whole_allowance() {
    let store = store().await;
    store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();

    let capacity = store.account_capacity(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(capacity[0].sent_today, 0);
    assert_eq!(capacity[0].remaining, DEFAULT_DAILY_LIMIT);
    assert!(capacity[0].is_available());
}

#[tokio::test]
async fn sending_uses_up_that_accounts_allowance() {
    let store = store().await;
    let id = store
        .add_sender_account(&NewSenderAccount {
            daily_limit: 3,
            ..account(DEFAULT_USER_ID, "jane@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();

    for broker in ["a", "b"] {
        record_send(&store, DEFAULT_USER_ID, id, broker).await;
    }

    let capacity = store.account_capacity(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(capacity[0].sent_today, 2);
    assert_eq!(capacity[0].remaining, 1);
}

/// The whole reason for several accounts: three caps, not one.
#[tokio::test]
async fn each_account_has_its_own_allowance() {
    let store = store().await;
    let first = store
        .add_sender_account(&NewSenderAccount {
            daily_limit: 2,
            priority: 0,
            ..account(DEFAULT_USER_ID, "one@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();
    store
        .add_sender_account(&NewSenderAccount {
            daily_limit: 2,
            priority: 1,
            ..account(DEFAULT_USER_ID, "two@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();

    // Spend the first one completely.
    for broker in ["a", "b"] {
        record_send(&store, DEFAULT_USER_ID, first, broker).await;
    }

    let capacity = store.account_capacity(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(capacity[0].remaining, 0, "the first is spent");
    assert!(!capacity[0].is_available());
    assert_eq!(capacity[1].remaining, 2, "the second is untouched");

    assert_eq!(
        store
            .remaining_capacity_today(DEFAULT_USER_ID)
            .await
            .unwrap(),
        2
    );
}

/// A shared mailbox has one allowance however many people send through it.
/// Counting per person would let a household blow past the provider's cap
/// and get the address rate limited.
#[tokio::test]
async fn a_shared_account_has_one_allowance_between_everyone() {
    let store = store().await;
    let id = store
        .add_sender_account(&NewSenderAccount {
            daily_limit: 10,
            ..account(DEFAULT_USER_ID, "house@gmail.com", AccountScope::Family)
        })
        .await
        .unwrap();

    record_send(&store, DEFAULT_USER_ID, id, "a").await;
    record_send(&store, SAM, id, "b").await;
    record_send(&store, SAM, id, "c").await;

    // Both people see the same remaining figure.
    for user in [DEFAULT_USER_ID, SAM] {
        let capacity = store.account_capacity(user).await.unwrap();
        let house = capacity
            .iter()
            .find(|c| c.account.from_address == "house@gmail.com")
            .expect("the shared account should be visible to both");

        assert_eq!(house.sent_today, 3, "for user {user}");
        assert_eq!(house.remaining, 7, "for user {user}");
    }
}

#[tokio::test]
async fn an_allowance_never_goes_negative() {
    let store = store().await;
    let id = store
        .add_sender_account(&NewSenderAccount {
            daily_limit: 1,
            ..account(DEFAULT_USER_ID, "jane@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();

    for broker in ["a", "b", "c"] {
        record_send(&store, DEFAULT_USER_ID, id, broker).await;
    }

    assert_eq!(
        store.account_capacity(DEFAULT_USER_ID).await.unwrap()[0].remaining,
        0
    );
}

/// Yesterday's sends are not today's problem.
#[tokio::test]
async fn an_allowance_resets_with_the_day() {
    let store = store().await;
    let id = store
        .add_sender_account(&NewSenderAccount {
            daily_limit: 2,
            ..account(DEFAULT_USER_ID, "jane@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();

    record_send(&store, DEFAULT_USER_ID, id, "a").await;
    sqlx::query("UPDATE removal_requests SET sent_at = ?")
        .bind(Utc::now() - chrono::Duration::days(2))
        .execute(store.pool())
        .await
        .unwrap();

    let capacity = store.account_capacity(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(capacity[0].sent_today, 0);
    assert_eq!(capacity[0].remaining, 2);
}

#[tokio::test]
async fn someone_with_no_accounts_can_send_nothing() {
    let store = store().await;
    assert!(
        store
            .account_capacity(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .remaining_capacity_today(DEFAULT_USER_ID)
            .await
            .unwrap(),
        0
    );
}

// -------------------------------------------------------------------
// Importing the config file
// -------------------------------------------------------------------

fn importable_config() -> Config {
    Config {
        profile: profile(),
        email: EmailConfig {
            provider: "smtp".into(),
            from: "jane@gmail.com".into(),
            smtp: SmtpConfig {
                host: "smtp.gmail.com".into(),
                port: 465,
                username: "jane@gmail.com".into(),
                password: "app-password".into(),
                use_tls: true,
            },
            ..Default::default()
        },
        options: Options {
            template: "gdpr".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// This is how an existing single-user install upgrades.
#[tokio::test]
async fn importing_a_config_file_brings_the_person_and_their_mailbox_across() {
    let store = store().await;
    assert!(
        store
            .import_config(DEFAULT_USER_ID, &importable_config())
            .await
            .unwrap()
    );

    assert_eq!(
        store.profile(DEFAULT_USER_ID).await.unwrap().first_name,
        "Jane"
    );
    assert_eq!(
        store.settings(DEFAULT_USER_ID).await.unwrap().0.template,
        "gdpr"
    );

    let accounts = store.sender_accounts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].from_address, "jane@gmail.com");
    assert_eq!(accounts[0].smtp.password, "app-password");
    assert_eq!(
        accounts[0].scope,
        AccountScope::Personal,
        "an imported account belongs to whoever was already using this install"
    );
}

/// Editing settings in the database must not be undone by a stale file on
/// the next start.
#[tokio::test]
async fn importing_only_happens_once() {
    let store = store().await;
    store
        .import_config(DEFAULT_USER_ID, &importable_config())
        .await
        .unwrap();

    let moved = Profile {
        city: "Oakland".into(),
        ..profile()
    };
    store.save_profile(DEFAULT_USER_ID, &moved).await.unwrap();

    assert!(
        !store
            .import_config(DEFAULT_USER_ID, &importable_config())
            .await
            .unwrap(),
        "the second import should be a no-op"
    );
    assert_eq!(
        store.profile(DEFAULT_USER_ID).await.unwrap().city,
        "Oakland"
    );
}

/// A config that could never send should not leave a broken account behind.
#[tokio::test]
async fn a_config_with_no_usable_mailbox_imports_the_profile_only() {
    let store = store().await;
    let no_mailbox = Config {
        email: EmailConfig::default(),
        ..importable_config()
    };

    store
        .import_config(DEFAULT_USER_ID, &no_mailbox)
        .await
        .unwrap();

    assert_eq!(
        store.profile(DEFAULT_USER_ID).await.unwrap().first_name,
        "Jane"
    );
    assert!(
        store
            .sender_accounts(DEFAULT_USER_ID)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn an_api_key_survives_the_import() {
    let store = store().await;
    let with_key = Config {
        email: EmailConfig {
            provider: "resend".into(),
            from: "jane@example.com".into(),
            resend: crate::config::ApiKeyConfig {
                api_key: "re_abc".into(),
            },
            ..Default::default()
        },
        ..importable_config()
    };

    store
        .import_config(DEFAULT_USER_ID, &with_key)
        .await
        .unwrap();

    let accounts = store.sender_accounts(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(accounts[0].provider, "resend");
    assert_eq!(accounts[0].api_key, "re_abc");
}

/// The code that still wants a Config should get one built from the
/// database, not the file.
#[tokio::test]
async fn a_config_can_be_rebuilt_from_the_database() {
    let store = store().await;
    store
        .import_config(DEFAULT_USER_ID, &importable_config())
        .await
        .unwrap();

    let rebuilt = store.config_for(DEFAULT_USER_ID).await.unwrap();

    assert_eq!(rebuilt.profile.first_name, "Jane");
    assert_eq!(rebuilt.options.template, "gdpr");
    assert_eq!(rebuilt.email.from, "jane@gmail.com");
    assert!(
        rebuilt.validate().is_ok(),
        "it should be usable for sending"
    );
}

/// A disabled account must not become the one a rebuilt config sends from.
#[tokio::test]
async fn a_rebuilt_config_skips_disabled_accounts() {
    let store = store().await;
    let disabled = store
        .add_sender_account(&NewSenderAccount {
            priority: 0,
            ..account(DEFAULT_USER_ID, "off@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();
    store
        .add_sender_account(&NewSenderAccount {
            priority: 1,
            ..account(DEFAULT_USER_ID, "on@gmail.com", AccountScope::Personal)
        })
        .await
        .unwrap();
    store
        .set_sender_account_enabled(DEFAULT_USER_ID, disabled, false)
        .await
        .unwrap();

    let rebuilt = store.config_for(DEFAULT_USER_ID).await.unwrap();
    assert_eq!(rebuilt.email.from, "on@gmail.com");
}

// -------------------------------------------------------------------
// Presentation
// -------------------------------------------------------------------

#[test]
fn an_account_is_named_by_its_label_and_address() {
    let named = SenderAccount {
        id: 1,
        user_id: 1,
        label: "personal gmail".into(),
        scope: AccountScope::Personal,
        provider: "smtp".into(),
        from_address: "jane@gmail.com".into(),
        smtp: SmtpConfig::default(),
        api_key: String::new(),
        daily_limit: 250,
        enabled: true,
        priority: 0,
        created_at: None,
    };

    assert_eq!(named.display_name(), "personal gmail (jane@gmail.com)");

    let unlabelled = SenderAccount {
        label: String::new(),
        ..named
    };
    assert_eq!(unlabelled.display_name(), "jane@gmail.com");
}

#[test]
fn scopes_round_trip_through_the_database_representation() {
    for scope in [AccountScope::Personal, AccountScope::Family] {
        assert_eq!(AccountScope::from_db(scope.as_str()), scope);
    }
    assert!(AccountScope::Family.is_shared());
    assert!(!AccountScope::Personal.is_shared());
}

#[tokio::test]
async fn a_status_is_still_recorded_against_the_account_that_sent_it() {
    let store = store().await;
    let id = store
        .add_sender_account(&account(
            DEFAULT_USER_ID,
            "jane@gmail.com",
            AccountScope::Personal,
        ))
        .await
        .unwrap();
    record_send(&store, DEFAULT_USER_ID, id, "acme").await;

    let record = store
        .last_request_for_broker(DEFAULT_USER_ID, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, Status::Sent);
}
