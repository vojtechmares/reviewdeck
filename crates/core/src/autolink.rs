//! The host conventions a pull request body is written in: `@someone`, `#123` and
//! `:tada:`. A port of src/shared/autolink.ts.
//!
//! Everything here is a pure function over one string, because the transform that
//! uses them ([`crate::markdown`]) runs over the text nodes of the parsed document
//! rather than over the raw source. That distinction is the whole correctness
//! story: a regular expression over the source would happily linkify a reference
//! sitting inside a code span, which is exactly where a code sample discussing a
//! number puts one.
//!
//! Offsets are byte offsets into the string (the TypeScript counts UTF-16 code
//! units); they always fall on character boundaries, so slicing with them is safe.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

use crate::model::{ProviderKind, ReviewItem};

/// The path segments each host puts between a repository and a pull request number.
/// GitLab moved merge requests under `/-/` some versions ago, so both shapes are
/// recognised.
fn pull_segments(provider: ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::Github => &["/pull/"],
        ProviderKind::Gitlab => &["/-/merge_requests/", "/merge_requests/"],
        ProviderKind::Forgejo => &["/pulls/"],
        ProviderKind::Bitbucket => &["/pull-requests/"],
    }
}

/// Where each host puts an issue, relative to the repository root. `None` where the
/// host has no equivalent, in which case the reference stays plain text rather than
/// becoming a wrong link.
///
/// Bitbucket has neither an issue path nor a profile path: its markdown does not
/// treat `#123` as a reference at all, and a mention there addresses an account id
/// rather than the display name that appears in the text.
fn issue_path(provider: ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::Github => Some("issues"),
        ProviderKind::Gitlab => Some("-/issues"),
        ProviderKind::Forgejo => Some("issues"),
        ProviderKind::Bitbucket => None,
    }
}

/// Whether a host puts profiles at `<web root>/<handle>`.
fn has_profile_path(provider: ProviderKind) -> bool {
    !matches!(provider, ProviderKind::Bitbucket)
}

/// The repository's web root, recovered from the pull request's own web URL.
///
/// Deliberately not the account's web root joined to the repository name: the
/// GitLab adapter falls back to a numeric project id for that field, so the naive
/// construction would build links that go nowhere. Returns `None` when the URL is
/// not the shape we expect, so a caller produces plain text rather than a wrong
/// link.
pub fn repository_root(provider: ProviderKind, url: &str) -> Option<String> {
    pull_segments(provider).iter().find_map(|segment| {
        let at = url.rfind(segment)?;
        (at > 0).then(|| url[..at].to_string())
    })
}

/// [`repository_root`] of a review item.
pub fn repository_root_of(item: &ReviewItem) -> Option<String> {
    repository_root(item.provider, &item.url)
}

/// `webUrl.replace(/\/+$/, '')`.
fn without_trailing_slashes(value: &str) -> &str {
    value.trim_end_matches('/')
}

/// The profile a mention points at, on the host's web root. `None` where the host
/// has no profile path, or there is no web root to put it on.
pub fn mention_url(provider: ProviderKind, web_url: &str, handle: &str) -> Option<String> {
    if !has_profile_path(provider) || web_url.is_empty() {
        return None;
    }
    Some(format!("{}/{handle}", without_trailing_slashes(web_url)))
}

/// The issue a `#number` reference points at. `None` where the host has no issue
/// path, or the repository root could not be recovered.
pub fn issue_url(provider: ProviderKind, repo_root: Option<&str>, number: u64) -> Option<String> {
    let path = issue_path(provider)?;
    let root = repo_root.filter(|root| !root.is_empty())?;
    Some(format!(
        "{}/{path}/{number}",
        without_trailing_slashes(root)
    ))
}

