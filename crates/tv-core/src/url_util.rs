//! URL helpers: registrable-domain extraction, exclusions, and sensitive-page
//! detection.
//!
//! We deliberately do **not** pull in the full Public Suffix List. TabVault runs
//! offline and storing a PSL copy would need its own refresh story. Instead we
//! use a pragmatic approximation that is correct for the domains people actually
//! browse from and never leaks credentials: we take the last two labels, plus a
//! small built-in table of multi-label public suffixes (co.uk, github.io, …).

/// Multi-label public suffixes we special-case. Not exhaustive by design; see
/// module docs.
const MULTI_LABEL_SUFFIXES: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "net.uk", "sch.uk", "ltd.uk", "plc.uk",
    "com.au", "net.au", "org.au", "edu.au", "gov.au", "id.au", "asn.au",
    "co.nz", "net.nz", "org.nz", "govt.nz", "ac.nz", "geek.nz", "school.nz",
    "co.jp", "or.jp", "ne.jp", "ac.jp", "go.jp", "ad.jp", "ed.jp", "gr.jp", "lg.jp",
    "co.kr", "or.kr", "ne.kr", "re.kr", "pe.kr", "go.kr", "ac.kr",
    "com.br", "net.br", "org.br", "gov.br", "edu.br",
    "com.cn", "net.cn", "org.cn", "gov.cn", "edu.cn", "ac.cn",
    "com.tw", "net.tw", "org.tw", "gov.tw", "edu.tw",
    "com.hk", "net.hk", "org.hk", "gov.hk", "edu.hk", "idv.hk",
    "com.sg", "net.sg", "org.sg", "gov.sg", "edu.sg", "per.sg",
    "co.in", "net.in", "org.in", "gen.in", "firm.in", "ind.in", "gov.in", "ac.in", "edu.in",
    "co.za", "net.za", "org.za", "gov.za", "ac.za", "web.za",
    "com.mx", "net.mx", "org.mx", "gob.mx", "edu.mx",
    "com.ar", "net.ar", "org.ar", "gob.ar", "edu.ar",
    "com.tr", "net.tr", "org.tr", "gov.tr", "edu.tr", "gen.tr", "web.tr",
    "com.ru", "net.ru", "org.ru", "msk.ru", "spb.ru",
    "co.il", "net.il", "org.il", "gov.il", "ac.il", "muni.il",
    "co.th", "or.th", "in.th", "go.th", "ac.th", "net.th",
    "com.vn", "net.vn", "org.vn", "gov.vn", "edu.vn",
    "com.ua", "net.ua", "org.ua", "gov.ua", "edu.ua", "in.ua",
    "com.pl", "net.pl", "org.pl", "gov.pl", "edu.pl", "waw.pl",
    "co.id", "net.id", "or.id", "web.id", "ac.id", "go.id", "sch.id",
    "com.my", "net.my", "org.my", "gov.my", "edu.my",
    "com.ph", "net.ph", "org.ph", "gov.ph", "edu.ph",
    "com.pk", "net.pk", "org.pk", "gov.pk", "edu.pk",
    "com.bd", "net.bd", "org.bd", "gov.bd", "edu.bd",
    // Common platform suffixes where the "site" is one label deeper.
    "github.io", "gitlab.io", "bitbucket.io", "netlify.app", "vercel.app",
    "pages.dev", "workers.dev", "herokuapp.com", "azurewebsites.net",
    "cloudfront.net", "s3.amazonaws.com", "firebaseapp.com", "web.app",
    "blogspot.com", "wordpress.com", "tumblr.com", "medium.com",
    "co.com", "uk.com", "us.com", "eu.com", "gb.com", "gb.net", "uk.net",
];

/// Extracts the scheme+host+port origin of a URL, lowercased.
///
/// Returns an empty string for URLs we cannot interpret (about:, chrome://,
/// data:, javascript:) — callers treat that as "no origin".
pub fn origin_of(url: &str) -> String {
    match parse_host(url) {
        Some((scheme, host)) if !scheme.is_empty() => format!("{scheme}://{host}"),
        Some((_, host)) => host,
        None => String::new(),
    }
}

/// Hostname of a URL, lowercased and without a trailing dot or userinfo.
pub fn host_of(url: &str) -> String {
    parse_host(url).map(|(_, h)| h).unwrap_or_default()
}

