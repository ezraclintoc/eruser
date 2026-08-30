use super::*;

/// A config that passes validate(), for tests that mutate one field at a time.
fn valid_config() -> Config {
    Config {
        profile: Profile {
            first_name: "Jane".into(),
            last_name: "Doe".into(),
            email: "jane@example.com".into(),
            ..Default::default()
        },
        email: EmailConfig {
            provider: "smtp".into(),
            from: "jane@example.com".into(),
            smtp: SmtpConfig {
                host: "smtp.example.com".into(),
                port: 465,
                username: "jane@example.com".into(),
                password: "app-password".into(),
                use_tls: true,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn full_name_joins_first_and_last() {
    assert_eq!(valid_config().profile.full_name(), "Jane Doe");
}

#[test]
fn defaults_fill_template_and_rate_limit() {
    let cfg: Config = serde_norway::from_str("profile:\n  first_name: Jane\n").unwrap();
    assert_eq!(cfg.options.template, DEFAULT_TEMPLATE);
    assert_eq!(cfg.options.rate_limit_ms, DEFAULT_RATE_LIMIT_MS);
}

#[test]
fn apply_defaults_replaces_explicit_zero_and_empty_values() {
    let mut cfg: Config =
        serde_norway::from_str("options:\n  template: \"\"\n  rate_limit_ms: 0\n").unwrap();
    cfg.apply_defaults();
    assert_eq!(cfg.options.template, DEFAULT_TEMPLATE);
    assert_eq!(cfg.options.rate_limit_ms, DEFAULT_RATE_LIMIT_MS);
}

#[test]
fn gmail_provider_implies_imap_server_and_port() {
    let mut cfg: Config =
        serde_norway::from_str("inbox:\n  enabled: true\n  provider: gmail\n").unwrap();
    cfg.apply_defaults();
    assert_eq!(cfg.inbox.server, "imap.gmail.com");
    assert_eq!(cfg.inbox.port, 993);
}

#[test]
fn outlook_provider_implies_imap_server_and_port() {
    let mut cfg: Config =
        serde_norway::from_str("inbox:\n  enabled: true\n  provider: outlook\n").unwrap();
    cfg.apply_defaults();
    assert_eq!(cfg.inbox.server, "outlook.office365.com");
    assert_eq!(cfg.inbox.port, 993);
}

#[test]
fn an_explicit_imap_server_is_not_overwritten() {
    let mut cfg: Config = serde_norway::from_str(
        "inbox:\n  provider: gmail\n  server: imap.self-hosted.example\n  port: 1993\n",
    )
    .unwrap();
    cfg.apply_defaults();
    assert_eq!(cfg.inbox.server, "imap.self-hosted.example");
    assert_eq!(cfg.inbox.port, 1993);
}

#[test]
fn inbox_folder_defaults() {
    let cfg = Config::default();
    assert_eq!(cfg.inbox.folder, "INBOX");
    assert_eq!(cfg.inbox.archive_folder, "Eraser");
}

#[test]
fn browser_defaults_to_headless_with_a_timeout() {
    let cfg = Config::default();
    assert!(cfg.pipeline.browser_headless);
    assert_eq!(
        cfg.pipeline.browser_timeout_sec,
        DEFAULT_BROWSER_TIMEOUT_SECS
    );
}

/// The Go version forced browser_headless = true on every load, which made
/// the config field impossible to turn off from the file. Here it is a real
/// setting that defaults to true.
#[test]
fn headless_can_be_disabled_from_the_config_file() {
    let mut cfg: Config = serde_norway::from_str("pipeline:\n  browser_headless: false\n").unwrap();
    cfg.apply_defaults();
    assert!(!cfg.pipeline.browser_headless);
}

#[test]
fn validate_accepts_a_complete_config() {
    assert!(valid_config().validate().is_ok());
}

#[test]
fn validate_rejects_missing_names() {
    let mut cfg = valid_config();
    cfg.profile.last_name.clear();
    assert_eq!(cfg.validate(), Err(ValidationError::MissingName));
}

#[test]
fn validate_rejects_missing_profile_email() {
    let mut cfg = valid_config();
    cfg.profile.email.clear();
    assert_eq!(cfg.validate(), Err(ValidationError::MissingProfileEmail));
}

#[test]
fn validate_rejects_missing_provider() {
    let mut cfg = valid_config();
    cfg.email.provider.clear();
    assert_eq!(cfg.validate(), Err(ValidationError::MissingProvider));
}

#[test]
fn validate_rejects_unknown_provider() {
    let mut cfg = valid_config();
    cfg.email.provider = "carrier-pigeon".into();
    assert_eq!(
        cfg.validate(),
        Err(ValidationError::UnknownProvider("carrier-pigeon".into()))
    );
}

#[test]
fn validate_rejects_missing_smtp_host_and_port() {
    let mut cfg = valid_config();
    cfg.email.smtp.host.clear();
    assert_eq!(cfg.validate(), Err(ValidationError::MissingSmtpHost));

    let mut cfg = valid_config();
    cfg.email.smtp.port = 0;
    assert_eq!(cfg.validate(), Err(ValidationError::MissingSmtpPort));
}

#[test]
fn validate_inbox_requires_enabled_and_credentials() {
    let mut cfg = valid_config();
    assert_eq!(cfg.validate_inbox(), Err(ValidationError::InboxDisabled));

    cfg.inbox.enabled = true;
    assert_eq!(
        cfg.validate_inbox(),
        Err(ValidationError::MissingInboxEmail)
    );

    cfg.inbox.email = "jane@example.com".into();
    assert_eq!(
        cfg.validate_inbox(),
        Err(ValidationError::MissingInboxPassword)
    );

    cfg.inbox.password = "app-password".into();
    assert_eq!(
        cfg.validate_inbox(),
        Err(ValidationError::MissingInboxServer)
    );

    cfg.inbox.server = "imap.example.com".into();
    assert_eq!(cfg.validate_inbox(), Err(ValidationError::MissingInboxPort));

    cfg.inbox.port = 993;
    assert!(cfg.validate_inbox().is_ok());
}

#[test]
fn save_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.yaml");

    let original = valid_config();
    original.save(&path).unwrap();
    let loaded = Config::load(&path).unwrap();

    assert_eq!(loaded, original);
}

#[cfg(unix)]
#[test]
fn save_writes_owner_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("config.yaml");
    valid_config().save(&path).unwrap();

    let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    let dir_mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600, "config file should be owner-only");
    assert_eq!(dir_mode, 0o700, "config directory should be owner-only");
}

#[cfg(unix)]
#[test]
fn load_refuses_a_world_readable_config() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    valid_config().save(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert!(matches!(
        Config::load(&path).unwrap_err(),
        Error::InsecurePermissions { mode: 0o644, .. }
    ));
}

#[cfg(unix)]
#[test]
fn load_lenient_returns_the_config_and_a_warning() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    valid_config().save(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let (cfg, warning) = Config::load_lenient(&path).unwrap();
    assert_eq!(cfg, valid_config());
    assert!(matches!(
        warning,
        Some(Error::InsecurePermissions { mode: 0o644, .. })
    ));
}

#[test]
fn missing_file_is_a_read_error() {
    assert!(matches!(
        Config::load("/nonexistent/config.yaml").unwrap_err(),
        Error::Read { .. }
    ));
}

#[test]
fn malformed_yaml_is_a_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");
    std::fs::write(&path, "profile: [unclosed").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(matches!(
        Config::load(&path).unwrap_err(),
        Error::Parse { .. }
    ));
}

