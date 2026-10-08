//! Images that sit behind an account's authentication: a port of
//! src/shared/images.ts and the request policy of src/main/images.ts.
//!
//! A screenshot pasted into a GitLab merge request becomes an upload on that
//! instance, and a Forgejo attachment behaves the same way. On a private project
//! both need a token, so on exactly the self-hosted hosts this app exists for,
//! description screenshots would come out broken if every image were fetched
//! anonymously.
//!
//! The Electron app pointed such images at a custom URL scheme its main process
//! served. Here there is no browser in between, so the decision is a value instead:
//! [`rewrite_image_source`] says whether an image is an ordinary [`ImageSource::Remote`]
//! (fetched with no credential at all), an [`ImageSource::Authenticated`] one (fetched
//! by the app state with that account's token, through [`fetch_authenticated`]), or
//! [`ImageSource::Blocked`] (never fetched; the UI shows the alt text).
//!
//! Everything except [`fetch_authenticated`] is pure, which matters most for
//! [`resolve_image_request`]: it is the security boundary of the feature, because
//! whoever acts on what it accepts attaches a credential to it. A token may only ever
//! go to a host belonging to its account.

use std::time::{Duration, Instant};

use url::Url;

use crate::error::{Error, Result, msg};
use crate::http::{Http, Method, RequestOptions, describe, safe_host};
use crate::model::{Account, ProviderKind};

/// How long an authenticated image may take, redirects included.
pub const IMAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Enough for the usual hop to object storage, not enough to be walked in circles.
pub const MAX_IMAGE_REDIRECTS: usize = 4;

/// What an image request accepts.
pub const IMAGE_ACCEPT: &str = "image/*,*/*;q=0.8";

/// The part of an account the image rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAccount {
    pub id: String,
    /// API root, e.g. https://gitlab.acme.dev/api/v4
    pub base_url: String,
    /// Web root, e.g. https://gitlab.acme.dev
    pub web_url: String,
}

impl From<&Account> for ImageAccount {
    fn from(account: &Account) -> Self {
        ImageAccount {
            id: account.id.clone(),
            base_url: account.base_url.clone(),
            web_url: account.web_url.clone(),
        }
    }
}

/// `new URL(url).host`, lower-cased: the host plus a non-default port.
fn host_of(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    host_of_url(&parsed)
}

fn host_of_url(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let host = host.to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn is_http(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

/// The hosts an account's token belongs to, and the only ones it may be sent to:
/// the web root's and the API root's, without repeating one.
pub fn account_hosts(account: &ImageAccount) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for host in [host_of(&account.web_url), host_of(&account.base_url)]
        .into_iter()
        .flatten()
    {
        if !host.is_empty() && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts
}

/// The first account whose hosts include `host` (any case).
pub fn account_for_host<'a>(
    accounts: &'a [ImageAccount],
    host: Option<&str>,
) -> Option<&'a ImageAccount> {
    let wanted = host.filter(|host| !host.is_empty())?.to_ascii_lowercase();
    accounts
        .iter()
        .find(|account| account_hosts(account).contains(&wanted))
}

/// Everything an image source is resolved against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageContext {
    /// Repository web root, so a relative source can be made absolute first.
    pub repo_root: Option<String>,
    pub accounts: Vec<ImageAccount>,
}

/// Where an image comes from, decided once while the markdown is parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// An absolute http(s) URL fetched as it is, with no credential involved: a
    /// public badge, an external screenshot.
    Remote(String),
    /// An absolute http(s) URL on a signed-in account's host. It must be fetched by
    /// whoever holds that account's token ([`fetch_authenticated`]), and the URL is
    /// re-checked against the account there ([`resolve_image_request`]).
    Authenticated { account_id: String, url: String },
    /// Nothing that can be fetched: no source, an unsafe scheme, a `data:` URI, or a
    /// relative source with no repository to resolve it against. Shown as its alt
    /// text, as a broken image is in the browser.
    Blocked,
}

