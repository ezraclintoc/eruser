use super::*;

#[test]
fn a_page_with_nothing_on_it_reports_nothing() {
    assert!(detect_in_html("<html><body><p>Thank you.</p></body></html>").is_none());
    assert!(detect_in_html("").is_none());
}

#[test]
fn the_named_providers_are_recognised() {
    let cases = [
        (
            r#"<div class="h-captcha" data-sitekey="x"></div>"#,
            CaptchaKind::HCaptcha,
        ),
        (
            r#"<div class="cf-turnstile" data-sitekey="x"></div>"#,
            CaptchaKind::Turnstile,
        ),
        (
            r#"<script src="https://client-api.arkoselabs.com/v2/api.js"></script>"#,
            CaptchaKind::FunCaptcha,
        ),
        (
            r#"<title>Just a moment...</title><p>Checking your browser before accessing.</p>"#,
            CaptchaKind::CloudflareChallenge,
        ),
    ];

    for (html, expected) in cases {
        let found = detect_in_html(html).unwrap_or_else(|| panic!("nothing found in {html}"));
        assert_eq!(found.kind, expected, "for {html}");
        assert!(found.confidence >= 0.85);
    }
}

#[test]
fn a_visible_recaptcha_is_recognised_as_version_two() {
    let found = detect_in_html(r#"<div class="g-recaptcha" data-sitekey="abc"></div>"#).unwrap();

    assert_eq!(found.kind, CaptchaKind::RecaptchaV2);
    assert!(found.blocks_automation());
}

/// Go reported every reCAPTCHA as v2, so a page using the invisible v3 was
/// called a blocking challenge and handed to a person for nothing.
#[test]
fn an_invisible_recaptcha_is_recognised_as_version_three() {
    let cases = [
        r#"<script src="https://www.google.com/recaptcha/api.js?render=SITEKEY"></script>"#,
        r#"<script>grecaptcha.execute('key', {action: 'submit'});</script>"#,
    ];

    for html in cases {
        let found = detect_in_html(html).unwrap_or_else(|| panic!("nothing found in {html}"));
        assert_eq!(found.kind, CaptchaKind::RecaptchaV3, "for {html}");
        assert!(
            !found.blocks_automation(),
            "v3 scores in the background and should not stop a fill"
        );
    }
}

#[test]
fn an_image_captcha_is_recognised() {
    let found = detect_in_html(r#"<img src="/captcha_image.php" alt="captcha">"#).unwrap();
    assert_eq!(found.kind, CaptchaKind::ImageCaptcha);
}

/// Better to say "something is asking you to prove you are human" than to
/// miss it and report the form as filled.
#[test]
fn generic_wording_is_reported_with_lower_confidence() {
    for html in [
        "<p>Please enter the verification code below.</p>",
        "<p>Prove you are human to continue.</p>",
        "<label>Security code</label>",
    ] {
        let found = detect_in_html(html).unwrap_or_else(|| panic!("nothing found in {html}"));
        assert_eq!(found.kind, CaptchaKind::Unknown, "for {html}");
        assert!(found.confidence < 0.85, "for {html}");
        assert!(found.blocks_automation());
    }
}

#[test]
fn detection_ignores_case() {
    assert!(detect_in_html(r#"<DIV CLASS="G-RECAPTCHA"></DIV>"#).is_some());
    assert!(detect_in_html("<P>CAPTCHA</P>").is_some());
}

/// A specific provider should be named rather than falling through to the
/// generic sweep, since the instructions differ.
#[test]
fn a_named_provider_wins_over_the_generic_sweep() {
    let html = r#"<p>Please complete the captcha</p><div class="h-captcha"></div>"#;
    assert_eq!(detect_in_html(html).unwrap().kind, CaptchaKind::HCaptcha);
}

#[test]
fn the_matched_text_is_reported_for_debugging() {
    let found = detect_in_html(r#"<div class="cf-turnstile"></div>"#).unwrap();
    assert_eq!(found.matched, "cf-turnstile");
}

#[test]
fn every_kind_has_instructions_and_a_name() {
    for kind in [
        CaptchaKind::RecaptchaV2,
        CaptchaKind::RecaptchaV3,
        CaptchaKind::HCaptcha,
        CaptchaKind::Turnstile,
        CaptchaKind::FunCaptcha,
        CaptchaKind::ImageCaptcha,
        CaptchaKind::TextCaptcha,
        CaptchaKind::CloudflareChallenge,
        CaptchaKind::Unknown,
    ] {
        assert!(!kind.as_str().is_empty());
        assert!(!kind.instructions().is_empty(), "{kind}");
    }
}

/// Only v3 passes on its own; everything else needs a person.
#[test]
fn only_the_invisible_one_lets_automation_continue() {
    assert!(!CaptchaKind::RecaptchaV3.blocks_automation());

    for kind in [
        CaptchaKind::RecaptchaV2,
        CaptchaKind::HCaptcha,
        CaptchaKind::Turnstile,
        CaptchaKind::FunCaptcha,
        CaptchaKind::ImageCaptcha,
        CaptchaKind::TextCaptcha,
        CaptchaKind::CloudflareChallenge,
        CaptchaKind::Unknown,
    ] {
        assert!(kind.blocks_automation(), "{kind}");
    }
}