/// Returns `(scheme, host)` if the URL has a parseable network host.
fn parse_host(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (scheme, rest) = match trimmed.split_once("://") {
        Some((scheme, rest)) => {
            let scheme = scheme.to_ascii_lowercase();
            // Only these schemes have network hosts worth grouping by.
            if !matches!(
                scheme.as_str(),
                "http" | "https" | "ftp" | "ws" | "wss" | "file"
            ) {
                return None;
            }
            (scheme, rest)
        }
        None => {
            // Bare `host[:port][/path]` pasted into an import list, or typed by
            // hand. Only accept it when the authority-looking prefix is a
            // genuine host: bracketed IPv6, a hostname with an optional numeric
            // port. Anything else ("about:blank", prose) is rejected.
            let end = trimmed.find(['/', '?', '#']).unwrap_or(trimmed.len());
            let authority = &trimmed[..end];
            if authority.is_empty() {
                return None;
            }
            let host_ok = if let Some(inner) = authority.strip_prefix('[') {
                // [::1] or [::1]:8080
                match inner.find(']') {
                    None => false,
                    Some(close) => {
                        let v6 = &inner[..close];
                        let after = &inner[close + 1..];
                        let port_ok = after.is_empty()
                            || (after.starts_with(':')
                                && after[1..].chars().all(|c| c.is_ascii_digit()));
                        port_ok
                            && !v6.is_empty()
                            && v6
                                .chars()
                                .all(|c| c.is_ascii_hexdigit() || matches!(c, ':' | '.'))
                    }
                }
            } else {
                // Strip userinfo if someone pasted `user:pass@host`.
                let host_and_port = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
                let (host, port) = match host_and_port.rsplit_once(':') {
                    Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                        (h, Some(p))
                    }
                    _ => (host_and_port, None),
                };
                let _ = port;
                !host.is_empty()
                    && host
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
                    && host.chars().any(|c| c.is_ascii_alphanumeric())
            };
            if !host_ok {
                return None;
            }
            ("".to_string(), trimmed)
        }
    };
    // Cut at the first delimiter that ends the authority.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut authority = &rest[..authority_end];
    // Strip userinfo (never store credentials).
    if let Some((_, after)) = authority.rsplit_once('@') {
        authority = after;
    }
    // Strip port, handling bracketed IPv6.
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((v6, _)) => v6.to_string(),
            None => stripped.to_string(),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => {
                h.to_string()
            }
            _ => authority.to_string(),
        }
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    Some((scheme, host))
}

/// The registrable domain ("example.com") for a URL, used to group visit
/// statistics so `mail.google.com` and `drive.google.com` roll up sensibly
/// while `foo.co.uk` does not collapse to `co.uk`.
pub fn registrable_domain(url: &str) -> String {
    let host = host_of(url);
    registrable_domain_of_host(&host)
}

pub fn registrable_domain_of_host(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return String::new();
    }
    // Bare IPv4 / IPv6 literals are their own domain.
    if host.parse::<std::net::IpAddr>().is_ok() || host.contains(':') {
        return host;
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() <= 2 {
        return labels.join(".");
    }
    // Walk from the right, consuming public-suffix labels.
    let last_two = format!("{}.{}", labels[labels.len() - 2], labels[labels.len() - 1]);
    if MULTI_LABEL_SUFFIXES.contains(&last_two.as_str()) && labels.len() >= 3 {
        return format!(
            "{}.{}",
            labels[labels.len() - 3],
            last_two
        );
    }
    last_two
}

/// True when `url` matches any entry in `exclusions`.
///
/// Each entry is treated as a domain suffix: `example.com` matches
/// `example.com` and `www.example.com`, but *not* `notexample.com`. An entry
/// beginning with `.` behaves identically (`.` is stripped), and a full URL
/// entry is reduced to its host.
pub fn is_excluded(url: &str, exclusions: &[String]) -> bool {
    let host = host_of(url);
    if host.is_empty() {
        return false;
    }
    exclusions
        .iter()
        .any(|raw| domain_entry_matches(&host, raw))
}

/// True when `host` equals or is a subdomain of the exclusion entry.
fn domain_entry_matches(host: &str, entry: &str) -> bool {
    let entry = entry.trim().to_ascii_lowercase();
    if entry.is_empty() {
        return false;
    }
    // Accept entries written as URLs or with a leading dot.
    let entry = entry
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&entry);
    let entry = entry
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(entry);
    let entry = entry.trim_start_matches('.').trim_end_matches('.');
    if entry.is_empty() {
        return false;
    }
    host == entry || host.ends_with(&format!(".{entry}"))
}

