// SPDX-License-Identifier: GPL-2.0-or-later
//! The bucket: plain objects through `curl --aws-sigv4`, Nix store URLs
//! for `nix copy`. And the anonymous side: [`get_public`], plain HTTPS.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// GET many URLs at once, each into its file: the files that came back 200. The
/// rest (404, or nothing at all: callers treat that the same) are removed. With a
/// bucket and credentials the requests are signed for it.
pub fn get_many(items: &[(String, PathBuf)], signed: Option<(&Bucket, &Credentials)>, curl: &str) -> Result<HashSet<PathBuf>, String> {
    if items.is_empty() {
        return Ok(HashSet::new());
    }
    let mut c = Command::new(curl);
    c.args(["--silent", "--location", "--parallel", "--parallel-max", "16", "--connect-timeout", "15", "--max-time", "120", "--retry", "2"])
        .args(["--config", "-", "--write-out", "%{http_code} %{filename_effective}\n"]);
    if let Some((bucket, _)) = signed {
        c.args(["--aws-sigv4", &format!("aws:amz:{}:s3", bucket.region)]);
    }
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("{curl}: {e}"))?;
    {
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut config = String::new();
        if let Some((_, creds)) = signed {
            config.push_str(&format!("user = {}\n", cq(&format!("{}:{}", creds.access_key_id, creds.secret_access_key))));
        }
        for (url, file) in items {
            config.push_str(&format!("url = {}\noutput = {}\n", cq(url), cq(&file.to_string_lossy())));
        }
        stdin.write_all(config.as_bytes()).map_err(|e| format!("{curl}: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("{curl}: {e}"))?;
    let mut ok = HashSet::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(("200", file)) = line.split_once(' ') {
            ok.insert(PathBuf::from(file));
        }
    }
    for (_, file) in items {
        if !ok.contains(file) {
            let _ = std::fs::remove_file(file);
        }
    }
    Ok(ok)
}

/// An S3-compatible bucket. R2: endpoint `https://<ACCOUNT_ID>.r2.cloudflarestorage.com`,
/// region `auto`.
#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    /// `https://host[:port]` (`http://` for a local Garage).
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
}

#[derive(Clone, PartialEq)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials({}, <secret>)", self.access_key_id)
    }
}

impl Credentials {
    /// From AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY.
    pub fn from_env() -> Option<Credentials> {
        let id = std::env::var("AWS_ACCESS_KEY_ID").ok().filter(|s| !s.is_empty())?;
        let secret = std::env::var("AWS_SECRET_ACCESS_KEY").ok().filter(|s| !s.is_empty())?;
        Some(Credentials { access_key_id: id, secret_access_key: secret })
    }

    /// Environment for `nix copy` (Nix's S3 store reads the standard AWS
    /// variables). The profile/config files are pointed away so a stray
    /// ~/.aws cannot override these.
    pub fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("AWS_ACCESS_KEY_ID", self.access_key_id.clone()),
            ("AWS_SECRET_ACCESS_KEY", self.secret_access_key.clone()),
            ("AWS_SESSION_TOKEN", String::new()),
            ("AWS_PROFILE", String::new()),
            ("AWS_SHARED_CREDENTIALS_FILE", "/dev/null".into()),
            ("AWS_CONFIG_FILE", "/dev/null".into()),
            ("AWS_EC2_METADATA_DISABLED", "true".into()),
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FetchError {
    /// No network, DNS failure, timeout.
    Offline(String),
    /// 401/403: wrong or revoked credentials.
    Denied,
    NotFound,
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Offline(e) => write!(f, "offline ({e})"),
            FetchError::Denied => write!(f, "access denied (check the bucket credentials)"),
            FetchError::NotFound => write!(f, "not found"),
            FetchError::Other(e) => write!(f, "{e}"),
        }
    }
}

