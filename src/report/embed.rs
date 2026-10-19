//! `--embed-artifacts`: the files a report links to, carried inside the page.
//!
//! The artifact on disk is only ever read. What goes into the page is the
//! smaller of the file and its re-encoding, so the cap `--embed-limit` counts
//! on-disk bytes and the page can only come out under it.

use anyhow::{Result, bail};
use clap::ValueEnum;

const DEFAULT_LIMIT: u64 = 20_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Images {
    /// 8-bit PNG as lossless WebP, exact pixels
    Lossless,
    /// 8-bit PNG as lossy WebP at --embed-quality
    Lossy,
    /// every file as it is
    Original,
}

#[allow(dead_code)] // read once the files are embedded, the next commit
#[derive(Clone, Debug)]
pub struct Options {
    pub limit: u64,
    pub images: Images,
    #[allow(dead_code)] // read by the lossy encoder, a later commit
    pub quality: u8,
}

impl Options {
    /// The flags as given, each `None` being "not passed". A quality that
    /// would change nothing is refused, the way every option that applies to
    /// nothing is.
    pub fn from_flags(
        limit: Option<u64>,
        images: Option<Images>,
        quality: Option<u8>,
    ) -> Result<Self> {
        let images = images.unwrap_or(Images::Lossless);
        if quality.is_some() && images != Images::Lossy {
            bail!("--embed-quality only applies to --embed-images lossy");
        }
        Ok(Self {
            limit: limit.unwrap_or(DEFAULT_LIMIT),
            images,
            quality: quality.unwrap_or(80),
        })
    }
}

/// `20MB`, `500kb`, `1048576`: whole numbers, decimal units, bytes when bare.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let (number, unit) = text.split_at(digits);
    let factor: u64 = match unit.to_ascii_uppercase().as_str() {
        "" | "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        _ => {
            return Err(format!(
                "`{text}` is not a size: use a whole number and B, KB, MB or GB, e.g. 20MB"
            ));
        }
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(|| {
            format!("`{text}` is not a size: use a whole number and B, KB, MB or GB, e.g. 20MB")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_as_the_spec_says() {
        for (text, bytes) in [
            ("20MB", 20_000_000),
            ("20mb", 20_000_000),
            ("500KB", 500_000),
            ("1048576", 1_048_576),
            ("1GB", 1_000_000_000),
            ("0", 0),
        ] {
            assert_eq!(parse_size(text), Ok(bytes), "{text}");
        }
        for bad in ["-1", "1.5MB", "20MiB", "MB", "", "99999999999GB"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_quality_without_lossy_is_refused() {
        assert!(Options::from_flags(None, None, Some(70)).is_err());
        assert!(Options::from_flags(None, Some(Images::Original), Some(70)).is_err());
        assert!(Options::from_flags(None, Some(Images::Lossy), Some(70)).is_ok());
    }
}
