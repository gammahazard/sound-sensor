//! tv_brand.rs — TvBrand enum extracted from firmware tv.rs

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TvBrand {
    Lg,
    Samsung,
    Sony,
    Roku,
}

impl TvBrand {
    pub fn supports_absolute_volume(self) -> bool {
        matches!(self, TvBrand::Lg | TvBrand::Sony)
    }

    pub fn default_port(self) -> u16 {
        match self {
            TvBrand::Lg => 3000,
            TvBrand::Samsung => 8001,
            TvBrand::Sony => 80,
            TvBrand::Roku => 8060,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "lg" | "webos" | "lge" => Some(TvBrand::Lg),
            "samsung" => Some(TvBrand::Samsung),
            "sony" | "bravia" => Some(TvBrand::Sony),
            "roku" => Some(TvBrand::Roku),
            _ => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            TvBrand::Lg => 0,
            TvBrand::Samsung => 1,
            TvBrand::Sony => 2,
            TvBrand::Roku => 3,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => TvBrand::Samsung,
            2 => TvBrand::Sony,
            3 => TvBrand::Roku,
            _ => TvBrand::Lg,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TvBrand::Lg => "lg",
            TvBrand::Samsung => "samsung",
            TvBrand::Sony => "sony",
            TvBrand::Roku => "roku",
        }
    }
}
