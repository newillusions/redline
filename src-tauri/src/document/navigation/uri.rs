//! The single URL allowlist. Every address that comes from a PDF (a Link annotation, an
//! outline item) or from the user passes through [`validate_uri`] before it is shown as
//! openable, written to a file, or handed to the operating system.
//!
//! Only `http`, `https` and `mailto` are allowed. `file:`, `javascript:`, `data:`,
//! custom app schemes, UNC-style paths and anything with whitespace or control characters
//! are refused.

use std::process::Command;

/// Longest accepted address, in bytes.
pub const MAX_URI_LEN: usize = 2048;

/// Schemes redline will ever open.
pub const ALLOWED_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Check an address. `Ok` returns the NORMALISED address; `Err` is a short reason that is safe
/// to show to the user.
pub fn validate_uri(raw: &[u8]) -> Result<String, String> {
    if raw.is_empty() {
        return Err("the address is empty".into());
    }
    if raw.len() > MAX_URI_LEN {
        return Err(format!(
            "the address is longer than {MAX_URI_LEN} characters"
        ));
    }
    // 7-bit ASCII, printable, no space: anything else (control characters, line breaks,
    // non-ASCII look-alikes, embedded NUL) is refused rather than "cleaned".
    if !raw.iter().all(|b| (0x21..=0x7E).contains(b)) {
        return Err("the address contains characters that are not allowed".into());
    }
    // Characters RFC 3986 requires to be percent-encoded, and that can confuse the system
    // handler's argument parsing, are refused rather than passed on.
    if raw.iter().any(|b| {
        matches!(
            b,
            b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'}' | b'|'
        )
    }) {
        return Err("the address contains characters that are not allowed".into());
    }
    let s = std::str::from_utf8(raw).map_err(|_| "the address is not text".to_string())?;
    // Percent-encoded control characters (NUL, CR, LF, tab, ...) have no place in an address
    // we will hand to another program (mail header injection, truncation).
    if has_encoded_control(s) {
        return Err("the address contains encoded control characters".into());
    }
    // A real URL parser decides what the scheme is: a prefix check can be fooled by
    // look-alikes and encodings, the parser's own scheme is what the system handler sees.
    let url = url::Url::parse(s)
        .map_err(|_| "the address is not a valid web or mail address".to_string())?;
    let scheme = url.scheme(); // already lower-case
    if !ALLOWED_SCHEMES.contains(&scheme) {
        let shown: String = scheme.chars().take(16).collect();
        return Err(format!("the scheme \"{shown}\" is not allowed"));
    }
    // The raw text must start with exactly the scheme the parser found, then `:` (mail) or
    // `://` (web): the parser also accepts and repairs other spellings.
    let spelled = |tail: &str| {
        s.len() >= scheme.len() + tail.len()
            && s[..scheme.len()].eq_ignore_ascii_case(scheme)
            && s[scheme.len()..].starts_with(tail)
    };
    let normalised = if scheme == "mailto" {
        if !spelled(":") {
            return Err("the mail address is not valid".into());
        }
        normalise_mailto(&url)?
    } else {
        if !spelled("://") {
            return Err("the address is not a web address".into());
        }
        let authority = s[scheme.len() + 3..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        if authority.is_empty() {
            return Err("the address has no host".into());
        }
        if authority.contains('@') {
            return Err("addresses with a user name or password are not allowed".into());
        }
        match url.host_str() {
            Some(h) if !h.is_empty() => {}
            _ => return Err("the address has no host".into()),
        }
        // `https://trusted.example@evil.example/` shows the wrong host to a reader.
        if !url.username().is_empty() || url.password().is_some() {
            return Err("addresses with a user name or password are not allowed".into());
        }
        check_authority_is_canonical(authority, &url)?;
        url.to_string()
    };
    if normalised.len() > MAX_URI_LEN {
        return Err(format!(
            "the address is longer than {MAX_URI_LEN} characters"
        ));
    }
    Ok(normalised)
}

/// The URL parser and the program that finally opens the address (macOS `open`, Windows
/// `FileProtocolHandler`, `xdg-open`, then the browser) must read the host and port the same
/// way. The WHATWG parser quietly rewrites several spellings - `0x7f.1`, `2130706433`, `127.1`
/// and `0177.0.0.1` become `127.0.0.1`; `%`-sequences in the host are decoded; default and
/// zero-padded ports are dropped - and a different program may read the original differently.
/// So the host and port must already be written the way the parser would write them.
fn check_authority_is_canonical(authority: &str, url: &url::Url) -> Result<(), String> {
    let bad = |why: &str| {
        Err(format!(
            "the address is not written in a plain form ({why})"
        ))
    };
    let (host, port) = if authority.starts_with('[') {
        match authority.find(']') {
            Some(end) => (&authority[..=end], authority[end + 1..].strip_prefix(':')),
            None => return bad("unclosed bracket"),
        }
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    let canonical = url.host_str().unwrap_or("");
    if !host.eq_ignore_ascii_case(canonical) {
        return bad("host is spelled differently from how it is read");
    }
    if host.starts_with('[') {
        // An IPv6 literal must be in its shortest lower-case form (it is, by the check above,
        // only if the parser did not rewrite it).
        if host != host.to_ascii_lowercase() {
            return bad("IPv6 literal");
        }
    } else {
        if host.ends_with('.') || host.starts_with('.') || host.contains("..") {
            return bad("empty host label");
        }
        // Internal host names use underscores (`build_box.corp.example`); the parser keeps them
        // unchanged, so they are fine in every label except the last (the public suffix).
        let last_label_start = host.rfind('.').map_or(0, |i| i + 1);
        if !host.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || (b == b'_' && i < last_label_start)
        }) {
            return bad("host characters");
        }
    }
    if let Some(p) = port {
        let digits_ok = !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && (p == "0" || !p.starts_with('0'));
        if !digits_ok || p.parse::<u32>().map_or(true, |n| n > 65535) {
            return bad("port");
        }
    }
    Ok(())
}