/// Guard compatibility with the config shipped for the Go version: it must
/// still parse and validate unchanged.
#[test]
fn upstream_example_config_still_parses() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.yaml");
    let data = std::fs::read_to_string(path).unwrap();
    let mut cfg: Config = serde_norway::from_str(&data).unwrap();
    cfg.apply_defaults();

    assert_eq!(cfg.profile.first_name, "John");
    assert_eq!(cfg.email.smtp.port, 465);
    assert_eq!(cfg.options.template, "generic");
    assert!(cfg.validate().is_ok());
}

#[test]
fn optional_profile_fields_are_omitted_when_empty() {
    let yaml = serde_norway::to_string(&valid_config()).unwrap();
    assert!(!yaml.contains("date_of_birth"));
    assert!(!yaml.contains("excluded_brokers"));
    assert!(yaml.contains("first_name"));
}

#[test]
fn default_config_path_lives_under_the_home_directory() {
    let path = default_config_path();
    assert!(path.ends_with("config.yaml"));
    if home_dir().is_some() {
        assert!(path.to_string_lossy().contains(".eraser"));
    }
}

// -------------------------------------------------------------------
// Sending providers
// -------------------------------------------------------------------

/// Upstream documented these in config.example.yaml and pulled both client
/// libraries into go.mod, but its code rejected anything but smtp.
#[test]
fn an_api_provider_validates_with_only_a_key() {
    let mut cfg = valid_config();
    cfg.email.provider = "resend".into();
    cfg.email.smtp = SmtpConfig::default();
    cfg.email.resend.api_key = "re_abc".into();

    assert!(cfg.validate().is_ok(), "no SMTP settings should be needed");
}

#[test]
fn an_api_provider_without_a_key_is_rejected() {
    for provider in ["resend", "sendgrid"] {
        let mut cfg = valid_config();
        cfg.email.provider = provider.into();

        assert_eq!(
            cfg.validate(),
            Err(ValidationError::MissingApiKey(provider)),
            "for {provider}"
        );
    }
}

#[test]
fn smtp_still_requires_a_host_and_port() {
    let mut cfg = valid_config();
    cfg.email.resend.api_key = "re_abc".into();
    cfg.email.smtp.host.clear();

    assert_eq!(
        cfg.validate(),
        Err(ValidationError::MissingSmtpHost),
        "a key for another provider must not excuse missing SMTP settings"
    );
}

#[test]
fn a_provider_that_does_not_exist_is_still_rejected() {
    let mut cfg = valid_config();
    cfg.email.provider = "carrier-pigeon".into();

    assert_eq!(
        cfg.validate(),
        Err(ValidationError::UnknownProvider("carrier-pigeon".into()))
    );
}

#[test]
fn every_advertised_provider_can_be_configured() {
    for provider in EMAIL_PROVIDERS {
        let mut cfg = valid_config();
        cfg.email.provider = (*provider).to_string();
        cfg.email.resend.api_key = "re_abc".into();
        cfg.email.sendgrid.api_key = "SG.abc".into();

        assert!(cfg.validate().is_ok(), "{provider} should be usable");
    }
}

/// An SMTP-only config should not grow empty resend and sendgrid sections.
#[test]
fn unused_provider_sections_are_left_out_of_the_file() {
    let yaml = serde_norway::to_string(&valid_config()).unwrap();

    assert!(!yaml.contains("resend"), "{yaml}");
    assert!(!yaml.contains("sendgrid"), "{yaml}");
}

#[test]
fn an_api_key_round_trips_through_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");

    let mut original = valid_config();
    original.email.provider = "resend".into();
    original.email.resend.api_key = "re_abc123".into();

    original.save(&path).unwrap();
    assert_eq!(Config::load(&path).unwrap(), original);
}
