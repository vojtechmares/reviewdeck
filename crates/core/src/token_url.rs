//! Where to send the user to create a token for an account they are adding. A port
//! of src/shared/token-url.ts.
//!
//! The link follows the host they typed, so a self-hosted GitLab or Forgejo opens
//! its own token page; a host that cannot be read falls back to the public instance.

use url::Url;

use crate::model::ProviderKind;

/// The hosts that are github.com itself, whose token page lives on github.com
/// rather than under the API host.
const GITHUB_COM: [&str; 3] = ["github.com", "www.github.com", "api.github.com"];

/// The token page of each provider, relative to the web root.
fn path(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Github => "/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck",
        ProviderKind::Gitlab => "/-/user_settings/personal_access_tokens",
        ProviderKind::Forgejo => "/user/settings/applications",
        ProviderKind::Bitbucket => "/account/settings/app-passwords/new",
    }
}

/// The public instance of each provider, for when the typed host is no help.
fn fallback(kind: ProviderKind) -> String {
    let origin = match kind {
        ProviderKind::Github => "https://github.com",
        ProviderKind::Gitlab => "https://gitlab.com",
        ProviderKind::Forgejo => "https://codeberg.org",
        ProviderKind::Bitbucket => "https://bitbucket.org",
    };
    format!("{origin}{}", path(kind))
}

/// The page where a token for this provider is created, on the host as typed.
///
/// Bitbucket Cloud is the only Bitbucket, so its page never follows the host. A
/// GitHub host that is github.com in any of its spellings (including the API host
/// people paste) goes to github.com; anything else is GitHub Enterprise and keeps
/// its own origin.
pub fn token_create_url(kind: ProviderKind, host: &str) -> String {
    if kind == ProviderKind::Bitbucket {
        return fallback(kind);
    }
    let Some((origin, hostname)) = try_origin(host) else {
        return fallback(kind);
    };
    if kind == ProviderKind::Github {
        let web = if GITHUB_COM.contains(&hostname.as_str()) {
            "https://github.com"
        } else {
            origin.as_str()
        };
        return format!("{web}{}", path(kind));
    }
    format!("{origin}{}", path(kind))
}

/// The typed host as an origin plus any path it was mounted under (no trailing
/// slash), and its bare hostname - or `None` when it cannot be read as a URL.
///
/// No scheme means https, as everywhere else the app reads a host.
fn try_origin(input: &str) -> Option<(String, String)> {
    let trimmed = input
        .trim_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}')
        .trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let has_scheme = trimmed
        .get(..8)
        .is_some_and(|head| head.eq_ignore_ascii_case("https://"))
        || trimmed
            .get(..7)
            .is_some_and(|head| head.eq_ignore_ascii_case("http://"));
    let with_scheme = if has_scheme {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&with_scheme).ok()?;
    let hostname = url.host_str()?.to_string();
    // `url.host` in the WHATWG sense: the hostname, plus the port when it is not
    // the scheme's default.
    let host = match url.port() {
        Some(port) => format!("{hostname}:{port}"),
        None => hostname.clone(),
    };
    let pathname = url.path();
    let mount = if pathname == "/" {
        ""
    } else {
        pathname.trim_end_matches('/')
    };
    Some((format!("{}://{host}{mount}", url.scheme()), hostname))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_create_url_follows_the_typed_forgejo_host() {
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "codeberg.org"),
            "https://codeberg.org/user/settings/applications"
        );
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "git.acme.dev"),
            "https://git.acme.dev/user/settings/applications"
        );
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "https://git.acme.dev/"),
            "https://git.acme.dev/user/settings/applications"
        );
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "http://localhost:3000"),
            "http://localhost:3000/user/settings/applications"
        );
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "https://acme.dev/git/"),
            "https://acme.dev/git/user/settings/applications"
        );
    }

    #[test]
    fn token_create_url_falls_back_when_the_host_is_empty_or_invalid() {
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "   "),
            "https://codeberg.org/user/settings/applications"
        );
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "://"),
            "https://codeberg.org/user/settings/applications"
        );
    }

    #[test]
    fn token_create_url_follows_custom_gitlab_and_ghes_hosts() {
        assert_eq!(
            token_create_url(ProviderKind::Gitlab, "gitlab.acme.com"),
            "https://gitlab.acme.com/-/user_settings/personal_access_tokens"
        );
        assert_eq!(
            token_create_url(ProviderKind::Github, "github.acme.com"),
            "https://github.acme.com/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck"
        );
        assert_eq!(
            token_create_url(ProviderKind::Github, "api.github.com"),
            "https://github.com/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck"
        );
    }

    #[test]
    fn token_create_url_keeps_bitbucket_on_bitbucket_org() {
        assert_eq!(
            token_create_url(ProviderKind::Bitbucket, "ignored.example"),
            "https://bitbucket.org/account/settings/app-passwords/new"
        );
    }

    #[test]
    fn token_create_url_reads_hosts_the_way_a_browser_does() {
        // A default port disappears, the scheme is case-insensitive and the host is
        // lowercased, as `new URL` normalises them.
        assert_eq!(
            token_create_url(ProviderKind::Gitlab, "HTTPS://GitLab.Acme.com:443/"),
            "https://gitlab.acme.com/-/user_settings/personal_access_tokens"
        );
        // Non-ASCII hosts go through IDNA, and multi-byte input is no trouble.
        assert_eq!(
            token_create_url(ProviderKind::Forgejo, "git.bücher.example"),
            "https://git.xn--bcher-kva.example/user/settings/applications"
        );
        assert_eq!(
            token_create_url(ProviderKind::Github, "https://WWW.GitHub.com"),
            "https://github.com/settings/tokens/new?scopes=repo,read:org&description=Reviewdeck"
        );
    }
}