/// A mail link is one address plus, at most, a subject and a body. Anything else a `mailto:`
/// can carry (`cc`, `bcc`, `attach`, `from`, `in-reply-to`, extra recipients, any other header)
/// is refused: mail programs treat those as instructions. The result is rebuilt from the
/// validated parts, not echoed from the input.
fn normalise_mailto(url: &url::Url) -> Result<String, String> {
    let bad = || "the mail address is not valid".to_string();
    let path = url.path();
    if path.is_empty() || path.contains(['%', ',', ';', '/', '\\']) || url.fragment().is_some() {
        return Err(bad());
    }
    let (local, domain) = path.split_once('@').ok_or_else(bad)?;
    let local_ok = !local.is_empty()
        && local
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+'-".contains(&b));
    let domain_ok = !domain.is_empty()
        && domain.contains('.')
        && !domain.starts_with(['.', '-'])
        && !domain.ends_with(['.', '-'])
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if !local_ok || !domain_ok {
        return Err(bad());
    }
    let mut subject = None;
    let mut body = None;
    // RFC 6068 percent-decoding only: `+` is a literal plus here, not a space (that is the
    // form-encoding the generic query iterator applies).
    for part in url.query().unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').ok_or_else(bad)?;
        let k = percent_decode(k);
        let v = percent_decode(v);
        let slot = match k.to_ascii_lowercase().as_str() {
            "subject" => &mut subject,
            "body" => &mut body,
            _ => return Err("only a subject and a body are allowed in a mail address".into()),
        };
        if slot.is_some() || v.chars().any(char::is_control) {
            return Err(bad());
        }
        *slot = Some(v);
    }
    let mut out = url::Url::parse(&format!("mailto:{path}")).map_err(|_| bad())?;
    if subject.is_some() || body.is_some() {
        let mut query = Vec::new();
        if let Some(v) = &subject {
            query.push(format!("subject={}", percent_encode(v)));
        }
        if let Some(v) = &body {
            query.push(format!("body={}", percent_encode(v)));
        }
        out.set_query(Some(&query.join("&")));
    }
    Ok(out.to_string())
}

