use super::*;

fn email(body: &str) -> Email {
    Email {
        body: body.to_string(),
        ..Default::default()
    }
}

fn html_email(html: &str) -> Email {
    Email {
        html_body: html.to_string(),
        ..Default::default()
    }
}

// -------------------------------------------------------------------
// URL extraction
// -------------------------------------------------------------------

#[test]
fn urls_are_found_in_plain_text() {
    let urls = parse_email_urls(&email(
        "Please visit https://acme.example/opt-out to remove your data.",
    ));
    assert_eq!(urls.all_urls, ["https://acme.example/opt-out"]);
}

#[test]
fn urls_are_found_in_html_hrefs() {
    let urls = parse_email_urls(&html_email(
        r#"<p>Go <a href="https://acme.example/optout">here</a> to opt out.</p>"#,
    ));
    assert!(
        urls.all_urls
            .contains(&"https://acme.example/optout".to_string())
    );
}

#[test]
fn a_url_in_html_text_is_found_as_well_as_the_href() {
    let urls = parse_email_urls(&html_email(
        r#"<p>Visit <a href="https://acme.example/optout">https://acme.example/optout</a></p>"#,
    ));
    assert_eq!(urls.all_urls.len(), 1, "the same URL should appear once");
}

#[test]
fn the_same_url_twice_is_listed_once() {
    let urls = parse_email_urls(&email(
        "https://acme.example/opt-out and again https://acme.example/opt-out",
    ));
    assert_eq!(urls.all_urls.len(), 1);
}

/// A URL ending a sentence picks up the full stop.
#[test]
fn trailing_punctuation_is_trimmed() {
    let urls = parse_email_urls(&email("See https://acme.example/opt-out."));
    assert_eq!(urls.all_urls, ["https://acme.example/opt-out"]);
}

#[test]
fn non_http_schemes_are_ignored() {
    let urls = parse_email_urls(&email(
        "Write to mailto:privacy@acme.example or ftp://acme.example/file",
    ));
    assert!(urls.all_urls.is_empty());
}

#[test]
fn an_email_with_no_links_yields_nothing() {
    let urls = parse_email_urls(&email("We have removed your data."));
    assert!(urls.is_empty());
}

// -------------------------------------------------------------------
// Categorising
// -------------------------------------------------------------------

#[test]
fn opt_out_links_are_recognised_as_forms() {
    for url in [
        "https://acme.example/opt-out",
        "https://acme.example/optout",
        "https://acme.example/do-not-sell",
        "https://acme.example/privacy-request",
        "https://acme.example/dsar",
        "https://acme.example/removal",
    ] {
        assert!(is_form_url(url), "{url} should read as a form");
    }
}

/// A link to a privacy policy explains your rights; it does not exercise
/// them. Sending someone there instead of to the form wastes the trip.
#[test]
fn policy_and_help_pages_are_not_forms() {
    for url in [
        "https://acme.example/privacy-policy",
        "https://acme.example/terms",
        "https://acme.example/faq",
        "https://acme.example/login",
        "https://acme.example/account",
        "https://acme.example/rights.pdf",
        "https://facebook.com/acme",
    ] {
        assert!(!is_form_url(url), "{url} should not read as a form");
    }
}

/// A privacy policy that happens to mention removal is still a policy.
#[test]
fn a_disqualifying_pattern_beats_every_positive_one() {
    let url = "https://acme.example/privacy-policy/opt-out-and-removal-request";
    assert!(score_form_url(url) < 0);
    assert!(!is_form_url(url));
}

/// Otherwise a link to "/delete-account" reads as an opt-out form.
#[test]
fn a_weak_pattern_alone_is_not_enough() {
    assert_eq!(score_form_url("https://shop.example/cart-remove-item"), 0);
    assert!(score_form_url("https://acme.example/opt-out/remove") > 10);
}

#[test]
fn confirmation_links_are_recognised() {
    for url in [
        "https://acme.example/confirm?token=abc",
        "https://acme.example/verify/123",
        "https://acme.example/activate",
        "https://acme.example/x?code=xyz",
    ] {
        assert!(
            is_confirmation_url(url),
            "{url} should read as a confirmation"
        );
    }
}