/// One `@mention` or `#123` reference found in a string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutolinkSpan {
    Mention {
        /// Byte offset of the `@`.
        start: usize,
        /// Byte offset just past the handle.
        end: usize,
        /// `@` plus the handle, as it appears.
        text: String,
        handle: String,
    },
    Issue {
        /// Byte offset of the `#`.
        start: usize,
        /// Byte offset just past the digits.
        end: usize,
        /// `#` plus the digits, as they appear.
        text: String,
        number: u64,
    },
}

impl AutolinkSpan {
    pub fn start(&self) -> usize {
        match self {
            AutolinkSpan::Mention { start, .. } | AutolinkSpan::Issue { start, .. } => *start,
        }
    }

    pub fn end(&self) -> usize {
        match self {
            AutolinkSpan::Mention { end, .. } | AutolinkSpan::Issue { end, .. } => *end,
        }
    }

    pub fn text(&self) -> &str {
        match self {
            AutolinkSpan::Mention { text, .. } | AutolinkSpan::Issue { text, .. } => text,
        }
    }
}

/// A mention is `@` plus a handle, and a reference is `#` plus digits. Both have to
/// start at a boundary: without that, an email address would read as a mention and
/// a CSS colour as a reference.
///
/// JavaScript's `\w` is ASCII-only, so the class is spelt out rather than left to
/// the Unicode-aware `\w` of the regex crate.
static AUTOLINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^A-Za-z0-9_/@#-])(?:@([A-Za-z0-9][A-Za-z0-9._-]*)|#([0-9]+))")
        .expect("valid pattern")
});

/// Every mention and issue reference in `value`, in order.
///
/// A reference whose number does not fit a `u64` is left out, so it stays plain
/// text (the TypeScript would build a link to `.../issues/1e+23`).
pub fn autolink_spans(value: &str) -> Vec<AutolinkSpan> {
    let mut spans = Vec::new();

    for captures in AUTOLINK.captures_iter(value) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let lead = captures.get(1).map_or(0, |lead| lead.len());
        let start = whole.start() + lead;
        let text = &value[start..whole.end()];

        if let Some(handle) = captures.get(2) {
            // A trailing dot or dash is punctuation, not part of the handle.
            let handle = handle.as_str().trim_end_matches(['.', '_', '-']);
            if !handle.is_empty() {
                spans.push(AutolinkSpan::Mention {
                    start,
                    end: start + 1 + handle.len(),
                    text: format!("@{handle}"),
                    handle: handle.to_string(),
                });
            }
        } else if let Some(digits) = captures.get(3)
            && let Ok(number) = digits.as_str().parse::<u64>()
        {
            spans.push(AutolinkSpan::Issue {
                start,
                end: whole.end(),
                text: text.to_string(),
                number,
            });
        }
    }

    spans
}

/// A curated table rather than the full set: these are the shortcodes that actually
/// turn up in pull requests, and carrying a few thousand entries to catch the rest
/// would cost more than it is worth. An unknown shortcode renders as it was typed.
const EMOJI: &[(&str, &str)] = &[
    ("+1", "👍"),
    ("-1", "👎"),
    ("100", "💯"),
    ("ambulance", "🚑"),
    ("art", "🎨"),
    ("bell", "🔔"),
    ("book", "📖"),
    ("books", "📚"),
    ("boom", "💥"),
    ("bug", "🐛"),
    ("bulb", "💡"),
    ("cat", "🐱"),
    ("checkered_flag", "🏁"),
    ("clap", "👏"),
    ("computer", "💻"),
    ("confused", "😕"),
    ("construction", "🚧"),
    ("cry", "😢"),
    ("dart", "🎯"),
    ("eyes", "👀"),
    ("fire", "🔥"),
    ("green_heart", "💚"),
    ("hammer", "🔨"),
    ("heart", "❤️"),
    ("heavy_check_mark", "✔️"),
    ("hourglass", "⏳"),
    ("joy", "😂"),
    ("key", "🔑"),
    ("lipstick", "💄"),
    ("lock", "🔒"),
    ("loud_sound", "🔊"),
    ("mag", "🔍"),
    ("memo", "📝"),
    ("ok_hand", "👌"),
    ("package", "📦"),
    ("partying_face", "🥳"),
    ("pencil", "✏️"),
    ("pray", "🙏"),
    ("question", "❓"),
    ("raised_hands", "🙌"),
    ("recycle", "♻️"),
    ("rocket", "🚀"),
    ("rotating_light", "🚨"),
    ("see_no_evil", "🙈"),
    ("shipit", "🚢"),
    ("shrug", "🤷"),
    ("smile", "😄"),
    ("sob", "😭"),
    ("sparkles", "✨"),
    ("star", "⭐"),
    ("tada", "🎉"),
    ("test_tube", "🧪"),
    ("thinking", "🤔"),
    ("thumbsdown", "👎"),
    ("thumbsup", "👍"),
    ("truck", "🚚"),
    ("warning", "⚠️"),
    ("wave", "👋"),
    ("white_check_mark", "✅"),
    ("wrench", "🔧"),
    ("x", "❌"),
    ("zap", "⚡"),
];