/// Decode `%XX` sequences (bytes that are not valid UTF-8 are replaced); `+` stays a plus.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// RFC 6068 encoding: everything but unreserved characters becomes `%XX` (a space is `%20`).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn has_encoded_control(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'%' {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                let v = h * 16 + l;
                if v < 0x20 || v == 0x7F {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// [`validate_uri`] for text typed by the user.
pub fn validate_uri_str(s: &str) -> Result<String, String> {
    validate_uri(s.as_bytes())
}

/// The one address the user has been asked about. Opening needs the token the confirmation
/// dialog was built from; the address that gets opened is the one stored here (validated and
/// normalised when the question was prepared), never a string sent along with the "yes".
/// One slot, single use, expires: a stale or guessed token opens nothing, and nothing a PDF
/// contains can answer the dialog.
#[derive(Debug, Default)]
pub struct PendingOpen {
    slot: Option<(String, String, std::time::Instant)>,
}

/// How long a prepared question stays answerable.
pub const PENDING_TTL: std::time::Duration = std::time::Duration::from_secs(120);

impl PendingOpen {
    /// Validate `raw`, remember its normalised form and return `(token, normalised)`.
    pub fn prepare(
        &mut self,
        raw: &str,
        now: std::time::Instant,
    ) -> Result<(String, String), String> {
        let uri = validate_uri_str(raw)?;
        let token = uuid::Uuid::new_v4().to_string();
        self.slot = Some((token.clone(), uri.clone(), now));
        Ok((token, uri))
    }

    /// Consume the question if `token` is the current, unexpired one.
    pub fn take(&mut self, token: &str, now: std::time::Instant) -> Option<String> {
        match self.slot.take() {
            Some((t, uri, at)) if t == token && now.duration_since(at) <= PENDING_TTL => Some(uri),
            _ => None,
        }
    }
}

/// Run `f` on the open question, failing CLOSED if the lock is poisoned (a panic elsewhere
/// while it was held): no answer is accepted from a state we cannot vouch for.
pub fn with_pending<R>(
    m: &std::sync::Mutex<PendingOpen>,
    f: impl FnOnce(&mut PendingOpen) -> Result<R, String>,
) -> Result<R, String> {
    let mut guard = m
        .lock()
        .map_err(|_| "the confirmation state is unavailable; nothing was opened".to_string())?;
    f(&mut guard)
}

/// Hand a validated address to the operating system's default handler. The address is
/// re-validated here: the frontend's confirmation dialog is not trusted to have run it.
pub fn open_external(raw: &str) -> Result<(), String> {
    let uri = validate_uri_str(raw)?;
    // Only the parser's own serialisation ever reaches the opener: whatever was passed in must
    // already be exactly that (the confirmation flow stores the normalised form).
    if uri != raw {
        return Err("the address is not in its normalised form".into());
    }
    let mut cmd = opener_command(&uri);
    match cmd.spawn() {
        Ok(mut child) => {
            // Reap it off-thread so a quick-exit `open` does not linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(e) => Err(format!("could not open the address: {e}")),
    }
}

#[cfg(target_os = "macos")]
fn opener_command(uri: &str) -> Command {
    let mut c = Command::new("/usr/bin/open");
    c.arg(uri);
    c
}

#[cfg(target_os = "windows")]
fn opener_command(uri: &str) -> Command {
    // rundll32 takes the address as one argument; no shell, so `&` and `^` are not special.
    // Absolute path under the Windows directory: a bare name would be resolved through the
    // current directory / PATH, which a hostile file's folder could control.
    let root = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("windir"))
        .unwrap_or_else(|| "C:\\Windows".into());
    let mut c = Command::new(std::path::Path::new(&root).join("System32").join("rundll32.exe"));
    c.arg("url.dll,FileProtocolHandler").arg(uri);
    c
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn opener_command(uri: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(uri);
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_the_three_schemes() {
        assert_eq!(
            validate_uri(b"https://example.com/a?b=1#c").unwrap(),
            "https://example.com/a?b=1#c"
        );
        assert!(validate_uri(b"http://example.com").is_ok());
        assert!(
            validate_uri(b"HTTP://EXAMPLE.COM").is_ok(),
            "scheme is case-insensitive"
        );
        assert!(validate_uri(b"mailto:someone@example.com").is_ok());
    }

    #[test]
    fn refuses_every_other_scheme() {
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "file:///etc/passwd",
            "file://server/share",
            "data:text/html,<script>1</script>",
            "ftp://example.com/x",
            "vbscript:msgbox",
            "ms-msdt:/id",
            "tel:+971000000",
            "smb://host/share",
            "\\\\server\\share",
            "C:\\Windows\\system32\\calc.exe",
            "/etc/passwd",
            "example.com",
            "x",
            ":",
            "://x",
        ] {
            assert!(
                validate_uri(bad.as_bytes()).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn refuses_hostile_bytes() {
        for bad in [
            &b"https://example.com/a b"[..],
            b"https://example.com/\n",
            b"https://example.com/\x00",
            b"https://example.com/\t",
            b"https://exa\xE2\x80\x8Bmple.com",
            b"https://\xD0\xB0pple.com",
            b" https://example.com",
            b"https://example.com ",
            b"https://example.com/\"onmouseover=x",
            b"https://example.com/<script>",
            b"https://example.com/a\\b",
            b"https://example.com/a^b",
            b"https://example.com/a|b",
            b"https://example.com/{x}",
            b"https://example.com/`x`",
        ] {
            assert!(validate_uri(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn refuses_malformed_web_addresses() {
        for bad in [
            "https:",
            "https:example.com",
            "https://",
            "https:///path",
            "http://?x",
            "mailto:",
        ] {
            assert!(
                validate_uri(bad.as_bytes()).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn caps_the_length() {
        let ok = format!("https://example.com/{}", "a".repeat(MAX_URI_LEN - 20));
        assert!(ok.len() <= MAX_URI_LEN);
        assert!(validate_uri(ok.as_bytes()).is_ok());
        let long = format!("https://example.com/{}", "a".repeat(MAX_URI_LEN));
        assert!(validate_uri(long.as_bytes()).is_err());
        assert!(validate_uri(b"").is_err());
    }

    #[test]
    fn open_external_revalidates_before_spawning() {
        // A refused address never reaches the operating system.
        assert!(open_external("javascript:alert(1)").is_err());
        assert!(open_external("file:///etc/passwd").is_err());
        assert!(open_external("https://exa mple.com").is_err());
    }

    #[test]
    fn returns_the_normalised_address() {
        assert_eq!(
            validate_uri(b"HTTP://EXAMPLE.com").unwrap(),
            "http://example.com/"
        );
        assert_eq!(
            validate_uri(b"HTTPS://Example.COM:443/a").unwrap(),
            "https://example.com/a"
        );
        assert_eq!(
            validate_uri(b"mailto:Me@Example.com").unwrap(),
            "mailto:Me@Example.com"
        );
        // Dot segments are resolved, so what is shown is what is opened.
        assert_eq!(
            validate_uri(b"https://example.com/a/../b").unwrap(),
            "https://example.com/b"
        );
    }

    #[test]
    fn refuses_user_info_that_disguises_the_host() {
        for bad in [
            "https://user:pass@example.com/",
            "https://trusted.example@evil.example/",
            "http://a@b.example",
            "https://:pw@example.com/",
            "https://@example.com/",
        ] {
            assert!(
                validate_uri(bad.as_bytes()).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn idn_hosts_are_shown_as_punycode_and_raw_unicode_is_refused() {
        // Raw non-ASCII (homograph look-alikes) never gets in.
        assert!(validate_uri("https://\u{0430}pple.com/".as_bytes()).is_err());
        assert!(validate_uri("https://m\u{fc}nchen.example/".as_bytes()).is_err());
        // The ASCII (punycode) form is allowed and is what the confirm dialog shows.
        assert_eq!(
            validate_uri(b"https://xn--mnchen-3ya.example/").unwrap(),
            "https://xn--mnchen-3ya.example/"
        );
        // Mixed-case punycode host is normalised to lower case.
        assert_eq!(
            validate_uri(b"https://XN--MNCHEN-3YA.example/").unwrap(),
            "https://xn--mnchen-3ya.example/"
        );
    }

    #[test]
    fn refuses_scheme_tricks() {
        for bad in [
            "//example.com/path",
            "///example.com",
            "\\\\example.com\\share",
            "javascript%3Aalert(1)",
            "%6Aavascript:alert(1)",
            "java%0Ascript:alert(1)",
            "https%3A//example.com",
            "ht\ttps://example.com",
            "https:/example.com\\@evil.example",
            "hxxps://example.com",
            "jav&#x61;script:alert(1)",
            "vbscript:msgbox(1)",
            "data:text/html;base64,AAAA",
            "file:///etc/passwd",
            "FILE://host/share",
            "blob:https://example.com/uuid",
            "about:blank",
            "view-source:https://example.com",
        ] {
            assert!(
                validate_uri(bad.as_bytes()).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn refuses_whitespace_control_and_encoded_control_characters() {
        for bad in [
            &b"https://example.com/a\0b"[..],
            b"https://example.com/\r\nHost: evil",
            b"mailto:a@b.example?subject=x\r\nBcc:c@d.example",
            b"https://example.com/\tx",
            b" https://example.com/",
            b"https://example.com/ ",
            b"https://example.com/%00",
            b"https://example.com/%0d%0aSet-Cookie:x",
            b"mailto:a@b.example?subject=%0D%0ABcc:x@y.example",
            b"https://example.com/%1f",
            b"https://example.com/%7F",
        ] {
            assert!(validate_uri(bad).is_err(), "{bad:?} must be refused");
        }
        // Ordinary percent-encoding is fine.
        assert!(validate_uri(b"https://example.com/a%20b?q=%C3%A9").is_ok());
    }

    #[test]
    fn host_must_exist() {
        for bad in [
            "https://",
            "https:///x",
            "https://?q",
            "http://#f",
            "https:example.com",
            "mailto:",
            "mailto:/x",
        ] {
            assert!(validate_uri(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_cap_applies_to_the_normalised_form_too() {
        // 2048 raw bytes of path is fine; growth by percent-encoding past the cap is not.
        let spaces_free = format!("https://example.com/?q={}", "\u{7f}".repeat(0));
        assert!(validate_uri(spaces_free.as_bytes()).is_ok());
        let grows = format!("https://example.com/{}", "'".repeat(1500)); // ' is kept as is
        assert!(validate_uri(grows.as_bytes()).is_ok());
        let long_query = format!("https://example.com/?q={}", "[".repeat(1100)); // [ becomes %5B
        let r = validate_uri(long_query.as_bytes());
        assert!(r.is_err() || r.unwrap().len() <= MAX_URI_LEN);
    }

    #[test]
    fn mail_addresses_are_one_address_with_at_most_a_subject_and_body() {
        assert_eq!(
            validate_uri(b"mailto:me@example.com?subject=Hello%20there&body=Hi").unwrap(),
            "mailto:me@example.com?subject=Hello%20there&body=Hi"
        );
        assert_eq!(
            validate_uri(b"MAILTO:Me@Example.com?SUBJECT=x").unwrap(),
            "mailto:Me@Example.com?subject=x"
        );
        for bad in [
            "mailto:me@example.com?attach=/etc/passwd",
            "mailto:me@example.com?attachment=C:%5Csecret.txt",
            "mailto:me@example.com?cc=x@y.example",
            "mailto:me@example.com?bcc=x@y.example",
            "mailto:me@example.com?from=x@y.example",
            "mailto:me@example.com?in-reply-to=%3Cid%3E",
            "mailto:me@example.com?subject=a&subject=b",
            "mailto:me@example.com?x=1",
            "mailto:me@example.com,other@example.com",
            "mailto:me@example.com;other@example.com",
            "mailto:me%40example.com",
            "mailto:me@example.com%2Cother@example.com",
            "mailto:me@example.com#frag",
            "mailto:/me@example.com",
            "mailto:me",
            "mailto:@example.com",
            "mailto:me@",
            "mailto:me@localhost",
            "mailto:me@-example.com",
            "mailto:me@example.com/../x",
            "mailto://me@example.com",
            "mailto:?to=me@example.com",
        ] {
            assert!(
                validate_uri(bad.as_bytes()).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn the_opener_gets_exactly_the_normalised_value() {
        // Case, default port and dot-segments are normalised before anything is shown or opened.
        for (raw, want) in [
            (
                "HtTpS://ExAmPlE.CoM:443/a/./b/../c",
                "https://example.com/a/c",
            ),
            ("http://example.com:80", "http://example.com/"),
            (
                "https://example.com:8443/x?y=1#z",
                "https://example.com:8443/x?y=1#z",
            ),
        ] {
            assert_eq!(validate_uri(raw.as_bytes()).unwrap(), want);
        }
        // Re-validating the normalised value is a no-op (what is shown validates identically).
        for raw in [
            "HTTPS://EXAMPLE.com/a/../b",
            "MAILTO:A@B.example?body=x%20y",
        ] {
            let n = validate_uri(raw.as_bytes()).unwrap();
            assert_eq!(validate_uri(n.as_bytes()).unwrap(), n);
        }
    }

    #[test]
    fn web_forms_the_parser_would_repair_are_refused() {
        for bad in [
            "https:example.com/?u=http://x",
            "https:/example.com",
            "https:\\example.com",
            "https:\\\\example.com",
            "http:example.com",
            "https://example.com\\@evil.example/",
            "https://example.com:99999/",
            "https://[::1/",
            "https://exa mple.com/",
            "https://example.com:abc/",
        ] {
            assert!(
                validate_uri(bad.as_bytes()).is_err(),
                "{bad} must be refused"
            );
        }
    }

    #[test]
    fn an_address_opens_only_with_the_token_of_the_question_that_was_asked() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut p = PendingOpen::default();
        // A refused address never creates a question.
        assert!(p.prepare("javascript:alert(1)", t0).is_err());
        assert!(p.take("anything", t0).is_none());

        let (token, shown) = p.prepare("HTTPS://Example.COM/a", t0).unwrap();
        assert_eq!(
            shown, "https://example.com/a",
            "the dialog shows the normalised address"
        );
        // Wrong token: nothing, and the question is gone (no retry by guessing).
        assert!(p.take("not-the-token", t0).is_none());
        assert!(
            p.take(&token, t0).is_none(),
            "a wrong answer clears the question"
        );

        // Right token opens exactly the stored address, once.
        let (token, _) = p.prepare("https://example.com/b", t0).unwrap();
        assert_eq!(p.take(&token, t0).as_deref(), Some("https://example.com/b"));
        assert!(p.take(&token, t0).is_none(), "single use");

        // Expired.
        let (token, _) = p.prepare("https://example.com/c", t0).unwrap();
        assert!(p
            .take(&token, t0 + PENDING_TTL + Duration::from_secs(1))
            .is_none());

        // A newer question replaces the older: the old token is dead.
        let (old, _) = p.prepare("https://example.com/old", t0).unwrap();
        let (new, _) = p.prepare("https://example.com/new", t0).unwrap();
        assert!(p.take(&old, t0).is_none());
        let _ = new;
    }

    #[test]
    fn hosts_the_parser_rewrites_differently_from_the_opener_are_refused() {
        for bad in [
            // decimal, hex, octal and short IPv4 forms (all read as 127.0.0.1 by the parser)
            "http://2130706433/",
            "http://0x7f000001/",
            "http://0x7f.1/",
            "http://127.1/",
            "http://0177.0.0.1/",
            "http://0x7f.0x0.0x0.0x1/",
            "http://127.0.0.1.1/",
            "http://1.2.3/",
            // trailing / leading dots, empty labels
            "https://example.com./",
            "https://.example.com/",
            "https://a..example.com/",
            "https://example.com..:443/",
            // percent-sequences in the host (decoded by the parser)
            "https://example.com%2e/",
            "https://exa%6dple.com/",
            "https://example%2ecom/",
            "https://%65xample.com/",
            // unicode dot separators and look-alikes (raw non-ASCII)
            "https://example\u{3002}com/",
            "https://example\u{FF0E}com/",
            "https://example\u{FF61}com/",
            "https://example.com\u{2024}evil.test/",
            // ports the parser normalises away
            "https://example.com:0443/",
            "https://example.com:/",
            "https://example.com:00/",
            "https://example.com:65536/",
            "https://example.com:+443/",
            // non-canonical IPv6
            "https://[0:0:0:0:0:0:0:1]/",
            "https://[::FFFF:127.0.0.1]/",
            "https://[::1/",
            // tab / newline inside the scheme or host
            "ht\ttps://example.com/",
            "https://exa\tmple.com/",
            "https://example.com\n.evil.test/",
        ] {
            assert!(
                validate_uri(bad.as_bytes()).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn plain_hosts_still_pass_in_canonical_form() {
        for good in [
            "https://example.com/",
            "http://sub.example.co.uk:8080/p?q=1#f",
            "https://1.2.3.4/",
            "https://1.2.3.4:443/x",
            "https://[::1]/",
            "https://[2001:db8::1]:8443/",
            "https://xn--mnchen-3ya.example/",
            "https://a-b.example/",
            "HTTPS://EXAMPLE.COM/",
        ] {
            let n = validate_uri(good.as_bytes()).unwrap_or_else(|e| panic!("{good}: {e}"));
            assert_eq!(validate_uri(n.as_bytes()).unwrap(), n);
        }
    }

    #[test]
    fn the_opener_is_only_ever_given_the_serialised_validated_value() {
        // A string that validates but is not already normalised is refused at the opener.
        assert!(open_external("HTTPS://EXAMPLE.com").is_err());
        assert!(
            open_external("https://example.com").is_err(),
            "no trailing slash: not the serialised form"
        );
        assert!(open_external("https://example.com/a/../b").is_err());
        assert!(
            open_external("mailto:me@example.com?subject=a%2fb").is_err(),
            "mail is serialised with upper-case percent escapes"
        );
        // The stored value from the confirmation flow is exactly the serialised form.
        let mut p = PendingOpen::default();
        let t0 = std::time::Instant::now();
        let (token, shown) = p.prepare("HTTPS://EXAMPLE.com", t0).unwrap();
        let stored = p.take(&token, t0).unwrap();
        assert_eq!(stored, shown);
        assert_eq!(
            validate_uri_str(&stored).unwrap(),
            stored,
            "serialising twice changes nothing"
        );
    }

    #[test]
    fn every_failure_in_the_confirmation_path_refuses() {
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let m = Arc::new(Mutex::new(PendingOpen::default()));
        let (token, _) = with_pending(&m, |p| p.prepare("https://example.com/", t0)).unwrap();
        // Empty, whitespace, truncated and extended tokens never match.
        for bad in [
            "",
            " ",
            &token[..token.len() - 1],
            &format!("{token}x"),
            &token.to_uppercase(),
        ] {
            let r = with_pending(&m, |p| Ok(p.take(bad, t0)));
            assert_eq!(r, Ok(None), "token {bad:?}");
            // each wrong answer also cleared the question
            let (t, _) = with_pending(&m, |p| p.prepare("https://example.com/", t0)).unwrap();
            let _ = t;
        }
        // Expired: refused even with the right token.
        let (token, _) = with_pending(&m, |p| p.prepare("https://example.com/", t0)).unwrap();
        assert_eq!(
            with_pending(&m, |p| Ok(
                p.take(&token, t0 + PENDING_TTL + Duration::from_secs(1))
            )),
            Ok(None)
        );
        // A clock that moved backwards (answer earlier than the question) must not panic.
        let later = t0 + Duration::from_secs(5);
        let (token, _) = with_pending(&m, |p| p.prepare("https://example.com/", later)).unwrap();
        let _ = with_pending(&m, |p| Ok(p.take(&token, t0)));
        // A poisoned lock: every call is an Err, never an Ok(opened).
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        let (token, _) = (String::new(), String::new());
        assert!(with_pending(&m, |p| Ok(p.take(&token, t0))).is_err());
        assert!(with_pending(&m, |p| p.prepare("https://example.com/", t0)).is_err());
    }
}
