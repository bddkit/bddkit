use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniqueKind {
    Token,
    Number,
}

/// `email`, `slug`, `url` are readable aliases for `token`, not separate guarantees.
pub fn parse_kind(s: &str) -> Result<UniqueKind, String> {
    match s.trim() {
        "" | "token" | "email" | "slug" | "url" => Ok(UniqueKind::Token),
        "number" => Ok(UniqueKind::Number),
        other => Err(format!(
            "unknown unique value kind {other:?}; allowed: token, email, slug, url, number"
        )),
    }
}

fn base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 is always ASCII")
}

fn base36_pad(n: u64, width: usize) -> String {
    let s = base36(n);
    if s.len() >= width {
        s[s.len() - width..].to_string()
    } else {
        format!("{}{}", "0".repeat(width - s.len()), s)
    }
}

pub struct Generator {
    run_id: String,
    epoch_micros: u64,
    counter: AtomicU64,
}

impl Generator {
    pub fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is before 1970");
        let secs = now.as_secs();
        let micros = now.as_micros() as u64;
        // 32 random bits decouple runs that start at the same instant.
        let rand = u64::from(uuid::Uuid::now_v7().as_u128() as u32);
        Self {
            run_id: format!("{}{}", base36_pad(secs, 6), base36_pad(rand, 6)),
            epoch_micros: micros,
            counter: AtomicU64::new(0),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn next(&self, kind: UniqueKind) -> String {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        match kind {
            // Leading `u`: the value must not start with a digit.
            UniqueKind::Token => format!("u{}{}", self.run_id, base36(n)),
            UniqueKind::Number => (self.epoch_micros + n).to_string(),
        }
    }
}

impl Default for Generator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn run_id_is_twelve_chars() {
        let g = Generator::new();
        assert_eq!(
            g.run_id().len(),
            12,
            "run_id = 6 time chars + 6 random chars"
        );
    }

    #[test]
    fn tokens_are_unique_and_share_run_prefix() {
        let g = Generator::new();
        let vals: Vec<String> = (0..1000).map(|_| g.next(UniqueKind::Token)).collect();
        let uniq: HashSet<&String> = vals.iter().collect();
        assert_eq!(uniq.len(), 1000, "all values from one run must be distinct");
        for v in &vals {
            assert!(
                v.starts_with(&format!("u{}", g.run_id())),
                "shared run prefix: {v}"
            );
        }
    }

    #[test]
    fn token_never_starts_with_digit() {
        let g = Generator::new();
        for _ in 0..100 {
            let v = g.next(UniqueKind::Token);
            assert!(v.starts_with('u'), "{v}");
        }
    }

    #[test]
    fn numbers_are_digits_only_and_increase() {
        let g = Generator::new();
        let a = g.next(UniqueKind::Number);
        let b = g.next(UniqueKind::Number);
        assert!(a.chars().all(|c| c.is_ascii_digit()), "{a}");
        assert!(b.parse::<u64>().unwrap() > a.parse::<u64>().unwrap());
    }

    #[test]
    fn two_generators_do_not_collide() {
        let (g1, g2) = (Generator::new(), Generator::new());
        assert_ne!(
            g1.run_id(),
            g2.run_id(),
            "different runs must have different prefixes"
        );
    }

    #[test]
    fn aliases_map_to_token_and_unknown_is_rejected() {
        for s in ["", "token", "email", "slug", "url"] {
            assert_eq!(parse_kind(s).unwrap(), UniqueKind::Token, "{s}");
        }
        assert_eq!(parse_kind("number").unwrap(), UniqueKind::Number);
        assert!(
            parse_kind("uuid").is_err(),
            "an unknown kind must be rejected"
        );
    }
}