#[test]
fn unsubscribe_links_are_kept_separately() {
    let urls = parse_email_urls(&email("https://acme.example/unsubscribe?id=1"));
    assert_eq!(urls.unsubscribe_urls.len(), 1);
}

#[test]
fn tracking_pixels_are_dropped() {
    for url in [
        "https://acme.example/track/open.gif",
        "https://acme.example/pixel.png?pixel=1",
        "https://acme.example/beacon?id=1",
        "https://acme.example/1x1.gif",
    ] {
        assert!(is_tracking_url(url), "{url} should read as tracking");
    }

    let urls = parse_email_urls(&email(
        "https://acme.example/opt-out https://acme.example/track/open.gif",
    ));
    assert_eq!(urls.all_urls, ["https://acme.example/opt-out"]);
}

/// Go's condition read `ends_with(".gif") || ends_with(".png") &&
/// contains("pixel")`, and && binds tighter, so the "pixel" test silently
/// applied only to .png. An ordinary PNG must not be treated as a pixel.
#[test]
fn an_ordinary_image_is_not_treated_as_a_tracking_pixel() {
    assert!(!is_tracking_url("https://acme.example/logo.png"));
    assert!(is_tracking_url("https://acme.example/pixel-1.png"));
}

// -------------------------------------------------------------------
// Choosing the primary link
// -------------------------------------------------------------------

#[test]
fn the_best_scoring_form_link_wins() {
    let urls = ExtractedUrls {
        form_urls: vec![
            "https://acme.example/remove".into(),
            "https://acme.example/do-not-sell-my-info".into(),
        ],
        ..Default::default()
    };

    assert_eq!(
        primary_form_url(&urls, "").as_deref(),
        Some("https://acme.example/do-not-sell-my-info")
    );
}

/// A link back to the broker's own site beats a generic one on some
/// third-party privacy portal.
#[test]
fn a_link_on_the_brokers_own_domain_is_preferred() {
    let urls = ExtractedUrls {
        form_urls: vec![
            "https://privacyportal.example/opt-out-and-removal-request".into(),
            "https://acme.example/optout".into(),
        ],
        ..Default::default()
    };

    assert_eq!(
        primary_form_url(&urls, "acme.example").as_deref(),
        Some("https://acme.example/optout")
    );
}

/// Two equally good links should not swap places between runs.
#[test]
fn ties_resolve_to_the_first_link_in_the_email() {
    let urls = ExtractedUrls {
        form_urls: vec![
            "https://acme.example/optout".into(),
            "https://acme.example/opt-out".into(),
        ],
        ..Default::default()
    };

    let first = primary_form_url(&urls, "");
    for _ in 0..10 {
        assert_eq!(primary_form_url(&urls, ""), first);
    }
    assert_eq!(first.as_deref(), Some("https://acme.example/optout"));
}

#[test]
fn no_form_links_means_no_primary_form() {
    assert!(primary_form_url(&ExtractedUrls::default(), "acme.example").is_none());
}

#[test]
fn the_confirmation_link_prefers_the_brokers_domain() {
    let urls = ExtractedUrls {
        confirmation_urls: vec![
            "https://tracker.example/verify".into(),
            "https://acme.example/confirm?token=abc".into(),
        ],
        ..Default::default()
    };

    assert_eq!(
        primary_confirmation_url(&urls, "acme.example").as_deref(),
        Some("https://acme.example/confirm?token=abc")
    );
    // With no domain to match, the first one is as good a guess as any.
    assert_eq!(
        primary_confirmation_url(&urls, "").as_deref(),
        Some("https://tracker.example/verify")
    );
}

// -------------------------------------------------------------------
// Confirmation tokens
// -------------------------------------------------------------------

#[test]
fn a_token_is_read_out_of_the_query_string() {
    assert_eq!(
        extract_confirmation_token("https://acme.example/confirm?token=abc123").as_deref(),
        Some("abc123")
    );
    assert_eq!(
        extract_confirmation_token("https://acme.example/v?code=xyz789").as_deref(),
        Some("xyz789")
    );
}