/// The decision for one image source.
///
/// A source on a signed-in account's host is [`ImageSource::Authenticated`];
/// everything else that is plain http(s) stays [`ImageSource::Remote`], so a public
/// badge or an external screenshot keeps loading over ordinary HTTPS with no
/// credential involved.
///
/// A relative source is resolved against the repository root first, because whether
/// it needs authenticating depends on the host it lands on.
pub fn rewrite_image_source(source: &str, context: &ImageContext) -> ImageSource {
    let Some(absolute) = absolute_source(source, context.repo_root.as_deref()) else {
        return ImageSource::Blocked;
    };
    if !is_http(&absolute) {
        return ImageSource::Blocked;
    }
    let host = host_of_url(&absolute);
    match account_for_host(&context.accounts, host.as_deref()) {
        Some(account) => ImageSource::Authenticated {
            account_id: account.id.clone(),
            url: absolute.to_string(),
        },
        None => ImageSource::Remote(absolute.to_string()),
    }
}

/// An image source with no context to resolve against: an absolute http(s) URL is
/// fetched as it is, and anything else cannot be fetched at all.
pub fn plain_image_source(source: &str) -> ImageSource {
    match absolute_source(source, None) {
        Some(url) if is_http(&url) => ImageSource::Remote(url.to_string()),
        _ => ImageSource::Blocked,
    }
}

fn absolute_source(source: &str, repo_root: Option<&str>) -> Option<Url> {
    let trimmed = source.trim();
    if trimmed.is_empty() || trimmed.starts_with("data:") {
        return None;
    }
    match repo_root {
        // A relative source only means something against the repository it was
        // written in.
        Some(root) => {
            let base = Url::parse(&format!("{}/", root.trim_end_matches('/'))).ok()?;
            base.join(trimmed).ok()
        }
        None => Url::parse(trimmed).ok(),
    }
}

/// An authenticated image request that passed [`resolve_image_request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImageRequest {
    pub account_id: String,
    /// The target, normalised (lower-case host and so on).
    pub target: String,
}

/// Checks an authenticated image request, and refuses anything it cannot vouch for.
///
/// The host of the target has to belong to the account the request names. Checking
/// the account exists would not be enough on its own: a body written by someone else
/// could otherwise pair a real account with a host of their choosing and have that
/// account's token posted to it.
///
/// Returns `None` rather than an error, so a malformed request is a failed image.
pub fn resolve_image_request(
    account_id: &str,
    target: &str,
    accounts: &[ImageAccount],
) -> Option<ResolvedImageRequest> {
    if account_id.is_empty() || target.is_empty() {
        return None;
    }
    let destination = Url::parse(target).ok()?;
    if !is_http(&destination) {
        return None;
    }
    let account = accounts
        .iter()
        .find(|candidate| candidate.id == account_id)?;
    let host = host_of_url(&destination)?;
    if !account_hosts(account).contains(&host) {
        return None;
    }
    Some(ResolvedImageRequest {
        account_id: account.id.clone(),
        target: destination.to_string(),
    })
}

/// How each host expects a token, plus the user agent. The same shapes the adapters
/// send - an upload URL is served by the instance itself, not by the API, but it
/// accepts the same credential.
pub fn image_authorization(account: &Account, token: &str) -> Vec<(String, String)> {
    let credential = match account.kind {
        ProviderKind::Github => ("Authorization".to_string(), format!("Bearer {token}")),
        ProviderKind::Gitlab => ("PRIVATE-TOKEN".to_string(), token.to_string()),
        ProviderKind::Forgejo => ("Authorization".to_string(), format!("token {token}")),
        ProviderKind::Bitbucket => (
            "Authorization".to_string(),
            format!(
                "Basic {}",
                base64_encode(format!("{}:{token}", account.username).as_bytes())
            ),
        ),
    };
    vec![
        credential,
        ("User-Agent".to_string(), "Reviewdeck".to_string()),
    ]
}

/// The headers of one hop: the credential only while `credentialed`.
pub fn image_request_headers(
    account: &Account,
    token: &str,
    credentialed: bool,
) -> Vec<(String, String)> {
    let mut headers = if credentialed {
        image_authorization(account, token)
    } else {
        vec![("User-Agent".to_string(), "Reviewdeck".to_string())]
    };
    headers.push(("Accept".to_string(), IMAGE_ACCEPT.to_string()));
    headers
}

