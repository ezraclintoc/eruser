use super::*;
use crate::config::SmtpConfig;
use crate::history::SenderAccount;

fn account(address: &str, label: &str, scope: AccountScope) -> SenderAccount {
    SenderAccount {
        id: 1,
        user_id: DEFAULT_USER_ID,
        label: label.to_string(),
        scope,
        provider: "smtp".to_string(),
        from_address: address.to_string(),
        smtp: SmtpConfig::default(),
        api_key: String::new(),
        daily_limit: 250,
        enabled: true,
        priority: 0,
        created_at: None,
    }
}

fn capacity(account: SenderAccount, sent_today: i64) -> AccountCapacity {
    AccountCapacity {
        remaining: (account.daily_limit - sent_today).max(0),
        sent_today,
        account,
    }
}

#[test]
fn an_empty_list_says_how_to_add_one() {
    let out = format_accounts(&[]);
    assert!(out.contains("No sending accounts yet"));
    assert!(out.contains("eruser accounts add"));
}

#[test]
fn each_account_shows_what_is_left_today() {
    let out = format_accounts(&[capacity(
        account("jane@gmail.com", "", AccountScope::Personal),
        30,
    )]);

    assert!(out.contains("jane@gmail.com"));
    assert!(out.contains("220 left today"));
}

#[test]
fn a_shared_account_says_so() {
    let out = format_accounts(&[capacity(
        account("house@gmail.com", "", AccountScope::Family),
        0,
    )]);

    assert!(out.contains("shared with the household"));
}

#[test]
fn a_personal_account_does_not_mention_sharing() {
    let out = format_accounts(&[capacity(
        account("jane@gmail.com", "", AccountScope::Personal),
        0,
    )]);

    assert!(!out.contains("shared"));
}

#[test]
fn the_label_is_shown_when_there_is_one() {
    let out = format_accounts(&[capacity(
        account("jane@gmail.com", "personal gmail", AccountScope::Personal),
        0,
    )]);

    assert!(out.contains("personal gmail"));
}

#[test]
fn a_disabled_account_is_marked_and_left_out_of_the_total() {
    let mut spare = account("spare@gmail.com", "", AccountScope::Personal);
    spare.enabled = false;

    let out = format_accounts(&[
        capacity(account("jane@gmail.com", "", AccountScope::Personal), 200),
        capacity(spare, 0),
    ]);

    assert!(out.contains("disabled"));
    assert!(out.contains("50 can be sent today"));
}

/// The whole point of several accounts: the allowances add up.
#[test]
fn the_total_is_the_sum_of_what_is_left() {
    let out = format_accounts(&[
        capacity(account("one@gmail.com", "", AccountScope::Personal), 200),
        capacity(account("two@gmail.com", "", AccountScope::Personal), 100),
    ]);

    assert!(out.contains("200 can be sent today"));
}

/// An account that has used its whole allowance counts for nothing today,
/// and never reports a negative number.
#[test]
fn a_spent_account_contributes_nothing() {
    let out = format_accounts(&[capacity(
        account("jane@gmail.com", "", AccountScope::Personal),
        400,
    )]);

    assert!(out.contains("0 left today"));
    assert!(out.contains("0 can be sent today"));
    assert!(!out.contains('-'));
}

#[test]
fn the_addresses_are_padded_to_the_longest() {
    let out = format_accounts(&[
        capacity(account("a@b.com", "", AccountScope::Personal), 0),
        capacity(
            account("a-much-longer@example.com", "", AccountScope::Personal),
            0,
        ),
    ]);

    let columns: Vec<_> = out
        .lines()
        .filter(|line| line.contains("smtp"))
        .map(|line| line.find("smtp").expect("the provider column"))
        .collect();
    assert_eq!(columns[0], columns[1]);
}

// -------------------------------------------------------------------
// Guessing the SMTP server
// -------------------------------------------------------------------

#[test]
fn the_common_providers_need_no_host_flag() {
    assert_eq!(smtp_host_for("jane@gmail.com"), Some("smtp.gmail.com"));
    assert_eq!(
        smtp_host_for("jane@outlook.com"),
        Some("smtp-mail.outlook.com")
    );
    assert_eq!(smtp_host_for("jane@icloud.com"), Some("smtp.mail.me.com"));
    assert_eq!(smtp_host_for("jane@proton.me"), Some("smtp.protonmail.ch"));
}

#[test]
fn the_domain_is_matched_regardless_of_case() {
    assert_eq!(smtp_host_for("Jane@GMail.COM"), Some("smtp.gmail.com"));
}

