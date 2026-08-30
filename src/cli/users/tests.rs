use chrono::TimeZone;

use super::*;

fn user(id: i64, username: &str, has_password: bool) -> User {
    User {
        id,
        username: username.to_string(),
        has_password,
        created_at: chrono::Utc.with_ymd_and_hms(2026, 3, 4, 9, 0, 0).single(),
    }
}

#[test]
fn an_empty_list_says_how_to_make_the_first_account() {
    let out = format_users(&[]);
    assert!(out.contains("No accounts yet"));
    assert!(out.contains("eruser users add"));
}

#[test]
fn each_account_is_listed_with_when_it_was_added() {
    let out = format_users(&[user(1, "jane", true), user(2, "sam", true)]);

    assert!(out.contains("jane"));
    assert!(out.contains("sam"));
    assert!(out.contains("2026-03-04"));
    assert_eq!(out.lines().count(), 2);
}

/// The names line up, so a long one does not ragged the column.
#[test]
fn the_names_are_padded_to_the_longest() {
    let out = format_users(&[user(1, "jo", true), user(2, "bartholomew", true)]);

    let columns: Vec<_> = out
        .lines()
        .map(|line| line.find("added").expect("every line says when"))
        .collect();
    assert_eq!(columns[0], columns[1]);
}

/// The row the single-user version left behind has no password, and the web
/// interface will offer it to whoever asks first. Say so.
#[test]
fn an_account_with_no_password_is_called_out() {
    let out = format_users(&[user(1, "jane", false)]);
    assert!(out.contains("unclaimed"));
}

#[test]
fn an_account_with_a_password_is_not_called_out() {
    let out = format_users(&[user(1, "jane", true)]);
    assert!(!out.contains("unclaimed"));
}

#[test]
fn an_account_with_no_recorded_date_still_lists() {
    let mut jane = user(1, "jane", true);
    jane.created_at = None;

    let out = format_users(&[jane]);
    assert!(out.contains("jane"));
    assert!(out.contains('—'));
}
