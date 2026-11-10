use std::{
    net::IpAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context};

/// Runtime configuration, read from `MUZIK_*` environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    pub library_paths: Vec<PathBuf>,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub port: u16,
    pub scan_interval: Option<Duration>,
    pub cache_max_size: u64,
    pub web_dir: PathBuf,
    /// Set the `Secure` flag on cookies. Disable only for plain-HTTP local development.
    pub secure_cookies: bool,
    pub ffmpeg: PathBuf,
    /// Reverse proxies whose `X-Forwarded-For` header is trusted for the client IP
    /// (rate limiting, auth logs). Empty = use the TCP peer address.
    pub trusted_proxies: Vec<IpNet>,
    /// Maximum number of ffmpeg processes running at once.
    pub max_transcodes: usize,
    /// Inkult's `library.json` (who owns which file), for separate libraries
    /// per Inkult account. `None` = not available.
    pub inkult_library: Option<PathBuf>,
    /// Where Inkult's music folder is in Muzik's file system: the paths in
    /// Inkult's index are relative to it. Defaults to the first library root.
    pub inkult_music_dir: Option<PathBuf>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let library_paths: Vec<PathBuf> = std::env::var("MUZIK_LIBRARY_PATHS")
            .unwrap_or_else(|_| dev_path("music/library"))
            .split([',', ':'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect();
        if library_paths.is_empty() {
            bail!("MUZIK_LIBRARY_PATHS is empty");
        }

        let port = env_or("MUZIK_PORT", "9873")
            .parse()
            .context("MUZIK_PORT must be a port number")?;

        let scan_interval = match std::env::var("MUZIK_SCAN_INTERVAL") {
            Ok(v) if !v.trim().is_empty() && v.trim() != "0" => Some(
                parse_duration(v.trim())
                    .context("MUZIK_SCAN_INTERVAL: expected e.g. 3600, 30m, 6h")?,
            ),
            _ => None,
        };

        let cache_max_size = parse_size(&env_or("MUZIK_CACHE_MAX_SIZE", "10G"))
            .context("MUZIK_CACHE_MAX_SIZE: expected e.g. 500M, 10G")?;

        let trusted_proxies = std::env::var("MUZIK_TRUSTED_PROXIES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                IpNet::parse(s)
                    .with_context(|| format!("MUZIK_TRUSTED_PROXIES: invalid IP or CIDR `{s}`"))
            })
            .collect::<anyhow::Result<_>>()?;

        let default_transcodes = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 8)
            .to_string();
        let max_transcodes: usize = env_or("MUZIK_MAX_TRANSCODES", &default_transcodes)
            .parse()
            .context("MUZIK_MAX_TRANSCODES must be a number")?;

        Ok(Self {
            library_paths,
            data_dir: env_or_dev("MUZIK_DATA_DIR", "music/data"),
            cache_dir: env_or_dev("MUZIK_CACHE_DIR", "music/cache"),
            port,
            scan_interval,
            cache_max_size,
            web_dir: env_or_dev("MUZIK_WEB_DIR", "crates/muzik-web/dist"),
            secure_cookies: env_or("MUZIK_SECURE_COOKIES", "true") != "false",
            ffmpeg: env_or("MUZIK_FFMPEG", "ffmpeg").into(),
            trusted_proxies,
            max_transcodes: max_transcodes.max(1),
            inkult_library: non_empty_env("MUZIK_INKULT_LIBRARY").map(PathBuf::from),
            inkult_music_dir: non_empty_env("MUZIK_INKULT_MUSIC_DIR").map(PathBuf::from),
        })
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

/// Default directories point inside the repository checkout (`./music/...`), so a plain
/// `cargo run` works from any working directory without touching `/music`, `/data` or
/// `/cache`. The Docker image sets every `MUZIK_*` path explicitly, so these defaults
/// only apply to local development.
fn dev_path(rel: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().unwrap_or(root);
    root.join(rel).to_string_lossy().into_owned()
}

fn env_or_dev(key: &str, rel: &str) -> PathBuf {
    std::env::var(key).map_or_else(|_| PathBuf::from(dev_path(rel)), PathBuf::from)
}

/// An IP address or CIDR block (`10.0.0.1`, `172.16.0.0/12`, `fd00::/8`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        let (ip, prefix) = match s.split_once('/') {
            Some((ip, p)) => (ip, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = ip.trim().parse()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.trim().parse::<u8>()?,
            None => max,
        };
        if prefix > max {
            bail!("prefix /{prefix} is too long");
        }
        Ok(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        match (canonical_ip(self.addr), ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// Maps IPv4-mapped IPv6 addresses (`::ffff:1.2.3.4`) to plain IPv4.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Parses `90`, `90s`, `30m`, `6h`, `1d`.
pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    let (num, mult) = match s.chars().last() {
        Some('s') => (&s[..s.len() - 1], 1),
        Some('m') => (&s[..s.len() - 1], 60),
        Some('h') => (&s[..s.len() - 1], 3600),
        Some('d') => (&s[..s.len() - 1], 86400),
        _ => (s, 1),
    };
    let n: u64 = num.trim().parse()?;
    Ok(Duration::from_secs(n * mult))
}

/// Parses `1048576`, `500K`, `500M`, `10G` (binary units).
pub fn parse_size(s: &str) -> anyhow::Result<u64> {
    let s = s.trim().trim_end_matches(['b', 'B']);
    let (num, mult) = match s.chars().last().map(|c| c.to_ascii_uppercase()) {
        Some('K') => (&s[..s.len() - 1], 1u64 << 10),
        Some('M') => (&s[..s.len() - 1], 1 << 20),
        Some('G') => (&s[..s.len() - 1], 1 << 30),
        Some('T') => (&s[..s.len() - 1], 1 << 40),
        _ => (s, 1),
    };
    Ok(num.trim().parse::<u64>()? * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(parse_duration("6h").unwrap(), Duration::from_secs(21600));
        assert!(parse_duration("abc").is_err());
    }

    #[test]
    fn ip_nets() {
        let n = IpNet::parse("172.16.0.0/12").unwrap();
        assert!(n.contains("172.20.1.2".parse().unwrap()));
        assert!(n.contains("::ffff:172.20.1.2".parse().unwrap()));
        assert!(!n.contains("172.32.0.1".parse().unwrap()));
        let one = IpNet::parse("10.0.0.1").unwrap();
        assert!(one.contains("10.0.0.1".parse().unwrap()));
        assert!(!one.contains("10.0.0.2".parse().unwrap()));
        assert!(IpNet::parse("0.0.0.0/0")
            .unwrap()
            .contains("8.8.8.8".parse().unwrap()));
        assert!(IpNet::parse("fd00::/8")
            .unwrap()
            .contains("fd12::1".parse().unwrap()));
        assert!(IpNet::parse("10.0.0.0/33").is_err());
        assert!(IpNet::parse("nope").is_err());
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("10G").unwrap(), 10 << 30);
        assert_eq!(parse_size("500MB").unwrap(), 500 << 20);
        assert_eq!(parse_size("1024").unwrap(), 1024);
    }
}
