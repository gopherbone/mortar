// SPDX-License-Identifier: GPL-2.0-or-later
//! How anything reaches the store from a binary cache: with the caches and keys
//! named here and no others, whatever nix.conf says; every signature required;
//! nothing ever built on the device.

/// `--option`s for `nix-store --realise`.
pub fn realise_options(substituters: &[String], keys: &[String]) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let mut opt = |k: &str, v: &str| a.extend(["--option".to_string(), k.to_string(), v.to_string()]);
    opt("substituters", &substituters.join(" "));
    opt("trusted-public-keys", &keys.join(" "));
    opt("require-sigs", "true");
    opt("max-jobs", "0");
    opt("builders", "");
    // a repaired cache counts at once, not when Nix's memory of it expires
    opt("narinfo-cache-negative-ttl", "0");
    opt("narinfo-cache-positive-ttl", "0");
    a.extend(timeouts());
    a
}

/// A slow network shouldn't hang forever, nor a dropped one for long.
pub fn timeouts() -> Vec<String> {
    ["--option", "connect-timeout", "20", "--option", "stalled-download-timeout", "120", "--option", "download-attempts", "3"].map(String::from).to_vec()
}

/// What a failed download's log comes down to: (no network, a sentence).
pub fn classify_failure(tail: &[String]) -> (bool, String) {
    let all = tail.join("\n");
    let err = tail.iter().rev().find(|l| l.contains("error")).cloned().unwrap_or_else(|| tail.last().cloned().unwrap_or_default());
    let low = all.to_ascii_lowercase();
    if low.contains("signature") || low.contains("not signed") {
        return (false, "not signed by a trusted key".into());
    }
    let offline = ["could not resolve", "couldn't resolve", "couldn't connect", "could not connect", "failed to connect", "unable to download", "timeout", "timed out", "network is unreachable", "connection reset"]
        .iter()
        .any(|k| low.contains(k));
    (offline, err.trim().trim_start_matches("error:").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures() {
        let l = |s: &str| vec![s.to_string()];
        assert_eq!(classify_failure(&l("warning: ignoring substitute, as it's not signed by any of the keys")), (false, "not signed by a trusted key".to_string()));
        assert!(classify_failure(&l("error: unable to download 'http://x': Could not connect")).0);
        assert_eq!(classify_failure(&l("error: out of cheese")), (false, "out of cheese".to_string()));
        assert_eq!(classify_failure(&[]), (false, String::new()));
    }

    #[test]
    fn nothing_is_built_and_every_signature_counts() {
        let a = realise_options(&["https://a".into(), "https://b".into()], &["k:AA==".into()]);
        let opt = |n: &str| a.iter().position(|x| x == n).map(|i| a[i + 1].as_str());
        assert_eq!(opt("substituters"), Some("https://a https://b"));
        assert_eq!(opt("trusted-public-keys"), Some("k:AA=="));
        assert_eq!(opt("require-sigs"), Some("true"));
        assert_eq!(opt("max-jobs"), Some("0"));
        assert_eq!(opt("builders"), Some(""));
    }
}
