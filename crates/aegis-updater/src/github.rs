//! Releases von GitHub holen und die signierten Manifeste prüfen.

use std::collections::HashMap;

use aegis_update::{decide, verify, Decision, Manifest, UpdateError, Version};
use serde::Deserialize;

use crate::fetch::Fetch;

/// Einzige Quelle der Release-Liste.
pub const RELEASES_URL: &str = "https://api.github.com/repos/fabianh854-sketch/Aegis/releases";
/// Einzige erlaubte Herkunft von Release-Dateien.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/fabianh854-sketch/Aegis/releases/download/";
pub const MANIFEST_ASSET: &str = "aegis-manifest.json";
pub const SIGNATURE_ASSET: &str = "aegis-manifest.json.minisig";

/// Ein veröffentlichtes Release: Tag-Version und erlaubte Download-URLs je Dateiname.
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseRef {
    pub tag: Version,
    /// Dateiname → `browser_download_url`
    pub assets: HashMap<String, String>,
}

/// Prüft, ob eine URL von der festen Release-Herkunft stammt.
pub fn is_allowed_url(url: &str) -> bool {
    url.starts_with(DOWNLOAD_PREFIX)
        && !url.contains("..")
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Höchstens so viele Releases (die neuesten über der installierten Version) werden geprüft.
const MAX_CANDIDATES: usize = 5;

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

/// Liest die Antwort von `/releases`. Entwürfe, Vorabversionen und Tags, die keine
/// Version sind, fallen weg; Dateien nur mit URL von der festen Release-Herkunft.
pub fn parse_releases(json: &[u8]) -> Result<Vec<ReleaseRef>, UpdateError> {
    let releases: Vec<ApiRelease> = serde_json::from_slice(json)
        .map_err(|e| UpdateError::BadFormat(format!("Release-Liste: {e}")))?;
    Ok(releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            let tag = r.tag_name.parse::<Version>().ok()?;
            let assets = r
                .assets
                .into_iter()
                .filter(|a| is_allowed_url(&a.browser_download_url))
                .map(|a| (a.name, a.browser_download_url))
                .collect();
            Some(ReleaseRef { tag, assets })
        })
        .collect())
}

/// Lädt Manifest und Signatur eines Releases und prüft sie.
/// Außen: Netzfehler (Prüfung abbrechen); innen: Ergebnis der Prüfung für [`decide`].
fn load_manifest(
    fetch: &dyn Fetch,
    release: &ReleaseRef,
    keys: &[&str],
    now: u64,
) -> Result<Result<Manifest, UpdateError>, UpdateError> {
    let (Some(manifest_url), Some(sig_url)) = (
        release.assets.get(MANIFEST_ASSET),
        release.assets.get(SIGNATURE_ASSET),
    ) else {
        return Ok(Err(UpdateError::BadFormat(
            "Manifest oder Signatur fehlt im Release".into(),
        )));
    };
    let json = fetch.get(manifest_url)?;
    let sig = fetch.get(sig_url)?;
    let Ok(sig) = String::from_utf8(sig) else {
        return Ok(Err(UpdateError::BadSignature));
    };
    Ok(verify(&json, &sig, keys, now))
}

/// Prüfung samt der zugehörigen Releases (für die Installation).
///
/// Geprüft werden nur Releases über `installed` (die anderen kann [`decide`] weder anbieten
/// noch ablehnen), absteigend nach Tag-Version, höchstens [`MAX_CANDIDATES`].
pub fn check_releases(
    fetch: &dyn Fetch,
    installed: Version,
    keys: &[&str],
    now: u64,
) -> Result<(Decision, Vec<ReleaseRef>), UpdateError> {
    let json = fetch.get(RELEASES_URL)?;
    let mut releases: Vec<ReleaseRef> = parse_releases(&json)?
        .into_iter()
        .filter(|r| r.tag > installed)
        .collect();
    releases.sort_by_key(|r| std::cmp::Reverse(r.tag));
    releases.truncate(MAX_CANDIDATES);

    let mut candidates = Vec::with_capacity(releases.len());
    for release in &releases {
        candidates.push((release.tag, load_manifest(fetch, release, keys, now)?));
    }
    Ok((decide(installed, candidates, now), releases))
}