/// The emoji a shortcode (without its colons) stands for, if we carry it.
pub fn emoji_for(shortcode: &str) -> Option<&'static str> {
    EMOJI
        .iter()
        .find(|(name, _)| *name == shortcode)
        .map(|(_, emoji)| *emoji)
}

static SHORTCODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":([a-z0-9_+-]+):").expect("valid pattern"));

/// Replaces the shortcodes we know; leaves the rest exactly as they were typed.
/// Borrows when there is nothing to replace.
pub fn render_emoji(value: &str) -> Cow<'_, str> {
    if !value.contains(':') {
        return Cow::Borrowed(value);
    }
    SHORTCODE.replace_all(value, |captures: &regex::Captures<'_>| {
        let whole = captures.get(0).map_or("", |m| m.as_str());
        let name = captures.get(1).map_or("", |m| m.as_str());
        emoji_for(name).unwrap_or(whole).to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- test/autolink.test.ts ---

    #[test]
    fn repository_root_strips_the_pull_request_segment_off_each_host_url_shape() {
        assert_eq!(
            repository_root(
                ProviderKind::Github,
                "https://github.com/acme/checkout-api/pull/412"
            )
            .as_deref(),
            Some("https://github.com/acme/checkout-api")
        );
        assert_eq!(
            repository_root(
                ProviderKind::Gitlab,
                "https://gitlab.acme.dev/group/sub/project/-/merge_requests/88"
            )
            .as_deref(),
            Some("https://gitlab.acme.dev/group/sub/project")
        );
        assert_eq!(
            repository_root(
                ProviderKind::Forgejo,
                "https://codeberg.org/vmares/dotfiles/pulls/9"
            )
            .as_deref(),
            Some("https://codeberg.org/vmares/dotfiles")
        );
        assert_eq!(
            repository_root(
                ProviderKind::Bitbucket,
                "https://bitbucket.org/acme/website/pull-requests/12"
            )
            .as_deref(),
            Some("https://bitbucket.org/acme/website")
        );
    }

    #[test]
    fn repository_root_recovers_the_gitlab_root_the_naive_construction_would_get_wrong() {
        // The GitLab adapter keeps the numeric project id in repoKey, so joining the
        // account web root to it would build https://gitlab.acme.dev/4711 - a dead
        // link. The item's own URL is the only place the real path survives.
        assert_eq!(
            repository_root(
                ProviderKind::Gitlab,
                "https://gitlab.acme.dev/acme/design-tokens/-/merge_requests/88"
            )
            .as_deref(),
            Some("https://gitlab.acme.dev/acme/design-tokens")
        );

        // Older GitLab served merge requests without the /-/ separator.
        assert_eq!(
            repository_root(
                ProviderKind::Gitlab,
                "https://gitlab.acme.dev/acme/x/merge_requests/3"
            )
            .as_deref(),
            Some("https://gitlab.acme.dev/acme/x")
        );
    }

    #[test]
    fn repository_root_gives_up_rather_than_guessing_at_an_unexpected_url() {
        assert_eq!(
            repository_root(ProviderKind::Github, "https://github.com/acme/repo"),
            None
        );
        assert_eq!(repository_root(ProviderKind::Github, ""), None);
        // A segment at the very start leaves no root in front of it.
        assert_eq!(repository_root(ProviderKind::Github, "/pull/3"), None);
    }

    #[test]
    fn mention_url_points_at_the_profile_on_the_host_web_root() {
        assert_eq!(
            mention_url(ProviderKind::Github, "https://github.com", "octocat").as_deref(),
            Some("https://github.com/octocat")
        );
        assert_eq!(
            mention_url(ProviderKind::Gitlab, "https://gitlab.acme.dev/", "vmares").as_deref(),
            Some("https://gitlab.acme.dev/vmares")
        );
        assert_eq!(
            mention_url(ProviderKind::Forgejo, "https://codeberg.org", "vmares").as_deref(),
            Some("https://codeberg.org/vmares")
        );
    }

    #[test]
    fn mention_url_produces_nothing_where_the_host_has_no_profile_path() {
        // A Bitbucket mention addresses an account id, not the name in the text.
        assert_eq!(
            mention_url(ProviderKind::Bitbucket, "https://bitbucket.org", "someone"),
            None
        );
        assert_eq!(mention_url(ProviderKind::Github, "", "octocat"), None);
    }

    #[test]
    fn issue_url_uses_the_right_path_shape_per_provider() {
        assert_eq!(
            issue_url(
                ProviderKind::Github,
                Some("https://github.com/acme/api"),
                12
            )
            .as_deref(),
            Some("https://github.com/acme/api/issues/12")
        );
        assert_eq!(
            issue_url(
                ProviderKind::Gitlab,
                Some("https://gitlab.acme.dev/acme/api"),
                12
            )
            .as_deref(),
            Some("https://gitlab.acme.dev/acme/api/-/issues/12")
        );
        assert_eq!(
            issue_url(
                ProviderKind::Forgejo,
                Some("https://codeberg.org/vmares/dotfiles"),
                12
            )
            .as_deref(),
            Some("https://codeberg.org/vmares/dotfiles/issues/12")
        );
    }

    #[test]
    fn issue_url_produces_nothing_without_a_host_path_or_a_repository_root() {
        assert_eq!(
            issue_url(
                ProviderKind::Bitbucket,
                Some("https://bitbucket.org/acme/web"),
                12
            ),
            None
        );
        assert_eq!(issue_url(ProviderKind::Github, None, 12), None);
    }

    #[test]
    fn autolink_spans_finds_mentions_and_references_in_ordinary_text() {
        assert_eq!(
            autolink_spans("cc @mnovotna about #412 please"),
            vec![
                AutolinkSpan::Mention {
                    start: 3,
                    end: 12,
                    text: "@mnovotna".into(),
                    handle: "mnovotna".into(),
                },
                AutolinkSpan::Issue {
                    start: 19,
                    end: 23,
                    text: "#412".into(),
                    number: 412,
                },
            ]
        );

        assert_eq!(
            autolink_spans("@vmares opened it").first(),
            Some(&AutolinkSpan::Mention {
                start: 0,
                end: 7,
                text: "@vmares".into(),
                handle: "vmares".into(),
            })
        );
    }

    #[test]
    fn autolink_spans_needs_a_boundary_so_an_address_or_a_colour_is_not_a_reference() {
        assert_eq!(autolink_spans("write to vojtech@mares.cz"), vec![]);
        assert_eq!(autolink_spans("the colour is #ff8800 now"), vec![]);
        assert_eq!(autolink_spans("see https://example.test/@someone"), vec![]);
    }

    #[test]
    fn autolink_spans_leaves_trailing_punctuation_out_of_a_handle() {
        assert_eq!(
            autolink_spans("thanks @lpeters."),
            vec![AutolinkSpan::Mention {
                start: 7,
                end: 15,
                text: "@lpeters".into(),
                handle: "lpeters".into(),
            }]
        );
    }

    #[test]
    fn emoji_for_resolves_a_known_shortcode_and_refuses_an_unknown_one() {
        assert_eq!(emoji_for("tada"), Some("🎉"));
        assert_eq!(emoji_for("white_check_mark"), Some("✅"));
        assert_eq!(emoji_for("+1"), Some("👍"));
        assert_eq!(emoji_for("not_an_emoji_we_carry"), None);
    }

    #[test]
    fn render_emoji_replaces_what_it_knows_and_leaves_the_rest_as_typed() {
        assert_eq!(render_emoji("Shipped :tada: at last"), "Shipped 🎉 at last");
        assert_eq!(
            render_emoji("nothing :obscure_thing: here"),
            "nothing :obscure_thing: here"
        );
        assert_eq!(
            render_emoji("ratio 3:4:5 unchanged"),
            "ratio 3:4:5 unchanged"
        );
    }

    // --- Rust-specific ---

    #[test]
    fn autolink_spans_offsets_are_byte_offsets_on_character_boundaries() {
        let value = "žluťoučký @kůň #7";
        let spans = autolink_spans(value);
        // A handle stops at the first character outside ASCII.
        assert_eq!(spans.len(), 2);
        for span in &spans {
            assert_eq!(&value[span.start()..span.end()], span.text());
        }
        assert_eq!(spans[0].text(), "@k");
        assert_eq!(spans[1].text(), "#7");
        // Non-ASCII letters are a boundary, as they are for JavaScript's `\w`.
        assert_eq!(autolink_spans("é@a").len(), 1);
    }

    #[test]
    fn autolink_spans_skips_a_number_too_large_to_link() {
        assert_eq!(autolink_spans("#99999999999999999999999"), vec![]);
        assert_eq!(
            autolink_spans("#007"),
            vec![AutolinkSpan::Issue {
                start: 0,
                end: 4,
                text: "#007".into(),
                number: 7,
            }]
        );
    }

    #[test]
    fn autolink_spans_skips_a_handle_that_is_only_punctuation_after_its_first_character() {
        // `@a-` trims to `@a`; adjacent references each need their own boundary.
        assert_eq!(autolink_spans("@a- #1#2").len(), 2);
        assert_eq!(autolink_spans("a@b"), vec![]);
    }

    #[test]
    fn render_emoji_borrows_when_nothing_changes() {
        assert!(matches!(render_emoji("plain"), Cow::Borrowed(_)));
        assert_eq!(render_emoji(":+1::-1:"), "👍👎");
    }

    #[test]
    fn repository_root_of_reads_the_item() {
        let item: ReviewItem = serde_json::from_value(serde_json::json!({
            "id": "a:1", "accountId": "a", "provider": "github", "repoKey": "acme/api",
            "repo": "acme/api", "number": 1, "title": "t",
            "url": "https://github.com/acme/api/pull/1",
            "author": {"name": "U", "avatarUrl": ""},
            "createdAt": "", "updatedAt": "", "draft": false, "sourceBranch": "a",
            "targetBranch": "b", "labels": [], "myReviewState": "pending",
            "approvals": {"given": 0, "outcome": "none_required"},
            "checks": {"status": "unknown", "passed": 0, "failed": 0, "running": 0, "total": 0, "runs": []}
        }))
        .unwrap_or_else(|error| panic!("fixture: {error}"));
        assert_eq!(
            repository_root_of(&item).as_deref(),
            Some("https://github.com/acme/api")
        );
    }
}
