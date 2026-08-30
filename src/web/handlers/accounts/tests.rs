//! Turning the add form into an account.

use super::*;
use crate::history::DEFAULT_USER_ID;

fn form(from: &str) -> AccountForm {
    AccountForm {
        from_address: from.to_string(),
        smtp_password: "app-password".to_string(),
        ..AccountForm::default()
    }
}

#[test]
fn a_gmail_address_needs_nothing_but_a_password() {
    let built = build(DEFAULT_USER_ID, &form("jane@gmail.com")).expect("an account");

    assert_eq!(built.from_address, "jane@gmail.com");
    assert_eq!(built.smtp.host, "smtp.gmail.com");
    assert_eq!(built.smtp.port, 465);
    assert_eq!(built.smtp.username, "jane@gmail.com");
    assert!(built.smtp.use_tls);
}

#[test]
fn an_empty_address_is_refused() {
    let problem = build(DEFAULT_USER_ID, &form("   ")).expect_err("this should be refused");
    assert!(problem.contains("Enter the address"));
}

#[test]
fn something_that_is_not_an_address_is_refused() {
    let problem = build(DEFAULT_USER_ID, &form("jane")).expect_err("this should be refused");
    assert!(problem.contains("not an email address"));
}

/// The most common reason a first send fails, so it is worth catching here
/// rather than at send time.
#[test]
fn an_smtp_account_without_a_password_is_refused() {
    let mut fields = form("jane@gmail.com");
    fields.smtp_password = String::new();

    let problem = build(DEFAULT_USER_ID, &fields).expect_err("this should be refused");
    assert!(problem.contains("app password"));
}

#[test]
fn an_unknown_domain_has_to_name_its_server() {
    let problem =
        build(DEFAULT_USER_ID, &form("jane@example.com")).expect_err("this should be refused");
    assert!(problem.contains("no SMTP server known"));
}

#[test]
fn a_named_server_is_used_for_a_domain_nobody_knows() {
    let mut fields = form("jane@example.com");
    fields.smtp_host = "smtp.example.com".to_string();

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");
    assert_eq!(built.smtp.host, "smtp.example.com");
}

#[test]
fn a_named_server_wins_over_the_guess() {
    let mut fields = form("jane@gmail.com");
    fields.smtp_host = "smtp.elsewhere.example".to_string();

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");
    assert_eq!(built.smtp.host, "smtp.elsewhere.example");
}

#[test]
fn a_separate_username_is_kept() {
    let mut fields = form("jane@example.com");
    fields.smtp_host = "smtp.example.com".to_string();
    fields.smtp_username = "jane.doe".to_string();

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");
    assert_eq!(built.smtp.username, "jane.doe");
}

// -------------------------------------------------------------------
// Who may send through it
// -------------------------------------------------------------------

/// A checkbox that was not ticked is simply absent from the form, so the
/// default has to be the safe one.
#[test]
fn an_account_is_personal_unless_the_box_was_ticked() {
    let built = build(DEFAULT_USER_ID, &form("jane@gmail.com")).expect("an account");

    assert_eq!(built.scope, AccountScope::Personal);
    assert!(!built.scope.is_shared());
}

#[test]
fn ticking_the_box_shares_the_account_with_the_household() {
    let mut fields = form("house@gmail.com");
    fields.family = Some("on".to_string());

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");

    assert_eq!(built.scope, AccountScope::Family);
    assert!(built.scope.is_shared());
}

#[test]
fn a_new_account_belongs_to_whoever_added_it() {
    let built = build(7, &form("jane@gmail.com")).expect("an account");
    assert_eq!(built.user_id, 7);
}

// -------------------------------------------------------------------
// The other providers
// -------------------------------------------------------------------

#[test]
fn an_api_provider_takes_a_key_and_no_server() {
    let mut fields = form("jane@example.com");
    fields.provider = "resend".to_string();
    fields.api_key = "re_key".to_string();
    fields.smtp_password = String::new();

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");

    assert_eq!(built.provider, "resend");
    assert_eq!(built.api_key, "re_key");
    assert!(built.smtp.host.is_empty());
}

#[test]
fn an_api_provider_without_a_key_is_refused() {
    let mut fields = form("jane@example.com");
    fields.provider = "sendgrid".to_string();

    let problem = build(DEFAULT_USER_ID, &fields).expect_err("this should be refused");
    assert!(problem.contains("API key"));
}

#[test]
fn an_unknown_provider_is_refused() {
    let mut fields = form("jane@gmail.com");
    fields.provider = "carrier-pigeon".to_string();

    let problem = build(DEFAULT_USER_ID, &fields).expect_err("this should be refused");
    assert!(problem.contains("carrier-pigeon"));
}

#[test]
fn an_empty_provider_means_smtp() {
    let built = build(DEFAULT_USER_ID, &form("jane@gmail.com")).expect("an account");
    assert_eq!(built.provider, "smtp");
}

// -------------------------------------------------------------------
// Numbers
// -------------------------------------------------------------------

#[test]
fn an_empty_daily_limit_means_the_default() {
    let built = build(DEFAULT_USER_ID, &form("jane@gmail.com")).expect("an account");
    assert_eq!(built.daily_limit, DEFAULT_DAILY_LIMIT);
}

#[test]
fn a_given_daily_limit_is_kept() {
    let mut fields = form("jane@gmail.com");
    fields.daily_limit = "40".to_string();

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");
    assert_eq!(built.daily_limit, 40);
}

/// A limit of zero would silently mean "never send from this", which is what
/// disabling is for.
#[test]
fn a_daily_limit_of_zero_or_less_is_refused() {
    for raw in ["0", "-5"] {
        let mut fields = form("jane@gmail.com");
        fields.daily_limit = raw.to_string();

        let problem = build(DEFAULT_USER_ID, &fields).expect_err("this should be refused");
        assert!(problem.contains("above zero"), "{raw} should be refused");
    }
}

#[test]
fn a_daily_limit_that_is_not_a_number_is_refused() {
    let mut fields = form("jane@gmail.com");
    fields.daily_limit = "lots".to_string();

    let problem = build(DEFAULT_USER_ID, &fields).expect_err("this should be refused");
    assert!(problem.contains("has to be a number"));
}

#[test]
fn an_empty_port_means_implicit_tls() {
    let built = build(DEFAULT_USER_ID, &form("jane@gmail.com")).expect("an account");
    assert_eq!(built.smtp.port, 465);
}

#[test]
fn a_given_port_is_kept() {
    let mut fields = form("jane@gmail.com");
    fields.smtp_port = "587".to_string();

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");
    assert_eq!(built.smtp.port, 587);
}

#[test]
fn a_port_that_is_not_a_number_is_refused() {
    let mut fields = form("jane@gmail.com");
    fields.smtp_port = "smtp".to_string();

    let problem = build(DEFAULT_USER_ID, &fields).expect_err("this should be refused");
    assert!(problem.contains("between 1 and 65535"));
}

/// Copied out of a password manager, an address often arrives with spaces
/// around it. They must not become part of the address, or nothing will
/// match it later.
#[test]
fn the_fields_are_trimmed() {
    let mut fields = form("  jane@gmail.com  ");
    fields.label = "  personal gmail  ".to_string();

    let built = build(DEFAULT_USER_ID, &fields).expect("an account");

    assert_eq!(built.from_address, "jane@gmail.com");
    assert_eq!(built.label, "personal gmail");
}
