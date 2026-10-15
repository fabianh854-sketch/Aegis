//! Passwortgenerator und Stärkeanzeige. Der Zufall kommt vom Betriebssystem (getrandom).

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
const SYMBOLS: &str = "!@#$%^&*()-_=+[]{};:,.<>?";
/// Zeichen, die leicht verwechselt werden. Werden bei `exclude_ambiguous` entfernt.
const AMBIGUOUS: &str = "Il1O0o|";

#[derive(Debug, Clone)]
pub struct Options {
    pub length: usize,
    pub upper: bool,
    pub lower: bool,
    pub digits: bool,
    pub symbols: bool,
    pub exclude_ambiguous: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            length: 20,
            upper: true,
            lower: true,
            digits: true,
            symbols: true,
            exclude_ambiguous: true,
        }
    }
}

/// Erzeugt ein Passwort. Gibt `None` zurück, wenn keine Zeichengruppe aktiv ist.
pub fn generate(opts: &Options) -> Option<String> {
    let mut pool: Vec<char> = Vec::new();
    for (enabled, chars) in [
        (opts.lower, LOWER),
        (opts.upper, UPPER),
        (opts.digits, DIGITS),
        (opts.symbols, SYMBOLS),
    ] {
        if enabled {
            pool.extend(
                chars
                    .chars()
                    .filter(|c| !(opts.exclude_ambiguous && AMBIGUOUS.contains(*c))),
            );
        }
    }
    if pool.is_empty() || opts.length == 0 {
        return None;
    }

    let mut out = String::with_capacity(opts.length);
    for _ in 0..opts.length {
        let idx = random_below(pool.len() as u32)?;
        out.push(pool[idx as usize]);
    }
    Some(out)
}

/// Gleichverteilte Zufallszahl in `0..n` per Rejection-Sampling, ohne Modulo-Verzerrung.
fn random_below(n: u32) -> Option<u32> {
    let zone = u32::MAX - (u32::MAX % n);
    loop {
        let mut buf = [0u8; 4];
        getrandom::getrandom(&mut buf).ok()?;
        let x = u32::from_le_bytes(buf);
        if x < zone {
            return Some(x % n);
        }
    }
}

/// Grobe Stärke von 0 (leer) bis 4 (stark), abhängig von Länge und Zeichenklassen.
pub fn strength(pw: &str) -> u8 {
    if pw.is_empty() {
        return 0;
    }
    let classes = [
        pw.chars().any(|c| c.is_lowercase()),
        pw.chars().any(|c| c.is_uppercase()),
        pw.chars().any(|c| c.is_ascii_digit()),
        pw.chars().any(|c| !c.is_alphanumeric()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    let len = pw.chars().count();

    match () {
        _ if len >= 16 && classes >= 3 => 4,
        _ if len >= 12 && classes >= 2 => 3,
        _ if len >= 8 => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_matches_option() {
        let opts = Options { length: 32, ..Default::default() };
        assert_eq!(generate(&opts).unwrap().chars().count(), 32);
    }

    #[test]
    fn ambiguous_characters_are_excluded() {
        let opts = Options { length: 300, ..Default::default() };
        let pw = generate(&opts).unwrap();
        assert!(!pw.chars().any(|c| AMBIGUOUS.contains(c)));
    }

    #[test]
    fn no_character_group_returns_none() {
        let opts = Options {
            lower: false,
            upper: false,
            digits: false,
            symbols: false,
            ..Default::default()
        };
        assert!(generate(&opts).is_none());
    }

    #[test]
    fn strength_levels() {
        assert_eq!(strength(""), 0);
        assert_eq!(strength("abc"), 1);
        assert_eq!(strength("abcdefgh"), 2);
        assert_eq!(strength("abcdefghijk1"), 3);
        assert_eq!(strength("Abcdefgh1234!xyz"), 4);
    }
}