/// Where a redirect goes: `location` resolved against the current URL, and only if
/// it is http(s).
pub fn redirect_target(current: &str, location: &str) -> Option<String> {
    let base = Url::parse(current).ok()?;
    let next = base.join(location).ok()?;
    is_http(&next).then(|| next.to_string())
}

/// Whether the credential may go with a request to `url`: only while it still has
/// it, and only to one of the account's hosts. Once dropped it never comes back.
pub fn keeps_credential(credentialed: bool, hosts: &[String], url: &str) -> bool {
    credentialed && host_of(url).is_some_and(|host| hosts.contains(&host))
}

/// Fetches an image with an account's credential, returning its bytes and its
/// `Content-Type`.
///
/// Refuses outright a URL that is not on one of the account's hosts. Follows
/// redirects by hand, so the credential stops at the account's own hosts: GitLab's
/// `PRIVATE-TOKEN` is an ordinary header that an automatic redirect would carry
/// straight on. An upload URL that hops to object storage is the everyday case, and
/// an open redirect on the instance is the adversarial one - in both the token has
/// no business at the far end. Once dropped the credential never comes back, so a
/// chain that returns to the account's host cannot pick it up again.
///
/// The whole exchange, redirects included, has [`IMAGE_FETCH_TIMEOUT`]; at most
/// [`MAX_IMAGE_REDIRECTS`] redirects are followed. A host that is down or slow is a
/// failed image, which the UI shows as its alt text.
pub async fn fetch_authenticated(
    http: &Http,
    account: &Account,
    token: &str,
    url: &str,
) -> Result<(Vec<u8>, Option<String>)> {
    let image_account = ImageAccount::from(account);
    let Some(resolved) =
        resolve_image_request(&account.id, url, std::slice::from_ref(&image_account))
    else {
        return Err(msg(format!(
            "Refused to send the {} token to {}.",
            account.kind.label(),
            safe_host(url)
        )));
    };

    let deadline = Instant::now() + IMAGE_FETCH_TIMEOUT;
    let hosts = account_hosts(&image_account);
    let mut current = resolved.target;
    let mut credentialed = true;
    let mut hop = 0;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::Api {
                status: 0,
                url: current.clone(),
                message: format!("Request to {} timed out.", safe_host(&current)),
                body: None,
            });
        }
        let options = RequestOptions::new(Method::Get)
            .headers(image_request_headers(account, token, credentialed))
            .timeout(remaining)
            .max_redirects(0);
        let response = http.send(&current, options).await?;

        let location = (300..400)
            .contains(&response.status)
            .then(|| response.header("location"))
            .flatten()
            .filter(|location| !location.is_empty());
        let Some(location) = location else {
            if !response.is_ok() {
                return Err(Error::Api {
                    status: response.status,
                    url: current.clone(),
                    message: describe(response.status, &current, ""),
                    body: None,
                });
            }
            let content_type = response.header("content-type");
            return Ok((response.body, content_type));
        };

        if hop >= MAX_IMAGE_REDIRECTS {
            return Err(Error::Api {
                status: 508,
                url: current.clone(),
                message: format!(
                    "Too many redirects fetching an image from {}.",
                    safe_host(&current)
                ),
                body: None,
            });
        }
        let Some(next) = redirect_target(&current, &location) else {
            return Err(Error::Api {
                status: 502,
                url: current.clone(),
                message: format!(
                    "An image on {} redirected somewhere it cannot be fetched from.",
                    safe_host(&current)
                ),
                body: None,
            });
        };
        credentialed = keeps_credential(credentialed, &hosts, &next);
        current = next;
        hop += 1;
    }
}

