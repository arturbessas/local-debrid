use std::fmt;

use anyhow::{Context, bail};

/// BitTorrent v1 info-hash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InfoHash(pub [u8; 20]);

impl InfoHash {
    /// Accepts 40-char hex or 32-char base32, case-insensitively.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let text = text.trim();
        let mut out = [0u8; 20];
        match text.len() {
            40 => {
                let bytes = data_encoding::HEXLOWER_PERMISSIVE
                    .decode(text.as_bytes())
                    .context("invalid hex info-hash")?;
                out.copy_from_slice(&bytes);
            }
            32 => {
                let bytes = data_encoding::BASE32
                    .decode(text.to_ascii_uppercase().as_bytes())
                    .context("invalid base32 info-hash")?;
                out.copy_from_slice(&bytes);
            }
            _ => bail!("info-hash must be 40 hex or 32 base32 characters"),
        }
        Ok(Self(out))
    }

    pub fn from_magnet(magnet: &str) -> anyhow::Result<Self> {
        let url = url::Url::parse(magnet).context("invalid magnet link")?;
        if url.scheme() != "magnet" {
            bail!("not a magnet link");
        }
        for (key, value) in url.query_pairs() {
            if key == "xt"
                && let Some(hash) = value.strip_prefix("urn:btih:")
            {
                return Self::parse(hash);
            }
        }
        bail!("magnet link has no btih info-hash")
    }

    pub fn to_id20(self) -> librqbit::dht::Id20 {
        librqbit::dht::Id20::new(self.0)
    }

    pub fn magnet(&self, trackers: &[String]) -> String {
        let mut magnet = format!("magnet:?xt=urn:btih:{self}");
        for tracker in trackers {
            magnet.push_str("&tr=");
            magnet.push_str(
                &percent_encoding::utf8_percent_encode(tracker, percent_encoding::NON_ALPHANUMERIC)
                    .to_string(),
            );
        }
        magnet
    }
}

impl fmt::Display for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&data_encoding::HEXLOWER.encode(&self.0))
    }
}

impl fmt::Debug for InfoHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InfoHash({self})")
    }
}

impl std::str::FromStr for InfoHash {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEX: &str = "dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c";

    #[test]
    fn hex_round_trip_is_lowercase() {
        assert_eq!(InfoHash::parse(&HEX.to_uppercase()).unwrap().to_string(), HEX);
    }

    #[test]
    fn base32_matches_hex() {
        let hex = InfoHash::parse(HEX).unwrap();
        let b32 = data_encoding::BASE32.encode(&hex.0);
        assert_eq!(InfoHash::parse(&b32.to_lowercase()).unwrap(), hex);
    }

    #[test]
    fn rejects_garbage() {
        assert!(InfoHash::parse("xyz").is_err());
        assert!(InfoHash::parse(&"z".repeat(40)).is_err());
    }

    #[test]
    fn magnet_round_trip() {
        let hash = InfoHash::parse(HEX).unwrap();
        let magnet = hash.magnet(&["udp://t.example:80/announce".into()]);
        assert!(magnet.contains("tr=udp%3A%2F%2Ft"));
        assert_eq!(InfoHash::from_magnet(&magnet).unwrap(), hash);
        assert!(InfoHash::from_magnet("http://x").is_err());
    }
}