/// curl config-file quoting.
fn cq(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// curl's exit status and `--write-out "%{stderr}\n%{http_code}"`: the HTTP
/// status, or why there is none.
fn http_code(out: &std::process::Output) -> Result<u16, FetchError> {
    let err = String::from_utf8_lossy(&out.stderr);
    let code: u16 = err.lines().last().and_then(|l| l.trim().parse().ok()).unwrap_or(0);
    let msg = err.lines().rev().skip(1).find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    match out.status.code() {
        Some(0) => Ok(code),
        // couldn't resolve, couldn't connect, timeout, TLS connect,
        // empty reply, send/receive failure
        Some(6 | 7 | 28 | 35 | 52 | 55 | 56) => Err(FetchError::Offline(msg)),
        _ => Err(FetchError::Other(if msg.is_empty() { format!("curl failed ({})", out.status) } else { msg })),
    }
}

/// The largest manifest taken from a public URL: it is read before its
/// signature can be checked, and anyone may be answering.
const MAX_PUBLIC_BYTES: &str = "1048576";

/// GET a public URL, with no credentials (a channel manifest in HTTPS mode).
/// What comes back is untrusted until its signature is checked.
pub fn get_public(url: &str, curl: &str) -> Result<Vec<u8>, FetchError> {
    let out = Command::new(curl)
        .args(["--silent", "--show-error", "--location", "--proto", "=https,http", "--connect-timeout", "15", "--max-time", "120", "--retry", "2"])
        .args(["--max-filesize", MAX_PUBLIC_BYTES, "--output", "-", "--write-out", "%{stderr}\n%{http_code}"])
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| FetchError::Other(format!("{curl}: {e}")))?;
    let code = http_code(&out)?;
    match code {
        200 => Ok(out.stdout),
        404 => Err(FetchError::NotFound),
        c => Err(FetchError::Other(format!("HTTP {c} from {url}"))),
    }
}

