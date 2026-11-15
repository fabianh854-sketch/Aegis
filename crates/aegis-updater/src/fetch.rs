//! Netzwerkzugriff: ausschließlich über `C:\Windows\System32\curl.exe`.

use aegis_update::UpdateError;

/// Lädt eine URL vollständig in den Speicher.
pub trait Fetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, UpdateError>;
}

/// Absoluter Pfad, damit kein fremdes `curl.exe` aus PATH oder dem Arbeitsordner startet.
const CURL_EXE: &str = r"C:\Windows\System32\curl.exe";
/// Obergrenze je Download: 64 MiB.
const MAX_FILESIZE: &str = "67108864";
/// Nur für die GitHub-API werden die API-Header gesendet.
const API_PREFIX: &str = "https://api.github.com/";

/// Lädt über das System-`curl.exe` (Schannel, nur HTTPS).
pub struct Curl;

/// Argumente für curl; die URL ist ein eigenes Argument nach `--url`, nie über eine Shell.
pub(crate) fn curl_args(url: &str) -> Vec<String> {
    let mut args = vec![
        // Muss das erste Argument sein: keine .curlrc des Nutzers lesen.
        "-q",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--max-time",
        "60",
        "--max-filesize",
        MAX_FILESIZE,
    ];
    if url.starts_with(API_PREFIX) {
        args.extend([
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "User-Agent: aegis-updater",
        ]);
    }
    let mut args: Vec<String> = args.into_iter().map(String::from).collect();
    args.push("--url".into());
    args.push(url.into());
    args
}

impl Fetch for Curl {
    fn get(&self, url: &str) -> Result<Vec<u8>, UpdateError> {
        let mut cmd = std::process::Command::new(CURL_EXE);
        cmd.args(curl_args(url)).stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // Kein Konsolenfenster für curl, der Updater läuft ohne Konsole.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let output = cmd
            .output()
            .map_err(|e| UpdateError::Network(format!("curl.exe konnte nicht starten: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let code = output
                .status
                .code()
                .map_or_else(|| "?".to_string(), |c| c.to_string());
            return Err(UpdateError::Network(format!(
                "curl (Code {code}): {}",
                stderr.trim()
            )));
        }
        Ok(output.stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_args_are_hardened() {
        let url = "https://github.com/fabianh854-sketch/Aegis/releases/download/v0.2.0/aegis.exe";
        let args = curl_args(url);
        // -q muss das erste Argument sein, sonst liest curl doch eine .curlrc.
        assert_eq!(args[0], "-q");
        for pair in [
            ["--proto", "=https"],
            ["--proto-redir", "=https"],
            ["--max-time", "60"],
            ["--max-filesize", "67108864"],
        ] {
            assert!(
                args.windows(2).any(|w| w == pair),
                "{pair:?} fehlt in {args:?}"
            );
        }
        for flag in [
            "--tlsv1.2",
            "--fail",
            "--location",
            "--silent",
            "--show-error",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag} fehlt in {args:?}");
        }
        assert_eq!(&args[args.len() - 2..], ["--url", url]);
        assert!(
            !args.iter().any(|a| a == "--header"),
            "Header nur für die API"
        );
    }

    #[test]
    fn curl_args_api_headers() {
        let url = "https://api.github.com/repos/fabianh854-sketch/Aegis/releases";
        let args = curl_args(url);
        assert!(args
            .windows(2)
            .any(|w| w == ["--header", "Accept: application/vnd.github+json"]));
        assert!(args
            .windows(2)
            .any(|w| w == ["--header", "User-Agent: aegis-updater"]));
        assert_eq!(&args[args.len() - 2..], ["--url", url]);
    }
}