/// Standard base64 with padding, for Bitbucket's Basic credential.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::executor::block_on;
    use parking_lot::Mutex;

    use super::*;
    use crate::http::{MockRequest, MockResponse};

    fn gitlab() -> ImageAccount {
        ImageAccount {
            id: "acct-gitlab".into(),
            base_url: "https://gitlab.acme.dev/api/v4".into(),
            web_url: "https://gitlab.acme.dev".into(),
        }
    }

    fn forgejo() -> ImageAccount {
        ImageAccount {
            id: "acct-forgejo".into(),
            base_url: "https://codeberg.org/api/v1".into(),
            web_url: "https://codeberg.org".into(),
        }
    }

    fn accounts() -> Vec<ImageAccount> {
        vec![gitlab(), forgejo()]
    }

    fn context(repo_root: Option<&str>) -> ImageContext {
        ImageContext {
            repo_root: repo_root.map(str::to_string),
            accounts: accounts(),
        }
    }

    /// What the proxy round trip of the TypeScript tests checks: the rewrite names an
    /// account and a URL, and the request check accepts that pairing.
    fn resolved(source: &ImageSource) -> Option<ResolvedImageRequest> {
        match source {
            ImageSource::Authenticated { account_id, url } => {
                resolve_image_request(account_id, url, &accounts())
            }
            _ => None,
        }
    }

    // --- test/images.test.ts ---

    #[test]
    fn account_hosts_covers_the_web_root_and_the_api_root_without_repeating_one() {
        assert_eq!(account_hosts(&gitlab()), vec!["gitlab.acme.dev"]);
        assert_eq!(
            account_hosts(&ImageAccount {
                id: "x".into(),
                web_url: "https://github.com".into(),
                base_url: "https://api.github.com".into(),
            }),
            vec!["github.com", "api.github.com"]
        );
        assert_eq!(
            account_hosts(&ImageAccount {
                id: "x".into(),
                web_url: "not a url".into(),
                base_url: String::new(),
            }),
            Vec::<String>::new()
        );
    }

    #[test]
    fn rewrite_image_source_redirects_a_source_on_a_signed_in_host() {
        let source = "https://gitlab.acme.dev/acme/api/uploads/abc123/shot.png";
        let rewritten = rewrite_image_source(source, &context(None));

        assert_ne!(rewritten, ImageSource::Remote(source.into()));
        assert_eq!(
            resolved(&rewritten),
            Some(ResolvedImageRequest {
                account_id: "acct-gitlab".into(),
                target: source.into(),
            })
        );
    }

    #[test]
    fn rewrite_image_source_leaves_a_source_on_any_other_host_alone() {
        // A public badge or an external screenshot keeps loading over ordinary HTTPS,
        // with no credential anywhere near it.
        for source in [
            "https://img.shields.io/badge/build-passing-green.svg",
            "https://user-images.githubusercontent.com/1/shot.png",
            "https://gitlab.example.com/other/uploads/x.png",
        ] {
            assert_eq!(
                rewrite_image_source(source, &context(None)),
                ImageSource::Remote(source.into())
            );
        }
        // A data URI is never fetched, and certainly never with a credential.
        assert_eq!(
            rewrite_image_source("data:image/png;base64,iVBORw0KGgo=", &context(None)),
            ImageSource::Blocked
        );
    }

    #[test]
    fn rewrite_image_source_resolves_a_relative_source_against_the_repository_root_first() {
        let context = context(Some("https://codeberg.org/vmares/dotfiles"));

        // Whether it needs a credential depends on the host it lands on, so it has to
        // be made absolute before the decision, not after.
        let rewritten = rewrite_image_source("docs/shot.png", &context);
        assert_eq!(
            resolved(&rewritten),
            Some(ResolvedImageRequest {
                account_id: "acct-forgejo".into(),
                target: "https://codeberg.org/vmares/dotfiles/docs/shot.png".into(),
            })
        );

        let rooted = rewrite_image_source("/attachments/abc", &context);
        assert_eq!(
            resolved(&rooted),
            Some(ResolvedImageRequest {
                account_id: "acct-forgejo".into(),
                target: "https://codeberg.org/attachments/abc".into(),
            })
        );
    }

    #[test]
    fn rewrite_image_source_leaves_a_relative_source_alone_with_no_root_to_resolve_it_against() {
        assert_eq!(
            rewrite_image_source("docs/shot.png", &context(None)),
            ImageSource::Blocked
        );
        assert_eq!(
            rewrite_image_source("   ", &context(None)),
            ImageSource::Blocked
        );
    }

    #[test]
    fn resolve_image_request_rejects_a_host_that_is_not_a_signed_in_account() {
        // The whole security boundary: whoever acts on this attaches a credential to
        // whatever it accepts, so a body written by someone else must not be able to
        // name a host of its choosing and have a real account's token posted to it.
        assert_eq!(
            resolve_image_request("acct-gitlab", "https://evil.test/steal", &accounts()),
            None
        );

        // Nor by pairing one account with another account's host.
        assert_eq!(
            resolve_image_request(
                "acct-gitlab",
                "https://codeberg.org/attachments/abc",
                &accounts()
            ),
            None
        );

        // Nor a host that merely looks like one.
        assert_eq!(
            resolve_image_request(
                "acct-gitlab",
                "https://gitlab.acme.dev.evil.test/x.png",
                &accounts()
            ),
            None
        );
    }

    #[test]
    fn resolve_image_request_rejects_an_unknown_account() {
        let target = "https://gitlab.acme.dev/uploads/x.png";
        assert_eq!(
            resolve_image_request("acct-gone", target, &accounts()),
            None
        );
        assert_eq!(resolve_image_request("acct-gone", target, &[]), None);
    }

    #[test]
    fn resolve_image_request_rejects_a_malformed_request_rather_than_failing() {
        for (account, target) in [
            ("", ""),
            ("acct-gitlab", ""),
            ("", "https://gitlab.acme.dev/x.png"),
            ("acct-gitlab", "not a url at all"),
            ("acct-gitlab", "%%%"),
            ("acct-gitlab", "file:///etc/passwd"),
            (
                "acct-gitlab",
                "reviewdeck-image://fetch/?account=acct-gitlab&url=x",
            ),
            ("acct-gitlab", "javascript:alert(1)"),
        ] {
            assert_eq!(
                resolve_image_request(account, target, &accounts()),
                None,
                "accepted: {account} {target}"
            );
        }
    }

    #[test]
    fn resolve_image_request_accepts_a_request_it_can_vouch_for_whatever_the_case_of_the_host() {
        assert_eq!(
            resolve_image_request(
                "acct-gitlab",
                "https://GitLab.Acme.Dev/acme/api/uploads/a/shot.png",
                &accounts()
            ),
            Some(ResolvedImageRequest {
                account_id: "acct-gitlab".into(),
                target: "https://gitlab.acme.dev/acme/api/uploads/a/shot.png".into(),
            })
        );
    }

    // --- Rust-specific: the request policy of src/main/images.ts ---

    fn account(kind: ProviderKind, web: &str, base: &str) -> Account {
        Account {
            id: "acct".into(),
            kind,
            label: "Work".into(),
            base_url: base.into(),
            web_url: web.into(),
            username: "vmares".into(),
            display_name: "V".into(),
            avatar_url: String::new(),
            added_at: String::new(),
            agent_command: None,
        }
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn image_authorization_sends_each_host_its_own_credential_shape() {
        let github = account(
            ProviderKind::Github,
            "https://github.com",
            "https://api.github.com",
        );
        let headers = image_authorization(&github, "t0k");
        assert_eq!(header(&headers, "authorization"), Some("Bearer t0k"));
        assert_eq!(header(&headers, "user-agent"), Some("Reviewdeck"));

        let gitlab = account(
            ProviderKind::Gitlab,
            "https://g.test",
            "https://g.test/api/v4",
        );
        assert_eq!(
            header(&image_authorization(&gitlab, "t0k"), "private-token"),
            Some("t0k")
        );

        let forgejo = account(
            ProviderKind::Forgejo,
            "https://f.test",
            "https://f.test/api/v1",
        );
        assert_eq!(
            header(&image_authorization(&forgejo, "t0k"), "authorization"),
            Some("token t0k")
        );

        let bitbucket = account(
            ProviderKind::Bitbucket,
            "https://bitbucket.org",
            "https://api.bitbucket.org/2.0",
        );
        // base64("vmares:t0k")
        assert_eq!(
            header(&image_authorization(&bitbucket, "t0k"), "authorization"),
            Some("Basic dm1hcmVzOnQwaw==")
        );
    }

    #[test]
    fn base64_encode_pads_like_buffer_to_string_base64() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode("ž:ü".as_bytes()), "xb46w7w=");
    }

    #[test]
    fn image_request_headers_drop_the_credential_but_keep_accept() {
        let gitlab = account(
            ProviderKind::Gitlab,
            "https://g.test",
            "https://g.test/api/v4",
        );
        let with = image_request_headers(&gitlab, "t0k", true);
        assert_eq!(header(&with, "private-token"), Some("t0k"));
        assert_eq!(header(&with, "accept"), Some(IMAGE_ACCEPT));

        let without = image_request_headers(&gitlab, "t0k", false);
        assert_eq!(header(&without, "private-token"), None);
        assert_eq!(header(&without, "user-agent"), Some("Reviewdeck"));
        assert_eq!(header(&without, "accept"), Some(IMAGE_ACCEPT));
    }

    #[test]
    fn redirect_target_resolves_relative_locations_and_refuses_other_schemes() {
        assert_eq!(
            redirect_target("https://g.test/a/b.png", "/c.png").as_deref(),
            Some("https://g.test/c.png")
        );
        assert_eq!(
            redirect_target("https://g.test/a/b.png", "https://s3.test/x").as_deref(),
            Some("https://s3.test/x")
        );
        assert_eq!(
            redirect_target("https://g.test/a", "file:///etc/passwd"),
            None
        );
        assert_eq!(
            redirect_target("https://g.test/a", "javascript:alert(1)"),
            None
        );
    }

    /// A mock host that records every request it is asked.
    fn recording(
        handler: impl Fn(&MockRequest) -> MockResponse + Send + Sync + 'static,
    ) -> (Http, Arc<Mutex<Vec<MockRequest>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let http = Http::mock(move |request| {
            log.lock().push(request.clone());
            handler(request)
        });
        (http, seen)
    }

    fn gitlab_account() -> Account {
        account(
            ProviderKind::Gitlab,
            "https://gitlab.acme.dev",
            "https://gitlab.acme.dev/api/v4",
        )
    }

    #[test]
    fn fetch_authenticated_returns_the_body_and_its_type() {
        let (http, seen) = recording(|_| {
            MockResponse::new(200, b"PNG".to_vec()).header("Content-Type", "image/png")
        });
        let fetched = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://gitlab.acme.dev/acme/api/uploads/a/shot.png",
        ));
        let (body, content_type) = fetched.unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(body, b"PNG");
        assert_eq!(content_type.as_deref(), Some("image/png"));

        let seen = seen.lock();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].header("PRIVATE-TOKEN"), Some("secret"));
        assert_eq!(seen[0].header("Accept"), Some(IMAGE_ACCEPT));
    }

    #[test]
    fn fetch_authenticated_refuses_a_url_off_the_account_hosts_before_sending_anything() {
        let (http, seen) = recording(|_| MockResponse::new(200, Vec::new()));
        let result = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://evil.test/steal",
        ));
        assert!(result.is_err());
        assert!(seen.lock().is_empty());
    }

    #[test]
    fn fetch_authenticated_drops_the_credential_when_a_redirect_leaves_the_account_and_never_takes_it_back()
     {
        let (http, seen) = recording(|request| match request.url.as_str() {
            "https://gitlab.acme.dev/uploads/a.png" => {
                MockResponse::new(302, Vec::new()).header("Location", "https://s3.test/a.png")
            }
            "https://s3.test/a.png" => MockResponse::new(301, Vec::new())
                .header("Location", "https://gitlab.acme.dev/again.png"),
            _ => MockResponse::new(200, b"ok".to_vec()),
        });
        let result = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://gitlab.acme.dev/uploads/a.png",
        ));
        assert!(result.is_ok());

        let seen = seen.lock();
        let urls: Vec<&str> = seen.iter().map(|request| request.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://gitlab.acme.dev/uploads/a.png",
                "https://s3.test/a.png",
                "https://gitlab.acme.dev/again.png",
            ]
        );
        assert_eq!(seen[0].header("PRIVATE-TOKEN"), Some("secret"));
        assert_eq!(seen[1].header("PRIVATE-TOKEN"), None);
        // Back on the account's host, but the credential stays dropped.
        assert_eq!(seen[2].header("PRIVATE-TOKEN"), None);
        assert_eq!(seen[2].header("User-Agent"), Some("Reviewdeck"));
    }

    #[test]
    fn fetch_authenticated_keeps_the_credential_on_a_redirect_within_the_account() {
        let (http, seen) = recording(|request| {
            if request.url.ends_with("/a.png") {
                MockResponse::new(302, Vec::new()).header("Location", "/b.png")
            } else {
                MockResponse::new(200, b"ok".to_vec())
            }
        });
        let result = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://gitlab.acme.dev/a.png",
        ));
        assert!(result.is_ok());
        let seen = seen.lock();
        assert_eq!(seen[1].url, "https://gitlab.acme.dev/b.png");
        assert_eq!(seen[1].header("PRIVATE-TOKEN"), Some("secret"));
    }

    #[test]
    fn fetch_authenticated_stops_after_four_redirects() {
        let (http, seen) = recording(|request| {
            let hop: u32 = request
                .url
                .rsplit('/')
                .next()
                .and_then(|last| last.parse().ok())
                .unwrap_or(0);
            MockResponse::new(302, Vec::new())
                .header("Location", format!("https://gitlab.acme.dev/{}", hop + 1))
        });
        let result = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://gitlab.acme.dev/0",
        ));
        assert!(matches!(result, Err(Error::Api { status: 508, .. })));
        // The original request plus four redirects.
        assert_eq!(seen.lock().len(), 5);
    }

    #[test]
    fn fetch_authenticated_refuses_a_redirect_to_another_scheme() {
        let (http, seen) = recording(|_| {
            MockResponse::new(302, Vec::new()).header("Location", "file:///etc/passwd")
        });
        let result = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://gitlab.acme.dev/a.png",
        ));
        assert!(matches!(result, Err(Error::Api { status: 502, .. })));
        assert_eq!(seen.lock().len(), 1);
    }

    #[test]
    fn fetch_authenticated_fails_on_an_error_status() {
        let (http, _) = recording(|_| MockResponse::new(404, Vec::new()));
        let result = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://gitlab.acme.dev/a.png",
        ));
        assert!(matches!(result, Err(Error::Api { status: 404, .. })));

        // A 3xx without a Location is a response like any other: not an image.
        let (http, _) = recording(|_| MockResponse::new(304, Vec::new()));
        let result = block_on(fetch_authenticated(
            &http,
            &gitlab_account(),
            "secret",
            "https://gitlab.acme.dev/a.png",
        ));
        assert!(matches!(result, Err(Error::Api { status: 304, .. })));
    }

    #[test]
    fn plain_image_source_fetches_only_absolute_http() {
        assert_eq!(
            plain_image_source("https://img.shields.io/x.svg"),
            ImageSource::Remote("https://img.shields.io/x.svg".into())
        );
        assert_eq!(plain_image_source("docs/x.png"), ImageSource::Blocked);
        assert_eq!(
            plain_image_source("javascript:alert(1)"),
            ImageSource::Blocked
        );
        assert_eq!(
            plain_image_source("data:image/png;base64,AA"),
            ImageSource::Blocked
        );
    }

    #[test]
    fn account_for_host_matches_any_case_and_nothing_for_no_host() {
        let accounts = accounts();
        assert_eq!(
            account_for_host(&accounts, Some("CodeBerg.org")).map(|a| a.id.as_str()),
            Some("acct-forgejo")
        );
        assert_eq!(account_for_host(&accounts, None), None);
        assert_eq!(account_for_host(&accounts, Some("")), None);
    }
}