impl Bucket {
    pub fn new(endpoint: &str, bucket: &str, region: &str) -> Result<Bucket, String> {
        let endpoint = endpoint.trim().trim_end_matches('/');
        let endpoint = if endpoint.contains("://") { endpoint.to_string() } else { format!("https://{endpoint}") };
        let host = endpoint.split_once("://").map(|(_, h)| h).unwrap_or("");
        if !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) || host.is_empty() || host.contains('/') {
            return Err(format!("endpoint '{endpoint}': want https://host[:port]"));
        }
        if bucket.is_empty() || !bucket.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') {
            return Err(format!("bucket '{bucket}': lowercase letters, digits, dashes, dots"));
        }
        let region = if region.is_empty() { "auto" } else { region };
        Ok(Bucket { endpoint, bucket: bucket.to_string(), region: region.to_string() })
    }

    /// The Nix binary-cache URL, e.g.
    /// `s3://mortar?endpoint=https://ACCOUNT.r2.cloudflarestorage.com&region=auto`,
    /// with `extra` parameters appended (`secret-key`, `compression`, ...).
    /// The endpoint keeps its scheme: Nix 2.33+ (S3 over curl) ignores
    /// `scheme=` and needs it in the endpoint.
    pub fn store_url(&self, extra: &[(&str, &str)]) -> String {
        let mut u = format!("s3://{}?endpoint={}&region={}", self.bucket, self.endpoint, self.region);
        for (k, v) in extra {
            u.push_str(&format!("&{k}={v}"));
        }
        u
    }

    /// Path-style object URL (R2 and Garage both take it).
    pub fn object_url(&self, key: &str) -> String {
        format!("{}/{}/{}", self.endpoint, self.bucket, key.trim_start_matches('/'))
    }

    fn curl(&self, creds: &Credentials, key: &str, extra: &[&str], curl: &str) -> Result<(Vec<u8>, u16), FetchError> {
        let mut c = Command::new(curl);
        c.args(["--silent", "--show-error", "--location", "--connect-timeout", "15", "--max-time", "120", "--retry", "2"])
            .args(["--aws-sigv4", &format!("aws:amz:{}:s3", self.region)])
            .args(["--config", "-", "--output", "-", "--write-out", "%{stderr}\n%{http_code}"])
            .args(extra)
            .arg(self.object_url(key))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| FetchError::Other(format!("{curl}: {e}")))?;
        {
            // the credentials go on stdin, not into argv (visible in ps)
            let mut stdin = child.stdin.take().expect("piped stdin");
            let _ = writeln!(stdin, "user = {}", cq(&format!("{}:{}", creds.access_key_id, creds.secret_access_key)));
        }
        let out = child.wait_with_output().map_err(|e| FetchError::Other(format!("{curl}: {e}")))?;
        let code = http_code(&out)?;
        Ok((out.stdout, code))
    }

    /// GET an object.
    pub fn get(&self, creds: &Credentials, key: &str, curl: &str) -> Result<Vec<u8>, FetchError> {
        let (body, code) = self.curl(creds, key, &[], curl)?;
        match code {
            200 => Ok(body),
            401 | 403 => Err(FetchError::Denied),
            404 => Err(FetchError::NotFound),
            c => Err(FetchError::Other(format!("HTTP {c}: {}", String::from_utf8_lossy(&body).chars().take(200).collect::<String>()))),
        }
    }

    /// PUT a local file as an object.
    pub fn put(&self, creds: &Credentials, key: &str, file: &Path, content_type: &str, curl: &str) -> Result<(), FetchError> {
        let f = file.to_string_lossy();
        let ct = format!("Content-Type: {content_type}");
        // the later --max-time wins: a NAR or an image takes longer than a manifest
        let (body, code) = self.curl(creds, key, &["--max-time", "7200", "--upload-file", &f, "--header", &ct, "--header", "Cache-Control: no-cache"], curl)?;
        match code {
            200 | 201 | 204 => Ok(()),
            401 | 403 => Err(FetchError::Denied),
            c => Err(FetchError::Other(format!("HTTP {c}: {}", String::from_utf8_lossy(&body).chars().take(300).collect::<String>()))),
        }
    }

    /// Copy an object within the bucket (S3 CopyObject), without moving its bytes.
    pub fn copy(&self, creds: &Credentials, from: &str, to: &str, curl: &str) -> Result<(), FetchError> {
        let source = format!("x-amz-copy-source: /{}/{}", self.bucket, from.trim_start_matches('/'));
        let (body, code) = self.curl(creds, to, &["--max-time", "600", "--request", "PUT", "--data-binary", "", "--header", &source, "--header", "Content-Type:"], curl)?;
        match code {
            200 | 201 | 204 => Ok(()),
            401 | 403 => Err(FetchError::Denied),
            404 => Err(FetchError::NotFound),
            c => Err(FetchError::Other(format!("HTTP {c}: {}", String::from_utf8_lossy(&body).chars().take(300).collect::<String>()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let b = Bucket::new("https://abc123.r2.cloudflarestorage.com/", "mortar-cache", "").unwrap();
        assert_eq!(b.store_url(&[]), "s3://mortar-cache?endpoint=https://abc123.r2.cloudflarestorage.com&region=auto");
        assert_eq!(
            b.store_url(&[("compression", "zstd")]),
            "s3://mortar-cache?endpoint=https://abc123.r2.cloudflarestorage.com&region=auto&compression=zstd"
        );
        assert_eq!(b.object_url("channels/mortar.json"), "https://abc123.r2.cloudflarestorage.com/mortar-cache/channels/mortar.json");
        let m = Bucket::new("http://cache:9000", "test", "us-east-1").unwrap();
        assert_eq!(m.store_url(&[]), "s3://test?endpoint=http://cache:9000&region=us-east-1");
        assert_eq!(Bucket::new("abc.r2.cloudflarestorage.com", "x", "auto").unwrap().endpoint, "https://abc.r2.cloudflarestorage.com");
        assert!(Bucket::new("https://h/path", "x", "auto").is_err());
        assert!(Bucket::new("ftp://h", "x", "auto").is_err());
        assert!(Bucket::new("https://h", "Bad_Bucket", "auto").is_err());
    }

    #[test]
    fn secrets_stay_out_of_debug() {
        let c = Credentials { access_key_id: "id".into(), secret_access_key: "hunter2".into() };
        assert!(!format!("{c:?}").contains("hunter2"));
        assert_eq!(cq(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
