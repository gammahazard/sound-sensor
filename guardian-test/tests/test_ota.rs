use guardian_test::ota::*;

// ── is_newer ─────────────────────────────────────────────────────────────────

#[test]
fn is_newer_basic() {
    assert!(is_newer("0.1.0", "0.2.0"));
}

#[test]
fn is_newer_with_v() {
    assert!(is_newer("0.1.0", "v0.2.0"));
}

#[test]
fn is_newer_equal() {
    assert!(!is_newer("0.1.0", "0.1.0"));
}

#[test]
fn is_newer_patch() {
    assert!(is_newer("0.1.0", "0.1.1"));
}

#[test]
fn is_newer_major() {
    assert!(is_newer("1.9.9", "2.0.0"));
}

#[test]
fn is_newer_older() {
    assert!(!is_newer("0.2.0", "0.1.0"));
}

#[test]
fn is_newer_both_v() {
    assert!(is_newer("v0.1.0", "v0.2.0"));
}

#[test]
fn is_newer_major_only() {
    assert!(is_newer("1", "2"));
}

// ── parse_tag_name ───────────────────────────────────────────────────────────

#[test]
fn parse_tag_basic() {
    let json = r#"{"tag_name":"v0.2.0","name":"Release 0.2.0"}"#;
    assert_eq!(parse_tag_name(json), Some("v0.2.0"));
}

#[test]
fn parse_tag_missing() {
    let json = r#"{"name":"foo"}"#;
    assert_eq!(parse_tag_name(json), None);
}

#[test]
fn parse_tag_with_spaces() {
    let json = r#"{"tag_name": "v1.0.0"}"#;
    assert_eq!(parse_tag_name(json), Some("v1.0.0"));
}

#[test]
fn parse_tag_empty() {
    let json = r#"{"tag_name":""}"#;
    assert_eq!(parse_tag_name(json), Some(""));
}

// ── status_json ──────────────────────────────────────────────────────────────

#[test]
fn status_json_checking() {
    let s = status_json(true, false, "0.1.0", "0.1.0", false, "0.3.0");
    let json = s.as_str();
    assert!(json.contains(r#""evt":"ota_status""#));
    assert!(json.contains(r#""checking":true"#));
    assert!(json.contains(r#""available":false"#));
    assert!(json.contains(r#""fw":"0.3.0""#));
}

#[test]
fn status_json_available() {
    let s = status_json(false, true, "0.1.0", "0.2.0", false, "0.3.0");
    let json = s.as_str();
    assert!(json.contains(r#""available":true"#));
    assert!(json.contains(r#""current":"0.1.0""#));
    assert!(json.contains(r#""latest":"0.2.0""#));
}

#[test]
fn status_json_done() {
    let s = status_json(false, false, "0.1.0", "0.2.0", true, "0.3.0");
    let json = s.as_str();
    assert!(json.contains(r#""evt":"ota_done""#));
    assert!(json.contains(r#""pwa":"0.2.0""#));
    assert!(json.contains(r#""fw":"0.3.0""#));
    // Should NOT contain ota_status fields
    assert!(!json.contains(r#""checking""#));
}
