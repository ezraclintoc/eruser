use super::*;

#[test]
fn the_common_providers_are_known() {
    assert_eq!(smtp_host_for("jane@gmail.com"), Some(GMAIL_SMTP_HOST));
    assert_eq!(
        smtp_host_for("jane@outlook.com"),
        Some("smtp-mail.outlook.com")
    );
    assert_eq!(smtp_host_for("jane@icloud.com"), Some("smtp.mail.me.com"));
    assert_eq!(smtp_host_for("jane@proton.me"), Some("smtp.protonmail.ch"));
    assert_eq!(
        smtp_host_for("jane@fastmail.com"),
        Some("smtp.fastmail.com")
    );
}

/// Google hands out two domains for the same mailbox.
#[test]
fn the_alternative_domains_reach_the_same_server() {
    assert_eq!(
        smtp_host_for("jane@googlemail.com"),
        smtp_host_for("jane@gmail.com")
    );
    assert_eq!(
        smtp_host_for("jane@me.com"),
        smtp_host_for("jane@icloud.com")
    );
}

#[test]
fn the_domain_is_matched_regardless_of_case_or_spacing() {
    assert_eq!(smtp_host_for("Jane@GMail.COM"), Some(GMAIL_SMTP_HOST));
    assert_eq!(smtp_host_for("jane@gmail.com "), Some(GMAIL_SMTP_HOST));
}

#[test]
fn an_unknown_domain_gets_no_guess() {
    assert_eq!(smtp_host_for("jane@example.com"), None);
    assert_eq!(smtp_host_for("jane@mail.example.com"), None);
}

#[test]
fn something_that_is_not_an_address_gets_no_guess() {
    assert_eq!(smtp_host_for("jane"), None);
    assert_eq!(smtp_host_for(""), None);
}

/// The last `@` wins, so a display name containing one does not confuse it.
#[test]
fn the_domain_is_taken_from_the_last_at_sign() {
    assert_eq!(smtp_host_for("weird@name@gmail.com"), Some(GMAIL_SMTP_HOST));
}

#[test]
fn the_providers_that_require_an_app_password_are_flagged() {
    assert!(needs_app_password("jane@gmail.com"));
    assert!(needs_app_password("jane@yahoo.com"));
    assert!(needs_app_password("jane@icloud.com"));
}

#[test]
fn a_provider_that_takes_the_account_password_is_not_flagged() {
    assert!(!needs_app_password("jane@fastmail.com"));
    assert!(!needs_app_password("jane@example.com"));
}
