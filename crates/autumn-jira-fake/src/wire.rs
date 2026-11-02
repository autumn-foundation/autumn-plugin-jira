//! Small wire helpers: URLs, percent decoding, queries, forms and Basic
//! auth. The fake has its own code for them.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// The parts of an `https` URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Url<'a> {
    pub(crate) host: &'a str,
    pub(crate) path: &'a str,
    pub(crate) query: &'a str,
}

/// Splits an `https` URL. Other schemes give `None`.
pub(crate) fn parse_url(url: &str) -> Option<Url<'_>> {
    let rest = url.strip_prefix("https://")?;
    let rest = rest.split_once('#').map_or(rest, |(r, _)| r);
    let (authority, path_query) = rest
        .find('/')
        .map_or((rest, "/"), |at| (&rest[..at], &rest[at..]));
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    let host = authority.strip_suffix(":443").unwrap_or(authority);
    let (path, query) = path_query.split_once('?').unwrap_or((path_query, ""));
    Some(Url { host, path, query })
}

/// Decodes `%XX` escapes. With `plus_space`, a `+` becomes a space. A bad
/// escape stays as it is. Bytes that are not UTF-8 become U+FFFD.
pub(crate) fn percent_decode(text: &str, plus_space: bool) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h * 16 + l);
                        i += 3;
                        continue;
                    }
                    _ => out.push(b'%'),
                }
            }
            b'+' if plus_space => out.push(b' '),
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Returns the value of one hex digit.
const fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parses `a=1&b=2` (a query or a form body). Names and values are decoded.
pub(crate) fn parse_pairs(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(name, true), percent_decode(value, true))
        })
        .collect()
}

/// Returns the first value of a name.
pub(crate) fn first<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Returns the comma-separated values of every pair with this name.
pub(crate) fn csv(pairs: &[(String, String)], name: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(n, _)| n == name)
        .flat_map(|(_, v)| v.split(','))
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Reads `Basic base64(user:password)`.
pub(crate) fn basic_credentials(header: &str) -> Option<(String, String)> {
    let (scheme, value) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = STANDARD.decode(value.trim()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some((user.to_owned(), password.to_owned()))
}

/// Reads `Bearer <token>`.
pub(crate) fn bearer_token(header: &str) -> Option<&str> {
    let (scheme, value) = header.trim().split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| value.trim())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_split() {
        let url = parse_url("https://acme.atlassian.net:443/rest/api/3/x?a=1#f").unwrap();
        assert_eq!(url.host, "acme.atlassian.net");
        assert_eq!(url.path, "/rest/api/3/x");
        assert_eq!(url.query, "a=1");
        let bare = parse_url("https://chat.example").unwrap();
        assert_eq!(
            (bare.host, bare.path, bare.query),
            ("chat.example", "/", "")
        );
        assert!(parse_url("http://acme.atlassian.net/").is_none());
        assert!(parse_url("https:///x").is_none());
        assert!(parse_url("https://user@evil.example/").is_none());
    }

    #[test]
    fn decoding() {
        assert_eq!(percent_decode("a%20b+c", false), "a b+c");
        assert_eq!(percent_decode("a%20b+c", true), "a b c");
        assert_eq!(percent_decode("100%", true), "100%");
        assert_eq!(percent_decode("%zz%4", true), "%zz%4");
        assert_eq!(percent_decode("%E2%9C%93", false), "\u{2713}");
        let pairs = parse_pairs("jql=a%3D1&fields=x,y&fields=z&flag&");
        assert_eq!(first(&pairs, "jql"), Some("a=1"));
        assert_eq!(first(&pairs, "flag"), Some(""));
        assert_eq!(csv(&pairs, "fields"), ["x", "y", "z"]);
    }

    #[test]
    fn auth_headers() {
        assert_eq!(
            basic_credentials("Basic ZnJlZDpmcmVk"),
            Some(("fred".to_owned(), "fred".to_owned()))
        );
        assert_eq!(basic_credentials("basic Zm9v"), None);
        assert_eq!(basic_credentials("Bearer x"), None);
        assert_eq!(basic_credentials("Basic"), None);
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  "), None);
        assert_eq!(bearer_token("Basic abc"), None);
    }
}