/// A domain nobody knows gets no guess, so the command asks rather than
/// inventing a server that will fail to connect.
#[test]
fn an_unknown_domain_gets_no_guess() {
    assert_eq!(smtp_host_for("jane@example.com"), None);
}

#[test]
fn something_that_is_not_an_address_gets_no_guess() {
    assert_eq!(smtp_host_for("jane"), None);
}

// -------------------------------------------------------------------
// Building an account from the arguments
// -------------------------------------------------------------------

fn add_args(from: &str) -> AddArgs {
    AddArgs {
        who: WhoArgs::default(),
        from: from.to_string(),
        label: None,
        provider: "smtp".to_string(),
        host: None,
        port: 465,
        username: None,
        family: false,
        daily_limit: 250,
        priority: 0,
    }
}

/// The scope has to be asked for. An account nobody marked shared stays the
/// owner's, because sharing one lets someone else send mail as them.
#[test]
fn an_account_is_personal_unless_the_flag_says_otherwise() {
    let args = add_args("jane@gmail.com");
    let built =
        build(DEFAULT_USER_ID, &args, |_| Ok("app-password".to_string())).expect("an account");

    assert_eq!(built.scope, AccountScope::Personal);
}

#[test]
fn an_unknown_provider_is_refused() {
    let mut args = add_args("jane@example.com");
    args.provider = "carrier-pigeon".to_string();

    let error = build(DEFAULT_USER_ID, &args, |_| Ok("app-password".to_string()))
        .expect_err("this should be refused");
    assert!(error.to_string().contains("carrier-pigeon"));
}

/// A domain with no known server and no `--host` cannot be guessed at.
#[test]
fn an_unguessable_address_asks_for_a_host() {
    let args = add_args("jane@example.com");
    args.host.is_none().then_some(()).expect("no host given");

    let error = build(DEFAULT_USER_ID, &args, |_| Ok("app-password".to_string()))
        .expect_err("this should be refused");
    assert!(error.to_string().contains("--host"));
}

#[test]
fn the_family_flag_marks_the_account_shared() {
    let mut args = add_args("house@gmail.com");
    args.family = true;

    let built =
        build(DEFAULT_USER_ID, &args, |_| Ok("app-password".to_string())).expect("an account");

    assert_eq!(built.scope, AccountScope::Family);
    assert!(built.scope.is_shared());
}

#[test]
fn the_smtp_server_is_filled_in_from_the_address() {
    let args = add_args("jane@gmail.com");

    let built =
        build(DEFAULT_USER_ID, &args, |_| Ok("app-password".to_string())).expect("an account");

    assert_eq!(built.smtp.host, "smtp.gmail.com");
    assert_eq!(built.smtp.username, "jane@gmail.com");
    assert_eq!(built.smtp.password, "app-password");
    assert!(built.smtp.use_tls);
}

/// An explicit host wins over the guess, so a provider that moved its server
/// is still usable.
#[test]
fn a_given_host_is_used_instead_of_the_guess() {
    let mut args = add_args("jane@gmail.com");
    args.host = Some("smtp.elsewhere.example".to_string());

    let built =
        build(DEFAULT_USER_ID, &args, |_| Ok("app-password".to_string())).expect("an account");

    assert_eq!(built.smtp.host, "smtp.elsewhere.example");
}

/// An API provider needs a key, not an SMTP server, and must not end up with
/// half-filled SMTP settings that look usable.
#[test]
fn an_api_provider_takes_a_key_and_no_server() {
    let mut args = add_args("jane@example.com");
    args.provider = "resend".to_string();

    let built = build(DEFAULT_USER_ID, &args, |_| Ok("re_key".to_string())).expect("an account");

    assert_eq!(built.api_key, "re_key");
    assert!(built.smtp.host.is_empty());
}

#[test]
fn the_provider_name_is_matched_regardless_of_case() {
    let mut args = add_args("jane@example.com");
    args.provider = "SendGrid".to_string();

    let built = build(DEFAULT_USER_ID, &args, |_| Ok("sg_key".to_string())).expect("an account");

    assert_eq!(built.provider, "sendgrid");
}

/// Surrounding whitespace from a copy and paste should not become part of
/// the address, or the account would never match on lookup.
#[test]
fn the_address_is_trimmed() {
    let args = add_args("  jane@gmail.com  ");

    let built =
        build(DEFAULT_USER_ID, &args, |_| Ok("app-password".to_string())).expect("an account");

    assert_eq!(built.from_address, "jane@gmail.com");
}
