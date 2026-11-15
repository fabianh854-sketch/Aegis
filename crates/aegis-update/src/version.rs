//! Semantische Version `MAJOR.MINOR.PATCH` ohne Zusätze.

use crate::manifest::UpdateError;
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// Programmversion; die Ableitung der Ordnung folgt der Feldreihenfolge.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
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

/// Serialisiert als String "MAJOR.MINOR.PATCH".
impl Serialize for Version {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Liest nur, was `FromStr` akzeptiert (strikt, Objektform wird abgelehnt).
impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(de::Error::custom)
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
    fn version_serializes_as_string() {
        let v: Version = "0.1.1".parse().unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), "\"0.1.1\"");
        assert_eq!(serde_json::from_str::<Version>("\"0.1.1\"").unwrap(), v);
        assert_eq!(serde_json::from_str::<Version>("\"v0.1.1\"").unwrap(), v);
        for bad in [
            "\"0.1\"",
            "\"0.1.1-beta\"",
            "\"\"",
            "1",
            "null",
            "{\"major\":0,\"minor\":1,\"patch\":1}",
        ] {
            assert!(
                serde_json::from_str::<Version>(bad).is_err(),
                "{bad} muss abgelehnt werden"
            );
        }
    }

    #[test]
    fn manifest_with_string_versions_roundtrips() {
        let json = format!(
            r#"{{"schema":1,"version":"0.1.1","security":false,"min_secure":"0.1.0","issued_at":10,"expires_at":{},"files":[{{"name":"aegis.exe","sha256":"{h}"}},{{"name":"aegis-updater.exe","sha256":"{h}"}}]}}"#,
            10 + crate::VALIDITY_SECS,
            h = "a".repeat(64)
        );
        let m: crate::Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.version.to_string(), "0.1.1");
        assert_eq!(m.min_secure.to_string(), "0.1.0");
        assert_eq!(serde_json::to_string(&m).unwrap(), json);
        let object_form = json.replace("\"0.1.1\"", r#"{"major":0,"minor":1,"patch":1}"#);
        assert!(serde_json::from_str::<crate::Manifest>(&object_form).is_err());
    }

    #[test]
    fn installed_matches_package_version() {
        assert_eq!(installed().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
