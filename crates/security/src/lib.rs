//! Best-effort secret detection and redaction (spec section 16.2).
//!
//! This is a heuristic scanner. It WILL miss secrets (custom formats, encoded or
//! split values) and WILL flag some non-secrets. A clean report is not proof that
//! content is safe to share. Findings never contain the secret itself.

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    AwsAccessKey,
    GithubToken,
    SlackToken,
    GoogleApiKey,
    StripeKey,
    ApiKey,
    Jwt,
    PrivateKey,
    UrlCredentials,
    /// `password = "..."`-style assignment; the most false-positive-prone rule.
    GenericAssignment,
}

impl SecretKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwsAccessKey => "aws_access_key",
            Self::GithubToken => "github_token",
            Self::SlackToken => "slack_token",
            Self::GoogleApiKey => "google_api_key",
            Self::StripeKey => "stripe_key",
            Self::ApiKey => "api_key",
            Self::Jwt => "jwt",
            Self::PrivateKey => "private_key",
            Self::UrlCredentials => "url_credentials",
            Self::GenericAssignment => "generic_assignment",
        }
    }
}

/// A detected secret. Holds location and a short fingerprint, never the value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: SecretKind,
    /// 1-based line.
    pub line: usize,
    /// Byte offsets of the secret within the scanned text.
    pub start: usize,
    pub end: usize,
    /// First 8 hex chars of SHA-256 of the secret, to recognise repeats.
    pub fingerprint: String,
}

struct Rule {
    kind: SecretKind,
    re: Regex,
    /// Capture group holding the secret (0 = whole match).
    group: usize,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let r = |kind, pat: &str, group| Rule { kind, re: Regex::new(pat).expect("valid regex"), group };
        vec![
            r(SecretKind::PrivateKey, r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY(?: BLOCK)?-----|\z)", 0),
            r(SecretKind::AwsAccessKey, r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", 0),
            r(SecretKind::GithubToken, r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})\b", 0),
            r(SecretKind::SlackToken, r"\bxox[baprs]-[A-Za-z0-9-]{10,}", 0),
            r(SecretKind::GoogleApiKey, r"\bAIza[0-9A-Za-z_-]{35}\b", 0),
            r(SecretKind::StripeKey, r"\b[sr]k_live_[0-9a-zA-Z]{24,}\b", 0),
            r(SecretKind::ApiKey, r"\bsk-(?:ant-|proj-)?[A-Za-z0-9_-]{20,}", 0),
            r(SecretKind::Jwt, r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}", 0),
            r(SecretKind::UrlCredentials, r"[a-zA-Z][a-zA-Z0-9+.-]*://[^\s:/@]+:([^\s:/@]{3,})@", 1),
            r(
                SecretKind::GenericAssignment,
                r#"(?i)\b(?:api[_-]?key|secret(?:[_-]?key)?|access[_-]?token|auth[_-]?token|token|passwd|password|client[_-]?secret)\b["']?\s*[:=]\s*["']?([A-Za-z0-9/+_\-=.]{16,})"#,
                1,
            ),
        ]
    })
}

/// Placeholders and references that are not real secrets.
fn looks_like_placeholder(v: &str) -> bool {
    let l = v.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "example",
        "your",
        "xxxx",
        "changeme",
        "placeholder",
        "dummy",
        "sample",
        "redacted",
        "<",
        "${",
        "{{",
        "process.env",
        "env.",
    ];
    if MARKERS.iter().any(|m| l.contains(m)) {
        return true;
    }
    let first = v.chars().next();
    v.chars().all(|c| Some(c) == first)
}