#[test]
fn a_token_is_read_out_of_the_path() {
    assert_eq!(
        extract_confirmation_token("https://acme.example/confirm/abcdef1234567890").as_deref(),
        Some("abcdef1234567890")
    );
}

/// A short path segment after /confirm is another path part, not a token.
#[test]
fn a_short_path_segment_is_not_mistaken_for_a_token() {
    assert!(extract_confirmation_token("https://acme.example/confirm/step2").is_none());
}

#[test]
fn a_link_with_no_token_yields_none() {
    assert!(extract_confirmation_token("https://acme.example/confirm").is_none());
    assert!(extract_confirmation_token("not a url").is_none());
}

// -------------------------------------------------------------------
// Bounced recipients
// -------------------------------------------------------------------

/// This is what makes removing dead addresses possible: an address that
/// bounces is one no request will ever reach.
#[test]
fn the_bounced_address_is_read_from_a_delivery_report() {
    let bounce = Email {
        from: "mailer-daemon@googlemail.com".into(),
        subject: "Delivery Status Notification (Failure)".into(),
        body: "The following address had permanent fatal errors: privacy@deadbroker.example".into(),
        ..Default::default()
    };

    assert_eq!(
        extract_bounced_recipient(&bounce).as_deref(),
        Some("privacy@deadbroker.example")
    );
}

#[test]
fn several_bounce_wordings_are_understood() {
    let cases = [
        (
            "Delivery to the following recipient failed: privacy@acme.example",
            "privacy@acme.example",
        ),
        (
            "Final-Recipient: rfc822;privacy@acme.example",
            "privacy@acme.example",
        ),
        (
            "Undeliverable to: privacy@acme.example",
            "privacy@acme.example",
        ),
        (
            "Your message could not be delivered to: privacy@acme.example",
            "privacy@acme.example",
        ),
    ];

    for (body, expected) in cases {
        let bounce = Email {
            body: body.into(),
            ..Default::default()
        };
        assert_eq!(
            extract_bounced_recipient(&bounce).as_deref(),
            Some(expected),
            "for {body:?}"
        );
    }
}

/// Removing your own address from the broker database would be a disaster.
#[test]
fn the_senders_own_address_is_never_reported_as_the_bounced_one() {
    let bounce = Email {
        from: "mailer-daemon@googlemail.com".into(),
        body: "Message from jane@gmail.com to privacy@acme.example failed".into(),
        ..Default::default()
    };

    assert_eq!(
        extract_bounced_recipient(&bounce).as_deref(),
        Some("privacy@acme.example")
    );
}

#[test]
fn a_bounce_naming_no_address_yields_none() {
    let bounce = Email {
        body: "Delivery failed for unknown reasons.".into(),
        ..Default::default()
    };
    assert!(extract_bounced_recipient(&bounce).is_none());
}

// -------------------------------------------------------------------
// HTML handling
// -------------------------------------------------------------------

#[test]
fn tags_are_stripped_leaving_the_text() {
    let text = strip_tags("<p>Hello <b>there</b></p>");
    assert!(text.contains("Hello"));
    assert!(text.contains("there"));
    assert!(!text.contains('<'));
}

#[test]
fn an_email_with_only_html_still_yields_its_text() {
    let email = Email {
        html_body: "<html><body><p>We have removed your data.</p></body></html>".into(),
        ..Default::default()
    };
    assert_eq!(email.text(), "We have removed your data.");
}

#[test]
fn entities_are_decoded_in_the_text() {
    let email = Email {
        html_body: "<p>Smith&nbsp;&amp;&nbsp;Sons said &quot;done&quot;</p>".into(),
        ..Default::default()
    };
    assert_eq!(email.text(), "Smith & Sons said \"done\"");
}

/// The plain part is authoritative when both are present.
#[test]
fn the_plain_body_wins_over_the_html_one() {
    let email = Email {
        body: "plain text".into(),
        html_body: "<p>html text</p>".into(),
        ..Default::default()
    };
    assert_eq!(email.text(), "plain text");
}
