use super::*;

fn broker(id: &str, region: &str) -> Broker {
    Broker {
        id: id.to_string(),
        name: id.to_string(),
        email: format!("privacy@{id}.example"),
        website: String::new(),
        opt_out_url: String::new(),
        region: region.to_string(),
        category: String::new(),
        notes: String::new(),
        requires_id: false,
        tags: Vec::new(),
    }
}

fn db(brokers: Vec<Broker>) -> BrokerDatabase {
    BrokerDatabase { brokers }
}

fn ids(brokers: &[&Broker]) -> Vec<String> {
    brokers.iter().map(|b| b.id.clone()).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn valid_url_accepts_http_and_https_and_empty() {
    assert!(is_valid_url(""));
    assert!(is_valid_url("http://example.com"));
    assert!(is_valid_url("https://example.com/opt-out"));
    assert!(is_valid_url("HTTPS://EXAMPLE.COM"));
}

#[test]
fn valid_url_rejects_other_schemes_and_junk() {
    assert!(!is_valid_url("javascript:alert(1)"));
    assert!(!is_valid_url("file:///etc/passwd"));
    assert!(!is_valid_url("ftp://example.com"));
    assert!(!is_valid_url("example.com"));
    assert!(!is_valid_url("https://"));
}

#[test]
fn sanitize_clears_non_http_urls_but_keeps_valid_ones() {
    let mut b = broker("evil", "us");
    b.website = "javascript:alert(1)".to_string();
    b.opt_out_url = "https://example.com/optout".to_string();
    b.sanitize();
    assert_eq!(b.website, "");
    assert_eq!(b.opt_out_url, "https://example.com/optout");
}

#[test]
fn filter_with_no_regions_returns_everything() {
    let d = db(vec![broker("a", "us"), broker("b", "eu")]);
    assert_eq!(ids(&d.filter(&[], &[])), ["a", "b"]);
}

#[test]
fn filter_by_region_keeps_matching_and_global() {
    let d = db(vec![
        broker("a", "us"),
        broker("b", "eu"),
        broker("c", "global"),
    ]);
    assert_eq!(ids(&d.filter(&strings(&["us"]), &[])), ["a", "c"]);
    assert_eq!(ids(&d.filter(&strings(&["eu"]), &[])), ["b", "c"]);
}

#[test]
fn filter_region_match_is_case_insensitive() {
    let d = db(vec![broker("a", "US"), broker("b", "eu")]);
    assert_eq!(ids(&d.filter(&strings(&["us"]), &[])), ["a"]);
}

#[test]
fn filter_excludes_by_id_and_by_name_case_insensitively() {
    let mut only_name = broker("keep-me", "us");
    only_name.name = "Excluded By Name".to_string();
    let d = db(vec![
        broker("spokeo", "us"),
        only_name,
        broker("stay", "us"),
    ]);

    let filtered = d.filter(&[], &strings(&["SPOKEO", "excluded by name"]));
    assert_eq!(ids(&filtered), ["stay"]);
}

#[test]
fn exclusion_wins_over_region_match() {
    let d = db(vec![broker("a", "us"), broker("b", "us")]);
    assert_eq!(ids(&d.filter(&strings(&["us"]), &strings(&["a"]))), ["b"]);
}

#[test]
fn find_by_id_and_email_are_case_insensitive() {
    let d = db(vec![broker("acme", "us")]);
    assert_eq!(d.find_by_id("ACME").map(|b| b.id.as_str()), Some("acme"));
    assert_eq!(
        d.find_by_email("PRIVACY@ACME.EXAMPLE")
            .map(|b| b.id.as_str()),
        Some("acme")
    );
    assert!(d.find_by_id("nope").is_none());
    assert!(d.find_by_email("nope@example.com").is_none());
}

#[test]
fn add_rejects_duplicate_id() {
    let mut d = db(vec![broker("acme", "us")]);
    let err = d.add(broker("acme", "eu")).unwrap_err();
    assert!(matches!(err, Error::DuplicateId(id) if id == "acme"));
    assert_eq!(d.brokers.len(), 1);
}

#[test]
fn add_appends_new_broker() {
    let mut d = db(vec![broker("acme", "us")]);
    d.add(broker("other", "eu")).unwrap();
    assert_eq!(d.brokers.len(), 2);
}

#[test]
fn remove_by_id_returns_the_removed_broker() {
    let mut d = db(vec![broker("a", "us"), broker("b", "us")]);
    let removed = d.remove_by_id("A").expect("broker should exist");
    assert_eq!(removed.id, "a");
    assert_eq!(ids(&d.filter(&[], &[])), ["b"]);
    assert!(d.remove_by_id("a").is_none());
}

#[test]
fn remove_by_email_returns_the_removed_broker() {
    let mut d = db(vec![broker("a", "us"), broker("b", "us")]);
    let removed = d
        .remove_by_email("PRIVACY@A.EXAMPLE")
        .expect("broker should exist");
    assert_eq!(removed.id, "a");
    assert_eq!(d.brokers.len(), 1);
}

#[test]
fn round_trips_through_yaml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brokers.yaml");

    let mut original = broker("acme", "us");
    original.website = "https://acme.example".to_string();
    original.category = "marketing".to_string();
    original.requires_id = true;
    original.tags = strings(&["slow", "verified"]);

    db(vec![original.clone()]).save(&path).unwrap();
    let loaded = BrokerDatabase::load_from_file(&path).unwrap();

    assert_eq!(loaded.brokers, vec![original]);
}