/// Hosts whose *purpose* is handling secrets. These are offered for exclusion
/// by default: recording them adds no recovery value (a restored login page is
/// useless without the session) and replicates sensitive URLs into a local
/// plaintext-ish store.
pub const SENSITIVE_HOST_PATTERNS: &[&str] = &[
    // Password managers
    "1password.com", "1password.ca", "1password.eu", "lastpass.com", "bitwarden.com",
    "dashlane.com", "keepersecurity.com", "nordpass.com", "protonpass.com",
    "keepass.info", "enpass.io", "roboform.com", "kaspersky.com",
    // Identity providers / SSO
    "login.microsoftonline.com", "login.live.com", "accounts.google.com",
    "auth0.com", "okta.com", "onelogin.com", "duosecurity.com", "pingidentity.com",
    // Banking heuristics (common aggregators / well-known banks)
    "chase.com", "bankofamerica.com", "wellsfargo.com", "citi.com", "capitalone.com",
    "usbank.com", "pnc.com", "tdbank.com", "hsbc.com", "barclays.co.uk",
    "lloydsbank.com", "natwest.com", "santander.com", "revolut.com", "monzo.com",
    "paypal.com", "stripe.com", "wise.com", "venmo.com", "coinbase.com",
    "binance.com", "kraken.com", "fidelity.com", "vanguard.com", "schwab.com",
];

/// Heuristic: does this URL look like it handles credentials or money?
///
/// Used only to *suggest* exclusions and to badge rows in the UI — it is never
/// a hard filter, because a wrong guess would silently drop a tab the user
/// wanted back.
pub fn looks_sensitive(url: &str) -> bool {
    let host = host_of(url);
    if host.is_empty() {
        return false;
    }
    SENSITIVE_HOST_PATTERNS
        .iter()
        .any(|p| domain_entry_matches(&host, p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_origins() {
        assert_eq!(host_of("https://www.Example.COM:8443/a/b"), "www.example.com");
        assert_eq!(origin_of("https://user:pw@Example.com/x"), "https://example.com");
        assert_eq!(host_of("http://[2001:db8::1]:8080/x"), "2001:db8::1");
        assert_eq!(host_of("about:blank"), "");
        assert_eq!(host_of("chrome://settings"), "");
        assert_eq!(host_of("data:text/html,hi"), "");
        assert_eq!(origin_of("not a url"), "");
    }

    #[test]
    fn registrable_domains() {
        assert_eq!(registrable_domain("https://mail.google.com/mail"), "google.com");
        assert_eq!(registrable_domain("https://a.b.c.example.com"), "example.com");
        assert_eq!(registrable_domain("https://foo.co.uk/x"), "foo.co.uk");
        assert_eq!(registrable_domain("https://example.com"), "example.com");
        assert_eq!(registrable_domain("https://sub.example.co.uk/x"), "example.co.uk");
        assert_eq!(registrable_domain("https://me.github.io/repo"), "me.github.io");
        assert_eq!(registrable_domain("http://192.168.1.1/admin"), "192.168.1.1");
        assert_eq!(registrable_domain("localhost:3000"), "localhost");
    }

    #[test]
    fn exclusions_match_suffix_not_substring() {
        let ex = vec!["example.com".to_string(), ".bank.co.uk".to_string()];
        assert!(is_excluded("https://example.com/a", &ex));
        assert!(is_excluded("https://www.example.com/a", &ex));
        assert!(is_excluded("https://deep.sub.example.com/a", &ex));
        assert!(is_excluded("https://bank.co.uk/x", &ex));
        assert!(!is_excluded("https://notexample.com/a", &ex));
        assert!(!is_excluded("https://example.com.evil.net/", &ex));
        assert!(!is_excluded("https://other.com/", &ex));
    }

    #[test]
    fn exclusion_entries_accept_urls() {
        let ex = vec!["https://tracker.example.org/path".to_string()];
        assert!(is_excluded("https://tracker.example.org/other", &ex));
        assert!(is_excluded("https://a.tracker.example.org/", &ex));
    }

    #[test]
    fn sensitive_detection() {
        assert!(looks_sensitive("https://my.1password.com/vault"));
        assert!(looks_sensitive("https://www.chase.com/"));
        assert!(looks_sensitive("https://accounts.google.com/signin"));
        assert!(!looks_sensitive("https://github.com/rust-lang"));
        assert!(!looks_sensitive("about:blank"));
    }
}
