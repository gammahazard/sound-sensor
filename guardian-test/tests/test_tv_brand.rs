use guardian_test::tv_brand::TvBrand;

// ── parse ──────────────────────────────────────────────────────────────────

#[test]
fn parse_lg_aliases() {
    assert_eq!(TvBrand::parse("lg"), Some(TvBrand::Lg));
    assert_eq!(TvBrand::parse("webos"), Some(TvBrand::Lg));
    assert_eq!(TvBrand::parse("lge"), Some(TvBrand::Lg));
}

#[test]
fn parse_samsung() {
    assert_eq!(TvBrand::parse("samsung"), Some(TvBrand::Samsung));
}

#[test]
fn parse_sony_aliases() {
    assert_eq!(TvBrand::parse("sony"), Some(TvBrand::Sony));
    assert_eq!(TvBrand::parse("bravia"), Some(TvBrand::Sony));
}

#[test]
fn parse_roku() {
    assert_eq!(TvBrand::parse("roku"), Some(TvBrand::Roku));
}

#[test]
fn parse_unknown() {
    assert_eq!(TvBrand::parse("vizio"), None);
    assert_eq!(TvBrand::parse(""), None);
    assert_eq!(TvBrand::parse("LG"), None); // case sensitive
}

// ── u8 roundtrip ───────────────────────────────────────────────────────────

#[test]
fn u8_roundtrip_all() {
    for brand in [TvBrand::Lg, TvBrand::Samsung, TvBrand::Sony, TvBrand::Roku] {
        assert_eq!(TvBrand::from_u8(brand.to_u8()), brand);
    }
}

#[test]
fn from_u8_unknown_defaults_lg() {
    assert_eq!(TvBrand::from_u8(255), TvBrand::Lg);
    assert_eq!(TvBrand::from_u8(4), TvBrand::Lg);
}

// ── default_port ───────────────────────────────────────────────────────────

#[test]
fn default_ports() {
    assert_eq!(TvBrand::Lg.default_port(), 3000);
    assert_eq!(TvBrand::Samsung.default_port(), 8001);
    assert_eq!(TvBrand::Sony.default_port(), 80);
    assert_eq!(TvBrand::Roku.default_port(), 8060);
}

// ── supports_absolute_volume ───────────────────────────────────────────────

#[test]
fn absolute_volume_lg_sony() {
    assert!(TvBrand::Lg.supports_absolute_volume());
    assert!(TvBrand::Sony.supports_absolute_volume());
}

#[test]
fn no_absolute_volume_samsung_roku() {
    assert!(!TvBrand::Samsung.supports_absolute_volume());
    assert!(!TvBrand::Roku.supports_absolute_volume());
}

// ── as_str ─────────────────────────────────────────────────────────────────

#[test]
fn as_str_all() {
    assert_eq!(TvBrand::Lg.as_str(), "lg");
    assert_eq!(TvBrand::Samsung.as_str(), "samsung");
    assert_eq!(TvBrand::Sony.as_str(), "sony");
    assert_eq!(TvBrand::Roku.as_str(), "roku");
}