#[test]
fn load_sanitizes_urls_on_the_way_in() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brokers.yaml");
    std::fs::write(
        &path,
        "brokers:\n  - id: evil\n    name: Evil\n    email: a@b.example\n    region: us\n    website: \"javascript:alert(1)\"\n",
    )
    .unwrap();

    let loaded = BrokerDatabase::load_from_file(&path).unwrap();
    assert_eq!(loaded.brokers[0].website, "");
}

#[test]
fn save_with_backup_preserves_the_previous_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brokers.yaml");

    db(vec![broker("first", "us")]).save(&path).unwrap();
    db(vec![broker("second", "us")])
        .save_with_backup(&path)
        .unwrap();

    let current = BrokerDatabase::load_from_file(&path).unwrap();
    let backup = BrokerDatabase::load_from_file(dir.path().join("brokers.yaml.bak")).unwrap();
    assert_eq!(current.brokers[0].id, "second");
    assert_eq!(backup.brokers[0].id, "first");
}

#[test]
fn load_from_dir_concatenates_yaml_files_in_sorted_order() {
    let dir = tempfile::tempdir().unwrap();
    db(vec![broker("b-second", "us")])
        .save(dir.path().join("20.yaml"))
        .unwrap();
    db(vec![broker("a-first", "us")])
        .save(dir.path().join("10.yml"))
        .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

    let loaded = BrokerDatabase::load_from_dir(dir.path()).unwrap();
    assert_eq!(ids(&loaded.filter(&[], &[])), ["a-first", "b-second"]);
}

#[test]
fn missing_file_is_a_read_error() {
    let err = BrokerDatabase::load_from_file("/nonexistent/brokers.yaml").unwrap_err();
    assert!(matches!(err, Error::Read { .. }));
}

#[test]
fn malformed_yaml_is_a_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("brokers.yaml");
    std::fs::write(&path, "brokers: [ this is not: valid").unwrap();
    assert!(matches!(
        BrokerDatabase::load_from_file(&path).unwrap_err(),
        Error::Parse { .. }
    ));
}

/// The shipped database is the product; guard its shape and its size.
#[test]
fn bundled_broker_database_loads_and_is_well_formed() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/data/brokers.yaml");
    let loaded = BrokerDatabase::load_from_file(path).unwrap();

    assert!(
        loaded.brokers.len() >= 750,
        "expected 750+ brokers, found {}",
        loaded.brokers.len()
    );

    let mut seen = std::collections::HashSet::new();
    for b in &loaded.brokers {
        assert!(!b.id.is_empty(), "broker {:?} has no id", b.name);
        assert!(!b.name.is_empty(), "broker {:?} has no name", b.id);
        assert!(!b.email.is_empty(), "broker {:?} has no email", b.id);
        assert!(
            b.email.contains('@'),
            "broker {:?} has a malformed email {:?}",
            b.id,
            b.email
        );
        assert!(
            seen.insert(b.id.to_lowercase()),
            "duplicate broker id {:?}",
            b.id
        );
    }
}