fn fingerprint(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// Find secrets in `text`. Overlapping matches keep the earliest/longest one.
pub fn scan_text(text: &str) -> Vec<Finding> {
    let mut found: Vec<Finding> = Vec::new();
    for rule in rules() {
        for caps in rule.re.captures_iter(text) {
            let Some(m) = caps.get(rule.group) else {
                continue;
            };
            if rule.kind == SecretKind::GenericAssignment && looks_like_placeholder(m.as_str()) {
                continue;
            }
            if rule.kind == SecretKind::UrlCredentials && looks_like_placeholder(m.as_str()) {
                continue;
            }
            found.push(Finding {
                kind: rule.kind,
                line: line_of(text, m.start()),
                start: m.start(),
                end: m.end(),
                fingerprint: fingerprint(m.as_str()),
            });
        }
    }
    found.sort_by_key(|f| (f.start, std::cmp::Reverse(f.end)));
    let mut out: Vec<Finding> = Vec::new();
    for f in found {
        if out.last().is_some_and(|p| f.start < p.end) {
            continue;
        }
        out.push(f);
    }
    out
}

/// Replace every finding with `[REDACTED:<kind>]`. Returns the new text and the
/// findings (so callers can record that redaction happened).
pub fn redact_text(text: &str) -> (String, Vec<Finding>) {
    let findings = scan_text(text);
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for f in &findings {
        out.push_str(&text[pos..f.start]);
        out.push_str(&format!("[REDACTED:{}]", f.kind.as_str()));
        pos = f.end;
    }
    out.push_str(&text[pos..]);
    (out, findings)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFinding {
    pub path: String,
    pub finding: Finding,
}

/// Scan only the added lines of a unified diff. `line` is relative to that file's
/// added line text (1 = the first line scanned for the file chunk), and offsets are
/// within that single line.
pub fn scan_patch(patch: &str) -> Vec<DiffFinding> {
    let mut path = String::new();
    let mut out = Vec::new();
    for line in patch.lines() {
        if let Some(p) = line.strip_prefix("+++ ") {
            path = p.strip_prefix("b/").unwrap_or(p).to_string();
        } else if let Some(added) = line.strip_prefix('+') {
            for f in scan_text(added) {
                out.push(DiffFinding {
                    path: path.clone(),
                    finding: f,
                });
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensitiveFile {
    pub path: String,
    pub reason: &'static str,
}

/// Flag files that commonly hold secrets, by name only (content is not read).
pub fn sensitive_file(path: &str) -> Option<SensitiveFile> {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let reason = if name == ".env" || (name.starts_with(".env.") && !is_template(&name)) {
        "environment file"
    } else if matches!(
        name.as_str(),
        "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519"
    ) {
        "ssh private key"
    } else if [".pem", ".key", ".p12", ".pfx", ".jks", ".keystore"]
        .iter()
        .any(|e| name.ends_with(e))
    {
        "key or certificate store"
    } else if matches!(
        name.as_str(),
        ".netrc"
            | ".pgpass"
            | ".npmrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "secrets.json"
            | "service-account.json"
    ) {
        "credentials file"
    } else if name.starts_with("secrets.") || name.starts_with("secret.") {
        "secrets file"
    } else {
        return None;
    };
    Some(SensitiveFile {
        path: path.into(),
        reason,
    })
}

fn is_template(name: &str) -> bool {
    ["example", "sample", "template", "dist"]
        .iter()
        .any(|t| name.ends_with(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fake secrets are assembled at runtime so this file never contains a real-looking token.
    fn aws() -> String {
        format!("AKIA{}", "ABCDEFGHIJKLMNOP")
    }
    fn gh() -> String {
        format!("ghp_{}", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8")
    }

    fn kinds(t: &str) -> Vec<SecretKind> {
        scan_text(t).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn detects_known_token_formats() {
        assert_eq!(kinds(&format!("key={}", aws())), [SecretKind::AwsAccessKey]);
        assert_eq!(
            kinds(&format!("export T={}", gh())),
            [SecretKind::GithubToken]
        );
        assert_eq!(
            kinds(&format!("xoxb-{}", "1234567890-abcdefghij")),
            [SecretKind::SlackToken]
        );
        assert_eq!(
            kinds(&format!("AIza{}", "SyA-1234567890abcdefghijklmnopqrstu")),
            [SecretKind::GoogleApiKey]
        );
        assert_eq!(
            kinds(&format!("sk_live_{}", "abcdefghijklmnopqrstuvwx")),
            [SecretKind::StripeKey]
        );
        assert_eq!(
            kinds(&format!("sk-ant-{}", "abcdefghijklmnopqrstuvwxyz")),
            [SecretKind::ApiKey]
        );
        let jwt = format!(
            "eyJ{}.eyJ{}.{}",
            "hbGciOiJIUzI1NiJ9", "zdWIiOiIxMjM0NTYifQ", "abcdefghijk123"
        );
        assert_eq!(kinds(&jwt), [SecretKind::Jwt]);
        assert_eq!(
            kinds("postgres://admin:hunter22pw@db.internal/app"),
            [SecretKind::UrlCredentials]
        );
    }

    #[test]
    fn detects_private_key_block_even_when_truncated() {
        let full =
            "-----BEGIN RSA PRIVATE KEY-----\nMIIabc\ndef\n-----END RSA PRIVATE KEY-----\nafter";
        let f = scan_text(&full);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, SecretKind::PrivateKey);
        assert!(full[f[0].end..].starts_with("\nafter"));
        let cut = format!("-----BEGIN OPENSSH {}-----\nb3BlbnNzaC1rZXk", "PRIVATE KEY");
        assert_eq!(kinds(&cut), [SecretKind::PrivateKey]);
    }

    #[test]
    fn generic_assignment_ignores_placeholders_and_short_values() {
        assert_eq!(
            kinds("password = \"s3cr3tValue-9f8e7d6c5b\""),
            [SecretKind::GenericAssignment]
        );
        assert_eq!(
            kinds("API_KEY: Zm9vYmFyYmF6cXV4MTIzNDU2"),
            [SecretKind::GenericAssignment]
        );
        assert!(kinds("password = \"your-password-here-please\"").is_empty());
        assert!(kinds("token = ${GITHUB_TOKEN_VALUE_FROM_CI}").is_empty());
        assert!(kinds("const token = process.env.SOME_LONG_TOKEN_NAME").is_empty());
        assert!(kinds("password = \"short\"").is_empty());
        assert!(kinds("secret = 0000000000000000000000").is_empty());
        assert!(kinds("https://user:{{PASSWORD}}@host/x").is_empty());
    }

    #[test]
    fn ordinary_text_has_no_findings() {
        assert!(scan_text(
            "fn main() { println!(\"hello\"); }\nlet x = 42;\nsee https://example.com/path"
        )
        .is_empty());
    }

    #[test]
    fn redaction_removes_the_secret_and_keeps_context() {
        let t = format!("line1\nAWS={} and gh={}\nend", aws(), gh());
        let (out, f) = redact_text(&t);
        assert_eq!(f.len(), 2);
        assert!(!out.contains(&aws()) && !out.contains(&gh()));
        assert_eq!(
            out,
            "line1\nAWS=[REDACTED:aws_access_key] and gh=[REDACTED:github_token]\nend"
        );
        assert_eq!(f[0].line, 2);
    }

    #[test]
    fn generic_redaction_keeps_the_key_name() {
        let (out, _) = redact_text(&format!("password = \"{}\"", "s3cr3tValue-9f8e7d6c5b"));
        assert_eq!(out, "password = \"[REDACTED:generic_assignment]\"");
        let (out, _) = redact_text(&format!("db=postgres://admin:{}@db/app", "hunter22pw"));
        assert_eq!(out, "db=postgres://admin:[REDACTED:url_credentials]@db/app");
    }

    #[test]
    fn overlapping_matches_are_reported_once() {
        // `token = sk-...` matches both ApiKey and GenericAssignment on the same span.
        let t = format!("token = sk-{}", "abcdefghijklmnopqrstuvwxyz0123");
        assert_eq!(scan_text(&t).len(), 1);
    }

    #[test]
    fn findings_never_contain_the_secret() {
        let t = format!("x {}", aws());
        let f = scan_text(&t);
        let dbg = format!("{:?} {}", f, serde_json::to_string(&f).unwrap_or_default());
        assert!(!dbg.contains(&aws()));
        assert_eq!(f[0].fingerprint.len(), 8);
    }

    #[test]
    fn patch_scan_only_checks_added_lines() {
        let patch = format!(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1,2 @@\n-old {}\n+new\n+key={}\n context {}\n",
            aws(),
            aws(),
            aws()
        );
        let f = scan_patch(&patch);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "a.txt");
    }

    #[test]
    fn sensitive_file_names() {
        for p in [
            ".env",
            "app/.env.local",
            "id_rsa",
            "certs/server.pem",
            "k.key",
            ".netrc",
            "secrets.yaml",
            "credentials.json",
        ] {
            assert!(sensitive_file(p).is_some(), "{p}");
        }
        for p in [
            ".env.example",
            ".env.sample",
            "src/main.rs",
            "README.md",
            "monkey.txt",
            "envelope.rs",
        ] {
            assert!(sensitive_file(p).is_none(), "{p}");
        }
    }
}