/// Holt Releases und Manifeste und entscheidet über ein Update-Angebot.
pub fn check(
    fetch: &dyn Fetch,
    installed: Version,
    keys: &[&str],
    now: u64,
) -> Result<Decision, UpdateError> {
    check_releases(fetch, installed, keys, now).map(|(decision, _)| decision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use serde_json::json;

    #[test]
    fn parse_releases_skips_drafts_prereleases_and_foreign_urls() {
        let good = asset_url("v0.2.0", "aegis.exe");
        let json = serde_json::to_vec(&json!([
            release_entry(
                "v0.2.0",
                false,
                false,
                &[
                    ("aegis.exe", good.clone()),
                    ("evil.exe", "https://evil.example/aegis.exe".into()),
                    ("http.exe", good.replace("https://", "http://")),
                    (
                        "other.exe",
                        "https://github.com/someone-else/Aegis/releases/download/v0.2.0/x".into()
                    ),
                    ("dots.exe", format!("{DOWNLOAD_PREFIX}../../../x")),
                ]
            ),
            release_entry(
                "v0.3.0",
                true,
                false,
                &[("aegis.exe", asset_url("v0.3.0", "aegis.exe"))]
            ),
            release_entry(
                "v0.4.0",
                false,
                true,
                &[("aegis.exe", asset_url("v0.4.0", "aegis.exe"))]
            ),
            release_entry("v0.5.0-beta", false, false, &[]),
            release_entry("nightly", false, false, &[]),
            release_entry("0.1.1", false, false, &[]),
        ]))
        .unwrap();
        let releases = parse_releases(&json).unwrap();
        assert_eq!(releases.len(), 2, "{releases:?}");
        assert_eq!(releases[0].tag, v("0.2.0"));
        assert_eq!(
            releases[0].assets,
            HashMap::from([("aegis.exe".to_string(), good)])
        );
        assert_eq!(releases[1].tag, v("0.1.1"));
        assert!(releases[1].assets.is_empty());
    }

    #[test]
    fn parse_releases_rejects_garbage() {
        assert!(matches!(
            parse_releases(b"{\"message\":\"rate limit\"}"),
            Err(UpdateError::BadFormat(_))
        ));
        assert!(matches!(
            parse_releases(b"<html>"),
            Err(UpdateError::BadFormat(_))
        ));
    }

    #[test]
    fn check_available_end_to_end_with_fake_fetch() {
        let key = Key::new();
        let mut fetch = FakeFetch::default();
        let entries = vec![
            publish(&mut fetch, &key, "v0.2.0"),
            publish(&mut fetch, &key, "v0.3.0"),
            release_entry("v0.1.0", false, false, &[]),
        ];
        set_releases(&mut fetch, entries);
        let decision = check(&fetch, v("0.1.0"), &[&key.public()], NOW).unwrap();
        match decision {
            Decision::Available { manifest, security } => {
                assert_eq!(manifest, test_manifest("0.3.0"));
                assert!(!security);
            }
            other => panic!("unerwartet: {other:?}"),
        }

        let (_, releases) = check_releases(&fetch, v("0.1.0"), &[&key.public()], NOW).unwrap();
        assert_eq!(releases[0].tag, v("0.3.0"), "absteigend sortiert");
    }

    #[test]
    fn check_none_when_no_release_newer() {
        let key = Key::new();
        let mut fetch = FakeFetch::default();
        let entries = vec![
            publish(&mut fetch, &key, "v0.2.0"),
            release_entry("v0.1.0", false, false, &[]),
        ];
        set_releases(&mut fetch, entries);
        assert_eq!(
            check(&fetch, v("0.2.0"), &[&key.public()], NOW),
            Ok(Decision::UpToDate)
        );
        // Ohne vertrauenswürdige Schlüssel gibt es nichts zu prüfen, was nicht neuer ist.
        assert_eq!(check(&fetch, v("0.2.0"), &[], NOW), Ok(Decision::UpToDate));
    }

    #[test]
    fn check_rejected_when_newer_signature_bad() {
        let key = Key::new();
        let attacker = Key::new();
        let mut fetch = FakeFetch::default();
        let entries = vec![
            publish(&mut fetch, &attacker, "v0.3.0"),
            release_entry("v0.1.0", false, false, &[]),
        ];
        set_releases(&mut fetch, entries);
        assert!(matches!(
            check(&fetch, v("0.1.0"), &[&key.public()], NOW),
            Ok(Decision::Rejected { version, .. }) if version == v("0.3.0")
        ));
        // Leere Schlüsselliste: neueres Release wird ebenfalls abgelehnt.
        assert!(matches!(
            check(&fetch, v("0.1.0"), &[], NOW),
            Ok(Decision::Rejected { version, .. }) if version == v("0.3.0")
        ));
    }

    #[test]
    fn check_rejected_when_newer_release_has_no_manifest() {
        let key = Key::new();
        let mut fetch = FakeFetch::default();
        let entries = vec![release_entry(
            "v0.3.0",
            false,
            false,
            &[("aegis.exe", asset_url("v0.3.0", "aegis.exe"))],
        )];
        set_releases(&mut fetch, entries);
        assert!(matches!(
            check(&fetch, v("0.1.0"), &[&key.public()], NOW),
            Ok(Decision::Rejected { version, .. }) if version == v("0.3.0")
        ));
    }

    #[test]
    fn check_network_error_is_error() {
        let fetch = FakeFetch::default();
        assert!(matches!(
            check(&fetch, v("0.1.0"), &[], NOW),
            Err(UpdateError::Network(_))
        ));
    }

    #[test]
    fn check_fetches_at_most_five_newest_manifests() {
        let key = Key::new();
        let mut fetch = FakeFetch::default();
        let mut entries = Vec::new();
        for minor in 2..=8 {
            entries.push(publish(&mut fetch, &key, &format!("v0.{minor}.0")));
        }
        set_releases(&mut fetch, entries);
        let counting = CountingFetch::new(&fetch);
        let decision = check(&counting, v("0.1.0"), &[&key.public()], NOW).unwrap();
        assert!(matches!(
            decision,
            Decision::Available { manifest, .. } if manifest.version == v("0.8.0")
        ));
        let manifests = counting.requested(MANIFEST_ASSET);
        assert_eq!(manifests.len(), 5, "{manifests:?}");
        assert!(!manifests
            .iter()
            .any(|u| u.contains("v0.2.0") || u.contains("v0.3.0")));
    }
}
