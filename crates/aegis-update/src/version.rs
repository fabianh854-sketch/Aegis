//! Semantische Version `MAJOR.MINOR.PATCH` ohne Zusätze.

use crate::manifest::UpdateError;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Programmversion; die Ableitung der Ordnung folgt der Feldreihenfolge.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// Eine Versionskomponente: nur ASCII-Ziffern, keine führende Null (außer "0").
fn component(s: &str, whole: &str) -> Result<u32, UpdateError> {
    let bad = || UpdateError::BadFormat(format!("ungültige Version: {whole:?}"));
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return Err(bad());
    }
    s.parse().map_err(|_| bad())
}

impl FromStr for Version {
    type Err = UpdateError;

    /// Nur "MAJOR.MINOR.PATCH", optional mit führendem "v".
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s.strip_prefix('v').unwrap_or(s);
        let parts: Vec<&str> = body.split('.').collect();
        let [major, minor, patch] = parts[..] else {
            return Err(UpdateError::BadFormat(format!("ungültige Version: {s:?}")));
        };
        Ok(Version {
            major: component(major, s)?,
            minor: component(minor, s)?,
            patch: component(patch, s)?,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Version dieses Builds (aus `Cargo.toml`).
pub fn installed() -> Version {
    env!("CARGO_PKG_VERSION")
        .parse()
        .expect("CARGO_PKG_VERSION ist MAJOR.MINOR.PATCH")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parse_and_order() {
        assert!("v0.1.10".parse::<Version>().unwrap() > "0.1.9".parse().unwrap());
        assert!("0.1".parse::<Version>().is_err());
        assert!("0.1.1-beta".parse::<Version>().is_err());
    }

    #[test]
    fn version_parse_is_strict() {
        for bad in [
            "",
            "v",
            "1.2.3.4",
            "+1.2.3",
            "1.-2.3",
            "1..3",
            " 1.2.3",
            "1.2.3 ",
            "01.2.3",
            "V1.2.3",
            "99999999999.0.0",
        ] {
            assert!(
                bad.parse::<Version>().is_err(),
                "{bad:?} muss abgelehnt werden"
            );
        }
    }

    #[test]
    fn version_display_roundtrip() {
        let v: Version = "v0.1.1".parse().unwrap();
        assert_eq!(v.to_string(), "0.1.1");
        assert_eq!(
            v,
            Version {
                major: 0,
                minor: 1,
                patch: 1
            }
        );
    }

    #[test]
    fn installed_matches_package_version() {
        assert_eq!(installed().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
