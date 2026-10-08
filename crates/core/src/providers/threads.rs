//! Where each host's idea of a conversation becomes the one thread shape the rest
//! of the app knows about: a port of src/main/providers/threads.ts.
//!
//! Every provider difference is absorbed here rather than leaking upwards, the same
//! way check roll-up already works: the UI reads `can_reply` and `can_resolve` and
//! never asks which host it is talking to.
//!
//! Nothing in this module does any I/O, so all of it stays reachable from the tests.
//!
//! # The host shapes
//!
//! The structs below are what each host's JSON deserialises into. They are lenient
//! on purpose, because one odd field must not cost the reviewer every comment on a
//! pull request: unknown fields are ignored, every field may be missing or `null`,
//! an id may arrive as a number or a string (the TypeScript only ever passes it
//! through `String(...)`), a line number that is not a usable line reads as absent,
//! and a nested object of the wrong shape reads as absent rather than failing the
//! whole response. Flags keep the TypeScript's two readings: those it tests for
//! truthiness (`!note.system`) accept any JSON value by JavaScript's rules, and
//! those it compares with `=== true` (`isResolved === true`) are true only for a
//! literal `true`.

use std::collections::{HashMap, HashSet};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::model::{CommentThread, PullComment, Side, User};

// --- Lenient deserialisation -------------------------------------------------------

/// JavaScript's truthiness of a JSON value, for the flags the TypeScript reads with
/// `Boolean(...)` or `!`.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// A flag read by truthiness. Never fails.
fn flag<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(truthy(&Value::deserialize(deserializer)?))
}

/// A flag the TypeScript compares with `=== true`: only a literal `true` counts.
fn exactly_true<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(Value::deserialize(deserializer)? == Value::Bool(true))
}

/// An identifier as the decimal or literal text the thread will carry - the
/// TypeScript's `String(comment.id)`. A number or a string; anything else is empty.
fn id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(optional_id(deserializer)?.unwrap_or_default())
}

fn optional_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => Some(text),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

/// Text that is always there in the host's own schema (a body, a timestamp); a
/// missing or non-string value reads as empty rather than failing the response.
fn text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(optional_text(deserializer)?.unwrap_or_default())
}

fn optional_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => Some(text),
        _ => None,
    })
}

/// A line number. A non-negative whole number that fits, given as a number or as a
/// numeric string; anything else - `null`, a negative, a fraction - is no line.
fn line<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    Ok(whole_number(&Value::deserialize(deserializer)?).and_then(|n| u32::try_from(n).ok()))
}

/// A signed count (Forgejo's `extra_lines_count`), read as leniently as a line.
fn count<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i64>, D::Error> {
    Ok(whole_number(&Value::deserialize(deserializer)?))
}

fn whole_number(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|n| n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64)
                .map(|n| n as i64)
        }),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A nested object, or `None` when it is absent, `null` or not an object at all.
fn object<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(deserializer)? {
        value @ Value::Object(_) => serde_json::from_value(value).ok(),
        _ => None,
    })
}

/// A list, keeping the entries that read and dropping `null` and malformed ones.
fn list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Array(entries) => entries
            .into_iter()
            .filter(|entry| entry.is_object())
            .filter_map(|entry| serde_json::from_value(entry).ok())
            .collect(),
        _ => Vec::new(),
    })
}

// --- Host shapes ------------------------------------------------------------------

/// The `user` of a GitHub REST comment, and of a Forgejo one (Forgejo mirrors the
/// GitHub API here). GitHub sends `null` for a deleted ("ghost") account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct HostUser {
    #[serde(deserialize_with = "optional_text")]
    pub login: Option<String>,
    #[serde(deserialize_with = "optional_text")]
    pub avatar_url: Option<String>,
}

/// A GitHub REST comment: an issue comment, or a review comment left on a line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GithubComment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(deserialize_with = "object")]
    pub user: Option<HostUser>,
    #[serde(deserialize_with = "text")]
    pub body: String,
    #[serde(deserialize_with = "text")]
    pub created_at: String,
    #[serde(deserialize_with = "optional_text")]
    pub path: Option<String>,
    #[serde(deserialize_with = "line")]
    pub line: Option<u32>,
    #[serde(deserialize_with = "line")]
    pub original_line: Option<u32>,
    #[serde(deserialize_with = "optional_text")]
    pub side: Option<String>,
    /// Set when the comment covers several lines; `line` is then the last of them.
    #[serde(deserialize_with = "line")]
    pub start_line: Option<u32>,
    #[serde(deserialize_with = "line")]
    pub original_start_line: Option<u32>,
    #[serde(deserialize_with = "optional_text")]
    pub start_side: Option<String>,
}

/// The `author` of a GitLab note.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GitlabAuthor {
    #[serde(deserialize_with = "optional_text")]
    pub username: Option<String>,
    #[serde(deserialize_with = "optional_text")]
    pub avatar_url: Option<String>,
}

/// One end of a GitLab note's `line_range`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GitlabLinePoint {
    #[serde(deserialize_with = "line")]
    pub new_line: Option<u32>,
    #[serde(deserialize_with = "line")]
    pub old_line: Option<u32>,
}

/// Set on a GitLab note that covers several lines; the position itself is the last.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GitlabLineRange {
    #[serde(deserialize_with = "object")]
    pub start: Option<GitlabLinePoint>,
    #[serde(deserialize_with = "object")]
    pub end: Option<GitlabLinePoint>,
}

/// Where in the diff a GitLab note was left.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GitlabPosition {
    #[serde(deserialize_with = "optional_text")]
    pub new_path: Option<String>,
    #[serde(deserialize_with = "optional_text")]
    pub old_path: Option<String>,
    #[serde(deserialize_with = "line")]
    pub new_line: Option<u32>,
    #[serde(deserialize_with = "line")]
    pub old_line: Option<u32>,
    #[serde(deserialize_with = "optional_text")]
    pub head_sha: Option<String>,
    /// Set when the note covers several lines; the position itself is the last.
    #[serde(deserialize_with = "object")]
    pub line_range: Option<GitlabLineRange>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GitlabNote {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(deserialize_with = "text")]
    pub body: String,
    #[serde(deserialize_with = "object")]
    pub author: Option<GitlabAuthor>,
    #[serde(deserialize_with = "text")]
    pub created_at: String,
    /// Read by truthiness, as the TypeScript's `!note.system` does.
    #[serde(deserialize_with = "flag")]
    pub system: bool,
    /// Read by truthiness.
    #[serde(deserialize_with = "flag")]
    pub resolvable: bool,
    /// True only for a literal `true` (`note.resolved === true`).
    #[serde(deserialize_with = "exactly_true")]
    pub resolved: bool,
    #[serde(deserialize_with = "object")]
    pub position: Option<GitlabPosition>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GitlabDiscussion {
    #[serde(deserialize_with = "id")]
    pub id: String,
    /// True for a standalone comment. It says nothing about whether a reply can be
    /// sent - GitLab makes the thread out of the comment on the first reply - so
    /// nothing is read off it.
    #[serde(deserialize_with = "flag")]
    pub individual_note: bool,
    #[serde(deserialize_with = "list")]
    pub notes: Vec<GitlabNote>,
}

/// An ordinary Forgejo pull request comment (the issue comment endpoint).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ForgejoComment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(deserialize_with = "object")]
    pub user: Option<HostUser>,
    #[serde(deserialize_with = "text")]
    pub body: String,
    #[serde(deserialize_with = "text")]
    pub created_at: String,
}

/// A comment of a Forgejo review, left on a line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ForgejoReviewComment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(deserialize_with = "object")]
    pub user: Option<HostUser>,
    #[serde(deserialize_with = "text")]
    pub body: String,
    #[serde(deserialize_with = "text")]
    pub created_at: String,
    #[serde(deserialize_with = "optional_text")]
    pub path: Option<String>,
    /// Line on the new side, or 0 when the comment sits on the old one.
    #[serde(deserialize_with = "line")]
    pub position: Option<u32>,
    /// Line on the old side, or 0. Exactly one of the two is set.
    #[serde(deserialize_with = "line")]
    pub original_position: Option<u32>,
    /// How many lines after the position the comment also covers. The position is
    /// the first line of the range, the opposite of every other host - the comment
    /// is shown on the last.
    #[serde(deserialize_with = "count")]
    pub extra_lines_count: Option<i64>,
    /// Who resolved the conversation this comment belongs to, if anyone has.
    #[serde(deserialize_with = "object")]
    pub resolver: Option<HostUser>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketLink {
    #[serde(deserialize_with = "optional_text")]
    pub href: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketLinks {
    #[serde(deserialize_with = "object")]
    pub avatar: Option<BitbucketLink>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketUser {
    #[serde(deserialize_with = "optional_text")]
    pub display_name: Option<String>,
    #[serde(deserialize_with = "object")]
    pub links: Option<BitbucketLinks>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketContent {
    #[serde(deserialize_with = "optional_text")]
    pub raw: Option<String>,
}

/// Where a Bitbucket comment was left in the diff: `to` on the new file, `from` on
/// the old one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketInline {
    #[serde(deserialize_with = "optional_text")]
    pub path: Option<String>,
    #[serde(deserialize_with = "line")]
    pub to: Option<u32>,
    #[serde(deserialize_with = "line")]
    pub from: Option<u32>,
    /// Where a comment covering several lines starts; `to` and `from` are its end.
    #[serde(deserialize_with = "line")]
    pub start_to: Option<u32>,
    #[serde(deserialize_with = "line")]
    pub start_from: Option<u32>,
}

/// Names the comment a Bitbucket reply answers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketParent {
    #[serde(deserialize_with = "optional_id")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketResolution {
    #[serde(rename = "type", deserialize_with = "optional_text")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BitbucketComment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(deserialize_with = "object")]
    pub user: Option<BitbucketUser>,
    #[serde(deserialize_with = "object")]
    pub content: Option<BitbucketContent>,
    #[serde(deserialize_with = "text")]
    pub created_on: String,
    /// Read by truthiness.
    #[serde(deserialize_with = "flag")]
    pub deleted: bool,
    #[serde(deserialize_with = "object")]
    pub inline: Option<BitbucketInline>,
    /// Set on a reply, naming the comment it answers.
    #[serde(deserialize_with = "object")]
    pub parent: Option<BitbucketParent>,
    /// Set on the thread's opening comment once the thread has been resolved.
    #[serde(deserialize_with = "object")]
    pub resolution: Option<BitbucketResolution>,
}

/// The `author` of a GitHub GraphQL comment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GithubGqlAuthor {
    #[serde(deserialize_with = "optional_text")]
    pub login: Option<String>,
    #[serde(deserialize_with = "optional_text")]
    pub avatar_url: Option<String>,
}

/// A GitHub GraphQL comment: a review thread's, or an issue comment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GithubGqlComment {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(deserialize_with = "text")]
    pub body: String,
    #[serde(deserialize_with = "text")]
    pub created_at: String,
    #[serde(deserialize_with = "object")]
    pub author: Option<GithubGqlAuthor>,
}

/// A review thread's `comments` connection. `null` nodes - GraphQL's way of saying
/// a comment could not be read - are dropped here, which is the TypeScript's
/// `filter(Boolean)`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct GithubCommentNodes {
    #[serde(deserialize_with = "list")]
    pub nodes: Vec<GithubGqlComment>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GithubReviewThread {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(deserialize_with = "exactly_true")]
    pub is_resolved: bool,
    #[serde(deserialize_with = "exactly_true")]
    pub is_outdated: bool,
    #[serde(deserialize_with = "optional_text")]
    pub path: Option<String>,
    /// The line in the current diff; null once the thread has gone outdated.
    #[serde(deserialize_with = "line")]
    pub line: Option<u32>,
    #[serde(deserialize_with = "optional_text")]
    pub diff_side: Option<String>,
    /// Set when the thread covers several lines, ending on `line`.
    #[serde(deserialize_with = "line")]
    pub start_line: Option<u32>,
    #[serde(deserialize_with = "optional_text")]
    pub start_diff_side: Option<String>,
    #[serde(deserialize_with = "exactly_true")]
    pub viewer_can_reply: bool,
    #[serde(deserialize_with = "exactly_true")]
    pub viewer_can_resolve: bool,
    #[serde(deserialize_with = "exactly_true")]
    pub viewer_can_unresolve: bool,
    #[serde(deserialize_with = "object")]
    pub comments: Option<GithubCommentNodes>,
}

// --- Shared pieces -----------------------------------------------------------------

/// JavaScript truthiness of an optional line: `0` is as absent as `undefined`.
fn is_set(line: Option<u32>) -> bool {
    line.is_some_and(|line| line != 0)
}

/// JavaScript truthiness of an optional string.
fn non_empty(text: Option<&str>) -> bool {
    text.is_some_and(|text| !text.is_empty())
}

fn pull_comment(
    id: &str,
    name: Option<&str>,
    avatar: Option<&str>,
    body: &str,
    at: &str,
) -> PullComment {
    PullComment {
        id: id.to_owned(),
        author: User {
            name: name.unwrap_or("unknown").to_owned(),
            avatar_url: avatar.unwrap_or_default().to_owned(),
        },
        body: body.to_owned(),
        created_at: at.to_owned(),
    }
}

/// Oldest first by the opening comment. ISO-8601 timestamps order the same by code
/// point as by the TypeScript's `localeCompare`, and the sort is stable, as
/// `Array#sort` is.
fn sort_by_age(threads: &mut [CommentThread]) {
    threads.sort_by(|a, b| opened_at(a).cmp(opened_at(b)));
}

fn opened_at(thread: &CommentThread) -> &str {
    thread
        .comments
        .first()
        .map_or("", |comment| comment.created_at.as_str())
}

/// A host comment plus wherever in the diff it was left, before it becomes a thread.
struct Anchored {
    comment: PullComment,
    path: Option<String>,
    line: Option<u32>,
    start_line: Option<u32>,
    side: Option<Side>,
}

impl Anchored {
    fn general(comment: PullComment) -> Self {
        Anchored {
            comment,
            path: None,
            line: None,
            start_line: None,
            side: None,
        }
    }
}

/// Hosts that give comments no thread structure of their own: each one becomes a
/// thread by itself with both capabilities off, so the app never offers a reply or
/// a resolve it cannot carry out.
fn lone_threads(anchored: Vec<Anchored>) -> Vec<CommentThread> {
    anchored
        .into_iter()
        .map(|entry| CommentThread {
            id: entry.comment.id.clone(),
            comments: vec![entry.comment],
            resolved: false,
            outdated: false,
            path: entry.path,
            line: entry.line,
            start_line: entry.start_line,
            side: entry.side,
            can_reply: false,
            can_resolve: false,
        })
        .collect()
}

// --- GitHub -----------------------------------------------------------------------

/// Where a GitHub range starts, when it starts on the side it ends on. GitHub lets
/// a range begin on the other side of the diff; the app's range does not, so such
/// a thread keeps its last line and loses the reach back rather than being drawn
/// over lines it does not cover.
fn github_start(
    side: Option<&str>,
    start_side: Option<&str>,
    start_line: Option<u32>,
) -> Option<u32> {
    let start_line = start_line?;
    (!non_empty(start_side) || start_side == side).then_some(start_line)
}

/// `old` for GitHub's left side, `new` for anything else that has a file.
fn github_side(side: Option<&str>, path: Option<&str>) -> Option<Side> {
    if side == Some("LEFT") {
        Some(Side::Old)
    } else if non_empty(path) {
        Some(Side::New)
    } else {
        None
    }
}

fn github_anchored(comment: &GithubComment) -> Anchored {
    let user = comment.user.as_ref();
    Anchored {
        comment: pull_comment(
            &comment.id,
            user.and_then(|user| user.login.as_deref()),
            user.and_then(|user| user.avatar_url.as_deref()),
            &comment.body,
            &comment.created_at,
        ),
        path: comment.path.clone(),
        line: comment.line.or(comment.original_line),
        start_line: github_start(
            comment.side.as_deref(),
            comment.start_side.as_deref(),
            if is_set(comment.line) {
                comment.start_line
            } else {
                comment.original_start_line
            },
        ),
        side: github_side(comment.side.as_deref(), comment.path.as_deref()),
    }
}

/// The REST shape, kept as the fallback for an instance whose GraphQL endpoint we
/// cannot reach: every comment stands alone, because review threads simply are not
/// in the REST API. Issue comments and review comments interleave by age.
pub fn github_flat_threads(
    issue_comments: &[GithubComment],
    review_comments: &[GithubComment],
) -> Vec<CommentThread> {
    let mut anchored: Vec<Anchored> = issue_comments
        .iter()
        .chain(review_comments)
        .map(github_anchored)
        .collect();
    anchored.sort_by(|a, b| a.comment.created_at.cmp(&b.comment.created_at));
    lone_threads(anchored)
}

fn gql_comment(comment: &GithubGqlComment) -> PullComment {
    let author = comment.author.as_ref();
    pull_comment(
        &comment.id,
        author.and_then(|author| author.login.as_deref()),
        author.and_then(|author| author.avatar_url.as_deref()),
        &comment.body,
        &comment.created_at,
    )
}

/// The GraphQL shape, which is the only place GitHub keeps review threads and the
/// only way their resolution can be read or changed.
///
/// An outdated thread carries its file but no line. GitHub still reports the line it
/// was originally left on, and using that was the defect: in the diff as it stands
/// now that number is a different line, so the app pointed some comments at code
/// their author never saw. Better to say where the thread belongs and not pretend to
/// know where in it.
///
/// Whether a thread can be resolved depends on which way it would go, so the flag is
/// read from whichever of the two the viewer would actually be doing.
pub fn github_threads(
    review_threads: &[GithubReviewThread],
    issue_comments: &[GithubGqlComment],
) -> Vec<CommentThread> {
    let mut threads = Vec::with_capacity(review_threads.len() + issue_comments.len());

    for thread in review_threads {
        let comments: Vec<PullComment> = thread
            .comments
            .as_ref()
            .map(|connection| connection.nodes.iter().map(gql_comment).collect())
            .unwrap_or_default();
        if comments.is_empty() {
            continue;
        }

        let outdated = thread.is_outdated;
        let resolved = thread.is_resolved;

        threads.push(CommentThread {
            id: thread.id.clone(),
            comments,
            resolved,
            outdated,
            path: thread.path.clone(),
            line: if outdated { None } else { thread.line },
            start_line: if outdated {
                None
            } else {
                github_start(
                    thread.diff_side.as_deref(),
                    thread.start_diff_side.as_deref(),
                    thread.start_line,
                )
            },
            side: github_side(thread.diff_side.as_deref(), thread.path.as_deref()),
            can_reply: thread.viewer_can_reply,
            can_resolve: if resolved {
                thread.viewer_can_unresolve
            } else {
                thread.viewer_can_resolve
            },
        });
    }

    // An ordinary issue comment is not a review thread and nothing can be replied
    // into it, so it stays a thread of one with neither affordance.
    for comment in issue_comments {
        threads.push(CommentThread {
            id: comment.id.clone(),
            comments: vec![gql_comment(comment)],
            resolved: false,
            outdated: false,
            path: None,
            line: None,
            start_line: None,
            side: None,
            can_reply: false,
            can_resolve: false,
        });
    }

    sort_by_age(&mut threads);
    threads
}

// --- Forgejo ----------------------------------------------------------------------

/// The anchor a Forgejo thread id carries: one side of one line of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoAnchor {
    pub path: String,
    pub side: Side,
    pub line: u32,
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Old => "old",
        Side::New => "new",
    }
}

/// Forgejo has the weakest thread identity of the four: a review comment belongs to
/// a review, and a review is a batch of comments across the whole diff - so there is
/// no thread object to point at. What it does have is the rule its own UI works by:
/// a conversation is every comment left on one side of one line of one file.
///
/// So the identifier is synthesised from that anchor, which makes it stable across
/// refreshes without depending on any id the host might renumber, and readable back
/// when a reply has to be addressed. All of it stays in the adapter; the UI reads
/// capability flags like it does everywhere else.
pub fn forgejo_thread_id(path: &str, side: Side, line: u32) -> String {
    // Path last, and everything before it fixed-shape, so a path containing a colon
    // still parses back out.
    format!("fj:{}:{line}:{path}", side_name(side))
}

/// Reads back what [`forgejo_thread_id`] wrote, or `None` for anything else.
///
/// This is the TypeScript's `/^fj:(old|new):(\d+):(.+)$/` written out by hand, with
/// its JavaScript semantics kept: the digits are ASCII only, and the path must be
/// non-empty and free of line terminators (which `.` does not match). A line too
/// large for a `u32` is not an id this app wrote, so it reads as `None` too.
pub fn parse_forgejo_thread_id(id: &str) -> Option<ForgejoAnchor> {
    let rest = id.strip_prefix("fj:")?;
    let (side, rest) = if let Some(rest) = rest.strip_prefix("old:") {
        (Side::Old, rest)
    } else {
        (Side::New, rest.strip_prefix("new:")?)
    };

    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let path = rest[digits..].strip_prefix(':')?;
    let is_terminator = |c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}');
    if path.is_empty() || path.contains(is_terminator) {
        return None;
    }

    Some(ForgejoAnchor {
        path: path.to_owned(),
        side,
        line: rest[..digits].parse().ok()?,
    })
}

fn forgejo_comment(id: &str, user: Option<&HostUser>, body: &str, created_at: &str) -> PullComment {
    pull_comment(
        id,
        user.and_then(|user| user.login.as_deref()),
        user.and_then(|user| user.avatar_url.as_deref()),
        body,
        created_at,
    )
}

/// Ordinary comments stay threads of one - Forgejo has no notion of replying to a
/// pull request comment - while review comments group by the line they were left on.
///
/// A reply is another comment at the same anchor, which is how the conversation is
/// joined, so an inline thread can be replied to. Resolution is a different matter:
/// the REST API has no endpoint for it at all, so the state is read where the host
/// reports it and the control is never offered.
///
/// Pass an empty slice for `review_comments` where the TypeScript leaves the
/// argument out.
pub fn forgejo_threads(
    comments: &[ForgejoComment],
    review_comments: &[ForgejoReviewComment],
) -> Vec<CommentThread> {
    let mut threads = lone_threads(
        comments
            .iter()
            .map(|comment| {
                Anchored::general(forgejo_comment(
                    &comment.id,
                    comment.user.as_ref(),
                    &comment.body,
                    &comment.created_at,
                ))
            })
            .collect(),
    );

    // Conversations in the order they were first seen, as a JavaScript Map keeps them.
    let mut conversations: Vec<(String, Vec<&ForgejoReviewComment>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for comment in review_comments {
        let Some(path) = comment.path.as_deref() else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let side = if is_set(comment.position) {
            Side::New
        } else {
            Side::Old
        };
        let line = if is_set(comment.position) {
            comment.position
        } else {
            comment.original_position
        };
        let Some(line) = line.filter(|line| *line != 0) else {
            continue;
        };

        // Keyed by the position the host groups on - the first line of a range - which
        // is also where a reply has to be aimed. Where the thread is shown is decided
        // below.
        let id = forgejo_thread_id(path, side, line);
        match index.get(&id) {
            Some(&at) => conversations[at].1.push(comment),
            None => {
                index.insert(id.clone(), conversations.len());
                conversations.push((id, vec![comment]));
            }
        }
    }

    for (id, mut group) in conversations {
        let anchor = parse_forgejo_thread_id(&id);
        group.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        // The comment that opened the conversation says how far it reaches; the host
        // shows the whole thread on the last line it covers, and so does the app.
        let extra = group
            .first()
            .and_then(|comment| comment.extra_lines_count)
            .unwrap_or(0)
            .max(0);

        threads.push(CommentThread {
            comments: group
                .iter()
                .map(|comment| {
                    forgejo_comment(
                        &comment.id,
                        comment.user.as_ref(),
                        &comment.body,
                        &comment.created_at,
                    )
                })
                .collect(),
            // Forgejo resolves a whole conversation, so it counts only when all of it is.
            resolved: group.iter().all(|comment| comment.resolver.is_some()),
            outdated: false,
            path: anchor.as_ref().map(|anchor| anchor.path.clone()),
            line: anchor
                .as_ref()
                .and_then(|anchor| u32::try_from(i64::from(anchor.line) + extra).ok()),
            start_line: anchor
                .as_ref()
                .filter(|_| extra != 0)
                .map(|anchor| anchor.line),
            side: anchor.as_ref().map(|anchor| anchor.side),
            can_reply: true,
            can_resolve: false,
            id,
        });
    }

    sort_by_age(&mut threads);
    threads
}

// --- Bitbucket --------------------------------------------------------------------

fn bitbucket_comment(comment: &BitbucketComment) -> PullComment {
    let user = comment.user.as_ref();
    pull_comment(
        &comment.id,
        user.and_then(|user| user.display_name.as_deref()),
        user.and_then(|user| user.links.as_ref())
            .and_then(|links| links.avatar.as_ref())
            .and_then(|avatar| avatar.href.as_deref()),
        comment
            .content
            .as_ref()
            .and_then(|content| content.raw.as_deref())
            .unwrap_or_default(),
        &comment.created_on,
    )
}

/// Bitbucket says a reply by naming the comment it answers, so a thread is a chain
/// walked from whichever comment has no parent. Replies can nest, and the app's
/// thread is one ordered list, so the tree is flattened depth first with each set of
/// children in the order they were written.
///
/// A reply whose parent is not here - deleted, or past the page we fetched - opens a
/// thread of its own rather than disappearing with it.
///
/// Resolution is real on this host: `POST .../comments/{id}/resolve` resolves a
/// thread and `DELETE` reopens one, and a resolved thread carries a resolution on
/// the comment that opens it. It is offered on inline threads only, which is where
/// Bitbucket itself offers it.
pub fn bitbucket_threads(comments: &[BitbucketComment]) -> Vec<CommentThread> {
    let live: Vec<&BitbucketComment> = comments.iter().filter(|comment| !comment.deleted).collect();
    let known: HashSet<&str> = live.iter().map(|comment| comment.id.as_str()).collect();

    let mut children: HashMap<&str, Vec<&BitbucketComment>> = HashMap::new();
    let mut roots: Vec<&BitbucketComment> = Vec::new();

    for &comment in &live {
        let parent = comment
            .parent
            .as_ref()
            .and_then(|parent| parent.id.as_deref());
        match parent {
            Some(parent) if known.contains(parent) && parent != comment.id => {
                children.entry(parent).or_default().push(comment);
            }
            _ => roots.push(comment),
        }
    }

    let by_age = |a: &&BitbucketComment, b: &&BitbucketComment| a.created_on.cmp(&b.created_on);
    roots.sort_by(by_age);
    for siblings in children.values_mut() {
        siblings.sort_by(by_age);
    }

    let mut seen: HashSet<&str> = HashSet::new();
    roots
        .iter()
        .map(|&root| {
            let inline = root.inline.as_ref();
            let to_is_set = inline.is_some_and(|inline| is_set(inline.to));
            let line = inline.and_then(|inline| inline.to.or(inline.from));
            let start = inline.and_then(|inline| {
                if to_is_set {
                    inline.start_to
                } else {
                    inline.start_from
                }
            });

            CommentThread {
                id: root.id.clone(),
                comments: bitbucket_chain(root, &children, &mut seen),
                resolved: root.resolution.is_some(),
                outdated: false,
                path: inline.and_then(|inline| inline.path.clone()),
                line,
                start_line: start,
                side: inline.map(|_| if to_is_set { Side::New } else { Side::Old }),
                can_reply: true,
                can_resolve: inline.is_some(),
            }
        })
        .collect()
}

/// Depth first, so a reply sits under what it answers rather than after it.
///
/// Walked with an explicit stack rather than by recursion, so a long chain of
/// replies to replies cannot exhaust the stack; the order is the same. `seen` is
/// shared across every thread, so no comment is ever shown twice.
fn bitbucket_chain<'c>(
    root: &'c BitbucketComment,
    children: &HashMap<&'c str, Vec<&'c BitbucketComment>>,
    seen: &mut HashSet<&'c str>,
) -> Vec<PullComment> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(comment) = stack.pop() {
        if !seen.insert(comment.id.as_str()) {
            continue;
        }
        out.push(bitbucket_comment(comment));
        if let Some(replies) = children.get(comment.id.as_str()) {
            stack.extend(replies.iter().rev());
        }
    }
    out
}

// --- GitLab -----------------------------------------------------------------------

/// GitLab is the host with real threads: a discussion carries its own notes, and
/// both replying and resolving are ordinary REST calls.
///
/// Every discussion can be replied into, the comment left on the merge request
/// itself included. Answering a standalone comment is how GitLab makes a thread out
/// of one - "this can also create a thread from a single comment" - so the browser
/// offers the reply and refusing it here was the app inventing a limit the host does
/// not have. GitLab publishes no per-discussion reply permission, so a merge request
/// that will not take the note says so when the reply is sent.
///
/// Resolution is a different matter: only some notes are resolvable at all, so that
/// capability is still read off the discussion.
///
/// `head_sha` is the merge request's current head, to spot a thread left against a
/// version of the diff that has since moved on.
pub fn gitlab_threads(
    discussions: &[GitlabDiscussion],
    head_sha: Option<&str>,
) -> Vec<CommentThread> {
    let mut threads = Vec::new();

    for (id, notes) in fold_discussion_pages(discussions) {
        // System notes are the "changed the description" chatter, not conversation.
        let notes: Vec<&GitlabNote> = notes.into_iter().filter(|note| !note.system).collect();
        if notes.is_empty() {
            continue;
        }

        let resolvable: Vec<&GitlabNote> = notes
            .iter()
            .copied()
            .filter(|note| note.resolvable)
            .collect();
        let position = notes.iter().find_map(|note| note.position.as_ref());
        let line = position.and_then(|position| position.new_line.or(position.old_line));
        // The start on the same side as the note itself; a range the browser drew from
        // the other side of the diff keeps its last line and nothing more.
        let start = position.and_then(|position| {
            let start = position.line_range.as_ref()?.start.as_ref()?;
            if is_set(position.new_line) {
                start.new_line
            } else {
                start.old_line
            }
        });
        let position_head = position.and_then(|position| position.head_sha.as_deref());

        threads.push(CommentThread {
            id: id.to_owned(),
            comments: notes
                .iter()
                .map(|note| {
                    let author = note.author.as_ref();
                    pull_comment(
                        &note.id,
                        author.and_then(|author| author.username.as_deref()),
                        author.and_then(|author| author.avatar_url.as_deref()),
                        &note.body,
                        &note.created_at,
                    )
                })
                .collect(),
            resolved: !resolvable.is_empty() && resolvable.iter().all(|note| note.resolved),
            outdated: non_empty(head_sha) && non_empty(position_head) && position_head != head_sha,
            path: position.and_then(|position| {
                position
                    .new_path
                    .clone()
                    .or_else(|| position.old_path.clone())
            }),
            line,
            start_line: start,
            side: position.and_then(|position| {
                if is_set(position.new_line) {
                    Some(Side::New)
                } else if is_set(position.old_line) {
                    Some(Side::Old)
                } else {
                    None
                }
            }),
            can_reply: true,
            can_resolve: !resolvable.is_empty(),
        });
    }

    threads
}

/// Discussions arrive a page at a time, and a long one can come back on more than
/// one page - the same id twice, each copy carrying part of the conversation.
///
/// Left alone that reads as a thread whose later replies simply are not there,
/// which is indistinguishable from nobody having written them. So the pages are
/// folded back into one discussion first, keeping the order they arrived in - which
/// is the order they were written - and a note both pages carried is kept once.
fn fold_discussion_pages(discussions: &[GitlabDiscussion]) -> Vec<(&str, Vec<&GitlabNote>)> {
    let mut folded: Vec<(&str, Vec<&GitlabNote>)> = Vec::new();
    let mut index: HashMap<&str, usize> = HashMap::new();

    for discussion in discussions {
        let Some(&at) = index.get(discussion.id.as_str()) else {
            index.insert(&discussion.id, folded.len());
            folded.push((&discussion.id, discussion.notes.iter().collect()));
            continue;
        };

        let existing = &mut folded[at].1;
        let mut seen: HashSet<&str> = existing.iter().map(|note| note.id.as_str()).collect();
        for note in &discussion.notes {
            if seen.insert(&note.id) {
                existing.push(note);
            }
        }
    }

    folded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Reads a host shape from the JSON a test writes, the way the adapter will.
    fn de<T: DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).expect("a test fixture deserialises")
    }

    fn bodies(threads: &[CommentThread]) -> Vec<&str> {
        threads
            .iter()
            .map(|thread| thread.comments[0].body.as_str())
            .collect()
    }

    fn ids(threads: &[CommentThread]) -> Vec<&str> {
        threads.iter().map(|thread| thread.id.as_str()).collect()
    }

    fn comment_bodies(thread: &CommentThread) -> Vec<&str> {
        thread
            .comments
            .iter()
            .map(|comment| comment.body.as_str())
            .collect()
    }

    fn comment_ids(thread: &CommentThread) -> Vec<&str> {
        thread
            .comments
            .iter()
            .map(|comment| comment.id.as_str())
            .collect()
    }

    #[test]
    fn github_flat_threads_gives_every_comment_its_own_thread_with_no_affordances() {
        let threads = github_flat_threads(
            &de::<Vec<GithubComment>>(json!([{
                "id": 1,
                "user": { "login": "mnovotna", "avatar_url": "https://avatars.test/m.png" },
                "body": "Looks good overall.",
                "created_at": "2026-08-01T10:00:00Z",
            }])),
            &de::<Vec<GithubComment>>(json!([{
                "id": 2,
                "user": { "login": "hkramer", "avatar_url": "" },
                "body": "This branch reads redundant.",
                "created_at": "2026-08-01T09:00:00Z",
                "path": "internal/capture.go",
                "line": 55,
                "side": "RIGHT",
            }])),
        );

        // Issue comments and review comments interleave by age, not by which call they came from.
        assert_eq!(
            bodies(&threads),
            ["This branch reads redundant.", "Looks good overall."]
        );
        for thread in &threads {
            assert_eq!(thread.comments.len(), 1);
            assert!(!thread.can_reply, "REST cannot reply to a GitHub thread");
            assert!(!thread.can_resolve, "resolution is a GraphQL mutation only");
            assert!(!thread.resolved);
        }

        let inline = &threads[0];
        assert_eq!(inline.path.as_deref(), Some("internal/capture.go"));
        assert_eq!(inline.line, Some(55));
        assert_eq!(inline.side, Some(Side::New));
        assert_eq!(inline.comments[0].author.name, "hkramer");

        assert_eq!(threads[1].path, None);
        assert_eq!(threads[1].line, None);
    }

    #[test]
    fn github_flat_threads_falls_back_to_the_original_line_and_reads_the_left_side() {
        let threads = github_flat_threads(
            &[],
            &[de(json!({
                "id": 3,
                "body": "Was this needed?",
                "created_at": "2026-08-01T09:00:00Z",
                "path": "a.ts",
                "line": null,
                "original_line": 12,
                "side": "LEFT",
            }))],
        );
        let thread = &threads[0];

        assert_eq!(thread.line, Some(12));
        assert_eq!(thread.side, Some(Side::Old));
        assert_eq!(thread.comments[0].author.name, "unknown");
    }

    #[test]
    fn forgejo_threads_keeps_an_ordinary_comment_a_thread_of_one_with_no_affordances() {
        let threads = forgejo_threads(
            &[de(json!({
                "id": 7,
                "user": { "login": "vmares", "avatar_url": "https://codeberg.test/v.png" },
                "body": "Ready from my side.",
                "created_at": "2026-08-02T08:00:00Z",
            }))],
            &[],
        );

        assert_eq!(threads.len(), 1);
        assert_eq!(
            threads[0].comments,
            [PullComment {
                id: "7".into(),
                author: User {
                    name: "vmares".into(),
                    avatar_url: "https://codeberg.test/v.png".into(),
                },
                body: "Ready from my side.".into(),
                created_at: "2026-08-02T08:00:00Z".into(),
            }]
        );
        // Forgejo has no notion of replying to a pull request comment.
        assert!(!threads[0].can_reply);
        assert!(!threads[0].can_resolve);
        assert_eq!(threads[0].path, None);
    }

    #[test]
    fn forgejo_thread_id_survives_a_round_trip_including_a_path_with_a_colon_in_it() {
        for (path, side, line) in [
            ("src/app.ts", Side::New, 42),
            ("src/a:b.ts", Side::Old, 7),
            ("deep/nested/path/with:colons/x.go", Side::New, 1),
        ] {
            assert_eq!(
                parse_forgejo_thread_id(&forgejo_thread_id(path, side, line)),
                Some(ForgejoAnchor {
                    path: path.into(),
                    side,
                    line
                })
            );
        }

        assert_eq!(parse_forgejo_thread_id("not-a-thread-id"), None);
        assert_eq!(parse_forgejo_thread_id("fj:sideways:1:a.ts"), None);
        assert_eq!(parse_forgejo_thread_id("fj:new:notanumber:a.ts"), None);
        assert_eq!(parse_forgejo_thread_id("fj:new:1:"), None);
    }

    #[test]
    fn forgejo_threads_groups_review_comments_by_the_line_they_were_left_on() {
        let threads = forgejo_threads(
            &[],
            &de::<Vec<ForgejoReviewComment>>(json!([
                {
                    "id": 20,
                    "user": { "login": "mnovotna" },
                    "body": "Can this go into config?",
                    "created_at": "2026-08-02T09:00:00Z",
                    "path": "internal/capture.go",
                    "position": 55,
                    "original_position": 0,
                },
                {
                    "id": 21,
                    "user": { "login": "hkramer" },
                    "body": "Follow-up.",
                    "created_at": "2026-08-02T10:00:00Z",
                    "path": "internal/capture.go",
                    "position": 55,
                    "original_position": 0,
                },
                {
                    "id": 22,
                    "user": { "login": "mnovotna" },
                    "body": "Different line entirely.",
                    "created_at": "2026-08-02T11:00:00Z",
                    "path": "internal/capture.go",
                    "position": 80,
                    "original_position": 0,
                },
            ])),
        );

        assert_eq!(threads.len(), 2, "two lines, two conversations");

        let (first, second) = (&threads[0], &threads[1]);
        assert_eq!(
            comment_bodies(first),
            ["Can this go into config?", "Follow-up."]
        );
        assert_eq!(first.line, Some(55));
        assert_eq!(first.side, Some(Side::New));
        assert_eq!(first.path.as_deref(), Some("internal/capture.go"));
        assert_eq!(second.line, Some(80));

        // Two comments left in different reviews still share one conversation, so the
        // identifier cannot come from a review id.
        assert_eq!(
            first.id,
            forgejo_thread_id("internal/capture.go", Side::New, 55)
        );
    }

    #[test]
    fn forgejo_threads_reads_the_old_side_from_the_original_position() {
        let threads = forgejo_threads(
            &[],
            &[de(json!({
                "id": 30,
                "body": "Why was this dropped?",
                "created_at": "2026-08-02T09:00:00Z",
                "path": "a.ts",
                "position": 0,
                "original_position": 12,
            }))],
        );
        let thread = &threads[0];

        assert_eq!(thread.side, Some(Side::Old));
        assert_eq!(thread.line, Some(12));
        assert_eq!(thread.comments[0].author.name, "unknown");
    }

    #[test]
    fn forgejo_threads_offers_a_reply_on_an_inline_thread_and_never_a_resolve() {
        let threads = forgejo_threads(
            &[],
            &[de(json!({
                "id": 40, "body": "A remark.", "created_at": "2026-08-02T09:00:00Z",
                "path": "a.ts", "position": 3,
            }))],
        );
        let thread = &threads[0];

        // A reply is another comment at the same anchor, which the API does support.
        assert!(thread.can_reply);
        // Resolution has no REST endpoint at all, so the control must never appear.
        assert!(!thread.can_resolve);
    }

    #[test]
    fn forgejo_threads_counts_a_conversation_resolved_only_once_every_comment_in_it_is() {
        let at = |id: u32, resolver: Option<&str>| -> ForgejoReviewComment {
            de(json!({
                "id": id,
                "body": format!("c{id}"),
                "created_at": "2026-08-02T09:00:00Z",
                "path": "a.ts",
                "position": 5,
                "resolver": resolver.map(|login| json!({ "login": login })),
            }))
        };

        let partly = &forgejo_threads(&[], &[at(1, Some("vmares")), at(2, None)])[0];
        assert!(!partly.resolved);

        let fully = &forgejo_threads(&[], &[at(3, Some("vmares")), at(4, Some("vmares"))])[0];
        assert!(fully.resolved);

        let none = &forgejo_threads(&[], &[at(5, None)])[0];
        assert!(!none.resolved);
    }

    #[test]
    fn forgejo_threads_drops_a_review_comment_it_cannot_anchor() {
        let threads = forgejo_threads(
            &[],
            &de::<Vec<ForgejoReviewComment>>(json!([
                { "id": 50, "body": "no path", "created_at": "2026-08-02T09:00:00Z", "position": 3 },
                { "id": 51, "body": "no line", "created_at": "2026-08-02T09:00:00Z", "path": "a.ts", "position": 0 },
            ])),
        );

        assert!(threads.is_empty(), "{threads:?}");
    }

    #[test]
    fn forgejo_threads_orders_ordinary_comments_and_conversations_together_by_age() {
        let threads = forgejo_threads(
            &[de(
                json!({ "id": 60, "body": "second", "created_at": "2026-08-02T10:00:00Z" }),
            )],
            &[de(json!({
                "id": 61, "body": "first", "created_at": "2026-08-02T09:00:00Z",
                "path": "a.ts", "position": 1,
            }))],
        );

        assert_eq!(bodies(&threads), ["first", "second"]);
    }

    #[test]
    fn bitbucket_threads_keeps_the_inline_anchor_and_drops_deleted_comments() {
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(json!([
            {
                "id": 11,
                "user": { "display_name": "L Peters", "links": { "avatar": { "href": "https://bb.test/l.png" } } },
                "content": { "raw": "Nit: naming." },
                "created_on": "2026-08-03T08:00:00Z",
                "inline": { "path": "src/app.ts", "to": 42 },
            },
            {
                "id": 12,
                "content": { "raw": "gone" },
                "created_on": "2026-08-03T09:00:00Z",
                "deleted": true,
            },
            {
                "id": 13,
                "content": { "raw": "On the old side." },
                "created_on": "2026-08-03T10:00:00Z",
                "inline": { "path": "src/app.ts", "from": 40 },
            },
        ])));

        assert_eq!(ids(&threads), ["11", "13"]);
        assert_eq!(threads[0].line, Some(42));
        assert_eq!(threads[0].side, Some(Side::New));
        assert_eq!(
            threads[0].comments[0].author.avatar_url,
            "https://bb.test/l.png"
        );
        assert_eq!(threads[1].line, Some(40));
        assert_eq!(threads[1].side, Some(Side::Old));
        assert_eq!(threads[1].comments[0].author.name, "unknown");
    }

    #[test]
    fn bitbucket_threads_walks_parent_references_into_one_ordered_thread() {
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(json!([
            {
                "id": 1,
                "content": { "raw": "opening" },
                "created_on": "2026-08-03T08:00:00Z",
                "inline": { "path": "a.ts", "to": 5 },
            },
            { "id": 3, "content": { "raw": "second reply" }, "created_on": "2026-08-03T10:00:00Z", "parent": { "id": 1 } },
            { "id": 2, "content": { "raw": "first reply" }, "created_on": "2026-08-03T09:00:00Z", "parent": { "id": 1 } },
            { "id": 4, "content": { "raw": "reply to a reply" }, "created_on": "2026-08-03T11:00:00Z", "parent": { "id": 2 } },
        ])));

        assert_eq!(threads.len(), 1, "one root, one thread");
        // Depth first, so a reply sits under what it answers rather than after it.
        assert_eq!(
            comment_bodies(&threads[0]),
            ["opening", "first reply", "reply to a reply", "second reply"]
        );
        assert_eq!(
            threads[0].id, "1",
            "the thread is addressed by the comment that opened it"
        );
        assert_eq!(threads[0].line, Some(5));
    }

    #[test]
    fn bitbucket_threads_opens_a_thread_for_a_reply_whose_parent_is_not_here() {
        // Past the page we fetched, or deleted: the reply must not vanish with it.
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(json!([
            { "id": 9, "content": { "raw": "orphaned" }, "created_on": "2026-08-03T09:00:00Z", "parent": { "id": 404 } },
            { "id": 10, "content": { "raw": "child of a deleted parent" }, "created_on": "2026-08-03T10:00:00Z", "parent": { "id": 11 } },
            { "id": 11, "content": { "raw": "gone" }, "created_on": "2026-08-03T08:00:00Z", "deleted": true },
        ])));

        assert_eq!(bodies(&threads), ["orphaned", "child of a deleted parent"]);
    }

    #[test]
    fn bitbucket_threads_reads_resolution_off_the_comment_that_opened_the_thread() {
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(json!([
            {
                "id": 1,
                "content": { "raw": "still open" },
                "created_on": "2026-08-03T08:00:00Z",
                "inline": { "path": "a.ts", "to": 1 },
            },
            {
                "id": 2,
                "content": { "raw": "dealt with" },
                "created_on": "2026-08-03T09:00:00Z",
                "inline": { "path": "a.ts", "to": 2 },
                "resolution": { "type": "pullrequest_comment_resolution" },
            },
        ])));
        let (open, resolved) = (&threads[0], &threads[1]);

        assert!(!open.resolved);
        assert!(resolved.resolved);
    }

    #[test]
    fn bitbucket_threads_offers_a_reply_everywhere_and_a_resolve_only_inline() {
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(json!([
            { "id": 1, "content": { "raw": "on the pull request" }, "created_on": "2026-08-03T08:00:00Z" },
            {
                "id": 2,
                "content": { "raw": "on a line" },
                "created_on": "2026-08-03T09:00:00Z",
                "inline": { "path": "a.ts", "to": 3 },
            },
        ])));
        let (general, inline) = (&threads[0], &threads[1]);

        assert!(general.can_reply);
        // Bitbucket resolves inline threads only, so the control must not appear elsewhere.
        assert!(!general.can_resolve);
        assert!(inline.can_reply);
        assert!(inline.can_resolve);
    }

    #[test]
    fn bitbucket_threads_survives_a_comment_that_names_itself_as_its_own_parent() {
        let threads = bitbucket_threads(&[de(json!({
            "id": 1, "content": { "raw": "self-parented" },
            "created_on": "2026-08-03T08:00:00Z", "parent": { "id": 1 },
        }))]);

        assert_eq!(
            threads.iter().map(comment_bodies).collect::<Vec<_>>(),
            [["self-parented"]]
        );
    }

    #[test]
    fn gitlab_threads_keeps_a_discussion_together_and_reads_both_capabilities_off_it() {
        let threads = gitlab_threads(
            &[de(json!({
                "id": "abc123",
                "individual_note": false,
                "notes": [
                    {
                        "id": 1,
                        "body": "Can this go into config?",
                        "author": { "username": "mnovotna", "avatar_url": "https://gl.test/m.png" },
                        "created_at": "2026-08-04T08:00:00Z",
                        "resolvable": true,
                        "resolved": false,
                        "position": {
                            "new_path": "internal/capture.go",
                            "old_path": "internal/capture.go",
                            "new_line": 55,
                            "head_sha": "head1",
                        },
                    },
                    {
                        "id": 2,
                        "body": "Follow-up, so this can ship today.",
                        "author": { "username": "hkramer" },
                        "created_at": "2026-08-04T09:00:00Z",
                        "resolvable": true,
                        "resolved": false,
                    },
                ],
            }))],
            Some("head1"),
        );
        let thread = &threads[0];

        assert_eq!(thread.id, "abc123");
        assert_eq!(comment_ids(thread), ["1", "2"]);
        assert!(thread.can_reply);
        assert!(thread.can_resolve);
        assert!(!thread.resolved);
        assert!(!thread.outdated);
        assert_eq!(thread.path.as_deref(), Some("internal/capture.go"));
        assert_eq!(thread.line, Some(55));
        assert_eq!(thread.side, Some(Side::New));
    }

    #[test]
    fn gitlab_threads_counts_a_discussion_resolved_only_once_every_resolvable_note_is() {
        let discussion = |id: &str, resolved: &[bool]| -> GitlabDiscussion {
            let notes: Vec<Value> = resolved
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    json!({
                        "id": index + 1,
                        "body": format!("note {index}"),
                        "created_at": "2026-08-04T08:00:00Z",
                        "resolvable": true,
                        "resolved": value,
                    })
                })
                .collect();
            de(json!({ "id": id, "notes": notes }))
        };

        let partly = &gitlab_threads(&[discussion("d1", &[true, false])], None)[0];
        assert!(!partly.resolved);

        let fully = &gitlab_threads(&[discussion("d2", &[true, true])], None)[0];
        assert!(fully.resolved);
    }

    #[test]
    fn gitlab_threads_takes_a_reply_into_a_standalone_comment_and_offers_no_resolution_where_nothing_resolves()
     {
        let threads = gitlab_threads(
            &[de(json!({
                "id": "d3",
                "individual_note": true,
                "notes": [{
                    "id": 9,
                    "body": "Just a comment.",
                    "author": { "username": "vmares" },
                    "created_at": "2026-08-04T08:00:00Z",
                    "resolvable": false,
                }],
            }))],
            None,
        );
        let thread = &threads[0];

        // GitLab makes the thread out of the comment on the first reply, so a comment
        // left on the merge request itself is as answerable as one left on a line.
        assert!(thread.can_reply);
        assert!(!thread.can_resolve);
        assert!(
            !thread.resolved,
            "nothing resolvable must not read as resolved"
        );
    }

    #[test]
    fn gitlab_threads_folds_a_discussion_handed_back_on_more_than_one_page_into_one_thread() {
        let note = |id: u32, body: &str, hour: u32| json!({ "id": id, "body": body, "created_at": format!("2026-08-04T0{hour}:00:00Z") });

        let threads = gitlab_threads(
            &de::<Vec<GitlabDiscussion>>(json!([
                {
                    "id": "d7",
                    "individual_note": false,
                    "notes": [note(1, "Opening remark.", 8), note(2, "First reply.", 9)],
                },
                { "id": "d8", "notes": [note(5, "Somewhere else entirely.", 8)] },
                // The next page repeats the discussion and carries the rest of it,
                // including the note the page boundary fell on.
                {
                    "id": "d7",
                    "individual_note": false,
                    "notes": [note(2, "First reply.", 9), note(3, "Second reply.", 10)],
                },
            ])),
            None,
        );

        assert_eq!(ids(&threads), ["d7", "d8"]);
        assert_eq!(
            comment_bodies(&threads[0]),
            ["Opening remark.", "First reply.", "Second reply."]
        );
    }

    #[test]
    fn gitlab_threads_drops_system_notes_and_the_discussions_made_only_of_them() {
        let threads = gitlab_threads(
            &de::<Vec<GitlabDiscussion>>(json!([
                {
                    "id": "d4",
                    "notes": [
                        { "id": 1, "body": "changed the description", "created_at": "2026-08-04T08:00:00Z", "system": true },
                    ],
                },
                {
                    "id": "d5",
                    "notes": [
                        { "id": 2, "body": "assigned to @vmares", "created_at": "2026-08-04T08:00:00Z", "system": true },
                        { "id": 3, "body": "A real remark.", "created_at": "2026-08-04T09:00:00Z" },
                    ],
                },
            ])),
            None,
        );

        assert_eq!(ids(&threads), ["d5"]);
        assert_eq!(comment_bodies(&threads[0]), ["A real remark."]);
    }

    #[test]
    fn gitlab_threads_flags_a_thread_left_against_a_diff_that_has_moved_on() {
        let discussion: GitlabDiscussion = de(json!({
            "id": "d6",
            "notes": [{
                "id": 1,
                "body": "On an older version.",
                "created_at": "2026-08-04T08:00:00Z",
                "position": { "new_path": "a.ts", "new_line": 3, "head_sha": "old-head" },
            }],
        }));
        let discussions = [discussion];

        assert!(gitlab_threads(&discussions, Some("new-head"))[0].outdated);
        assert!(!gitlab_threads(&discussions, Some("old-head"))[0].outdated);
        // Without a head to compare against, saying it is outdated would be a guess.
        assert!(!gitlab_threads(&discussions, None)[0].outdated);
    }

    const NOW: &str = "2026-08-05T08:00:00Z";

    fn gql_comment_json(id: &str, body: &str, at: &str) -> Value {
        json!({
            "id": id,
            "body": body,
            "createdAt": at,
            "author": { "login": "mnovotna", "avatarUrl": "https://gh.test/m.png" },
        })
    }

    fn gql(id: &str, body: &str) -> Value {
        gql_comment_json(id, body, NOW)
    }

    /// The TypeScript's `{ ...base, ...overrides }` over JSON objects.
    fn spread(base: &Value, overrides: Value) -> Value {
        let mut merged = base.clone();
        if let (Value::Object(merged), Value::Object(overrides)) = (&mut merged, overrides) {
            merged.extend(overrides);
        }
        merged
    }

    #[test]
    fn github_threads_keeps_a_review_thread_whole_and_reads_both_capabilities_off_it() {
        let threads = github_threads(
            &[de(json!({
                "id": "RT_1",
                "isResolved": false,
                "isOutdated": false,
                "path": "internal/capture.go",
                "line": 55,
                "diffSide": "RIGHT",
                "viewerCanReply": true,
                "viewerCanResolve": true,
                "viewerCanUnresolve": false,
                "comments": {
                    "nodes": [gql("C_1", "Can this go into config?"), gql("C_2", "Follow-up.")],
                },
            }))],
            &[],
        );
        let thread = &threads[0];

        assert_eq!(thread.id, "RT_1");
        assert_eq!(comment_ids(thread), ["C_1", "C_2"]);
        assert!(thread.can_reply);
        assert!(thread.can_resolve);
        assert!(!thread.resolved);
        assert!(!thread.outdated);
        assert_eq!(thread.path.as_deref(), Some("internal/capture.go"));
        assert_eq!(thread.line, Some(55));
        assert_eq!(thread.side, Some(Side::New));
        assert_eq!(
            thread.comments[0].author.avatar_url,
            "https://gh.test/m.png"
        );
    }

    #[test]
    fn github_threads_reads_resolvability_from_whichever_way_the_thread_would_go() {
        let base = json!({
            "id": "RT_2",
            "path": "a.ts",
            "line": 3,
            "comments": { "nodes": [gql("C_3", "Dealt with?")] },
        });
        let one = |overrides: Value| {
            github_threads(&[de(spread(&base, overrides))], &[])
                .into_iter()
                .next()
                .expect("one thread")
        };

        // An unresolved thread is resolvable when the viewer may resolve it...
        let open = one(
            json!({ "isResolved": false, "viewerCanResolve": true, "viewerCanUnresolve": false }),
        );
        assert!(open.can_resolve);

        // ...and a resolved one when the viewer may reopen it, which is the other flag.
        let closed = one(
            json!({ "isResolved": true, "viewerCanResolve": false, "viewerCanUnresolve": true }),
        );
        assert!(closed.resolved);
        assert!(closed.can_resolve);

        let locked = one(
            json!({ "isResolved": true, "viewerCanResolve": true, "viewerCanUnresolve": false }),
        );
        assert!(
            !locked.can_resolve,
            "a resolved thread is not reopenable just because it was resolvable"
        );
    }

    #[test]
    fn github_threads_never_anchors_an_outdated_thread_to_a_line() {
        // The defect this fixes: GitHub reports the line the thread was originally left
        // on, and in the diff as it stands that number is a different line entirely.
        let threads = github_threads(
            &[de(json!({
                "id": "RT_3",
                "isOutdated": true,
                "path": "internal/capture.go",
                "line": null,
                "diffSide": "RIGHT",
                "comments": { "nodes": [gql("C_4", "This moved.")] },
            }))],
            &[],
        );
        let thread = &threads[0];

        assert!(thread.outdated);
        assert_eq!(thread.line, None);
        // The file is kept, so the thread can still be shown against the file it belongs to.
        assert_eq!(thread.path.as_deref(), Some("internal/capture.go"));
    }

    #[test]
    fn github_threads_keeps_an_issue_comment_a_thread_of_one_with_no_affordances() {
        let threads = github_threads(&[], &[de(gql("IC_1", "Looks good overall."))]);

        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comments.len(), 1);
        assert!(
            !threads[0].can_reply,
            "an issue comment is not a review thread"
        );
        assert!(!threads[0].can_resolve);
        assert_eq!(threads[0].path, None);
    }

    #[test]
    fn github_threads_orders_review_threads_and_issue_comments_together_by_age() {
        let threads = github_threads(
            &[de(json!({
                "id": "RT_4",
                "path": "a.ts",
                "line": 1,
                "comments": { "nodes": [gql_comment_json("C_5", "second", "2026-08-05T09:00:00Z")] },
            }))],
            &[de(gql_comment_json(
                "IC_2",
                "first",
                "2026-08-05T08:00:00Z",
            ))],
        );

        assert_eq!(bodies(&threads), ["first", "second"]);
    }

    #[test]
    fn github_threads_drops_a_thread_whose_comments_have_all_gone() {
        let threads = github_threads(
            &de::<Vec<GithubReviewThread>>(json!([
                { "id": "RT_5", "path": "a.ts", "line": 1, "comments": { "nodes": [] } },
                { "id": "RT_6", "path": "a.ts", "line": 2, "comments": null },
                { "id": "RT_7", "path": "a.ts", "line": 3, "comments": { "nodes": [null, gql("C_6", "here")] } },
            ])),
            &[],
        );

        assert_eq!(ids(&threads), ["RT_7"]);
        assert_eq!(threads[0].comments.len(), 1);
    }

    // --- ranges ---

    #[test]
    fn github_flat_threads_reads_where_a_range_starts_and_only_on_the_side_it_ends_on() {
        let threads = github_flat_threads(
            &[],
            &de::<Vec<GithubComment>>(json!([
                {
                    "id": 40,
                    "body": "These three.",
                    "created_at": "2026-08-01T09:00:00Z",
                    "path": "a.ts",
                    "line": 12,
                    "side": "RIGHT",
                    "start_line": 10,
                    "start_side": "RIGHT",
                },
                {
                    "id": 41,
                    "body": "From the old side over to the new.",
                    "created_at": "2026-08-01T09:01:00Z",
                    "path": "a.ts",
                    "line": 12,
                    "side": "RIGHT",
                    "start_line": 10,
                    "start_side": "LEFT",
                },
                {
                    "id": 42,
                    "body": "Written against an older diff.",
                    "created_at": "2026-08-01T09:02:00Z",
                    "path": "a.ts",
                    "line": null,
                    "original_line": 30,
                    "side": "LEFT",
                    "start_line": null,
                    "original_start_line": 28,
                    "start_side": "LEFT",
                },
            ])),
        );
        let (same, crossed, outdated) = (&threads[0], &threads[1], &threads[2]);

        assert_eq!(same.line, Some(12));
        assert_eq!(same.start_line, Some(10));
        // A range the app cannot draw keeps its last line and nothing more.
        assert_eq!(crossed.line, Some(12));
        assert_eq!(crossed.start_line, None);
        assert_eq!(outdated.line, Some(30));
        assert_eq!(outdated.start_line, Some(28));
    }

    #[test]
    fn github_threads_reads_where_a_review_thread_starts_and_forgets_it_once_outdated() {
        let base = json!({
            "path": "a.ts",
            "line": 12,
            "diffSide": "RIGHT",
            "startLine": 10,
            "startDiffSide": "RIGHT",
            "viewerCanReply": true,
            "comments": { "nodes": [gql("C_9", "These three.")] },
        });
        let threads = github_threads(
            &[
                de(spread(&base, json!({ "id": "RT_9" }))),
                de(spread(
                    &base,
                    json!({ "id": "RT_10", "startDiffSide": "LEFT" }),
                )),
                de(spread(&base, json!({ "id": "RT_11", "isOutdated": true }))),
            ],
            &[],
        );
        let (live, crossed, outdated) = (&threads[0], &threads[1], &threads[2]);

        assert_eq!(live.start_line, Some(10));
        assert_eq!(crossed.start_line, None);
        assert_eq!(outdated.line, None);
        assert_eq!(outdated.start_line, None);
    }

    #[test]
    fn forgejo_threads_shows_a_range_on_its_last_line_and_keys_it_by_its_first() {
        let threads = forgejo_threads(
            &[],
            &de::<Vec<ForgejoReviewComment>>(json!([
                {
                    "id": 50,
                    "body": "These three.",
                    "created_at": "2026-08-02T09:00:00Z",
                    "path": "a.ts",
                    "position": 10,
                    "original_position": 0,
                    "extra_lines_count": 2,
                },
                {
                    "id": 51,
                    "body": "A reply, at the same position.",
                    "created_at": "2026-08-02T10:00:00Z",
                    "path": "a.ts",
                    "position": 10,
                    "original_position": 0,
                },
            ])),
        );
        let thread = &threads[0];

        assert_eq!(thread.comments.len(), 2);
        assert_eq!(thread.line, Some(12));
        assert_eq!(thread.start_line, Some(10));
        // A reply is aimed at the position the host groups on, which is the first line.
        assert_eq!(thread.id, forgejo_thread_id("a.ts", Side::New, 10));
    }

    #[test]
    fn forgejo_threads_leaves_a_single_line_conversation_without_a_start() {
        let threads = forgejo_threads(
            &[],
            &[de(json!({
                "id": 52,
                "body": "Just this one.",
                "created_at": "2026-08-02T09:00:00Z",
                "path": "a.ts",
                "position": 0,
                "original_position": 7,
                "extra_lines_count": 0,
            }))],
        );
        let thread = &threads[0];
        assert_eq!(thread.line, Some(7));
        assert_eq!(thread.side, Some(Side::Old));
        assert_eq!(thread.start_line, None);
    }

    #[test]
    fn bitbucket_threads_reads_where_a_range_starts_on_either_side() {
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(json!([
            {
                "id": 60,
                "content": { "raw": "These three." },
                "created_on": "2026-08-03T08:00:00Z",
                "inline": { "path": "a.ts", "to": 12, "start_to": 10 },
            },
            {
                "id": 61,
                "content": { "raw": "Those two." },
                "created_on": "2026-08-03T09:00:00Z",
                "inline": { "path": "a.ts", "from": 12, "start_from": 11 },
            },
        ])));
        let (on_new, on_old) = (&threads[0], &threads[1]);

        assert_eq!(on_new.line, Some(12));
        assert_eq!(on_new.start_line, Some(10));
        assert_eq!(on_new.side, Some(Side::New));
        assert_eq!(on_old.line, Some(12));
        assert_eq!(on_old.start_line, Some(11));
        assert_eq!(on_old.side, Some(Side::Old));
    }

    #[test]
    fn gitlab_threads_reads_where_a_range_starts_on_the_side_the_note_is_on() {
        let note = |id: u32, position: Value| {
            json!({
                "id": id.to_string(),
                "notes": [{
                    "id": id,
                    "body": "These lines.",
                    "created_at": "2026-08-04T08:00:00Z",
                    "resolvable": true,
                    "position": position,
                }],
            })
        };
        let threads = gitlab_threads(
            &de::<Vec<GitlabDiscussion>>(json!([
                note(
                    70,
                    json!({
                        "new_path": "a.ts",
                        "new_line": 12,
                        "line_range": { "start": { "old_line": 10, "new_line": 10 }, "end": { "new_line": 12 } },
                    })
                ),
                note(
                    71,
                    json!({
                        "new_path": "a.ts",
                        "old_line": 12,
                        "line_range": { "start": { "old_line": 11 }, "end": { "old_line": 12, "new_line": 13 } },
                    })
                ),
                note(
                    72,
                    json!({
                        "new_path": "a.ts",
                        "new_line": 12,
                        // Drawn in the browser from a removed line, which has no new number.
                        "line_range": { "start": { "old_line": 11 }, "end": { "new_line": 12 } },
                    })
                ),
            ])),
            None,
        );
        let (on_new, on_old, crossed) = (&threads[0], &threads[1], &threads[2]);

        assert_eq!(on_new.line, Some(12));
        assert_eq!(on_new.start_line, Some(10));
        assert_eq!(on_old.line, Some(12));
        assert_eq!(on_old.side, Some(Side::Old));
        assert_eq!(on_old.start_line, Some(11));
        assert_eq!(crossed.start_line, None);
    }

    // --- Rust-specific: the shapes are lenient, and ids are text ---

    #[test]
    fn host_shapes_ignore_unknown_fields_and_tolerate_nulls_everywhere() {
        let comment: GithubComment = de(json!({
            "id": 5,
            "user": null,
            "body": null,
            "created_at": "2026-08-01T09:00:00Z",
            "path": null,
            "line": null,
            "side": null,
            "html_url": "https://github.test/x",
            "reactions": { "+1": 3 },
        }));
        assert_eq!(comment.id, "5");
        assert_eq!(comment.user, None);
        assert_eq!(comment.body, "");
        let thread = &github_flat_threads(&[comment], &[])[0];
        assert_eq!(thread.comments[0].author.name, "unknown");
        assert_eq!(thread.comments[0].author.avatar_url, "");
        assert_eq!(thread.side, None);

        // An empty object is a valid (if useless) value of every shape.
        let _: GithubComment = de(json!({}));
        let _: GitlabDiscussion = de(json!({}));
        let _: ForgejoReviewComment = de(json!({}));
        let _: BitbucketComment = de(json!({}));
        let _: GithubReviewThread = de(json!({}));
    }

    #[test]
    fn host_shapes_take_ids_and_lines_as_numbers_or_strings() {
        let comment: ForgejoReviewComment = de(json!({
            "id": "77",
            "body": "x",
            "created_at": "2026-08-02T09:00:00Z",
            "path": "a.ts",
            "position": "9",
            "extra_lines_count": "2",
        }));
        assert_eq!(comment.id, "77");
        assert_eq!(comment.position, Some(9));
        assert_eq!(comment.extra_lines_count, Some(2));

        // A line that is no line - negative, fractional, nonsense - reads as absent.
        let odd: GithubComment =
            de(json!({ "id": 1, "line": -3, "original_line": 2.5, "start_line": "x" }));
        assert_eq!(
            (odd.line, odd.original_line, odd.start_line),
            (None, None, None)
        );
        let whole: GithubComment = de(json!({ "id": 1, "line": 12.0 }));
        assert_eq!(whole.line, Some(12));
    }

    #[test]
    fn a_malformed_nested_object_reads_as_absent_rather_than_failing_the_response() {
        let comments: Vec<BitbucketComment> = de(json!([
            {
                "id": 1,
                "user": "not an object",
                "content": { "raw": 42 },
                "created_on": "2026-08-03T08:00:00Z",
                "inline": { "path": "a.ts", "to": "nope", "from": 4 },
                "parent": [],
            },
        ]));
        let threads = bitbucket_threads(&comments);
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comments[0].author.name, "unknown");
        assert_eq!(threads[0].comments[0].body, "");
        assert_eq!(threads[0].line, Some(4));
        assert_eq!(threads[0].side, Some(Side::Old));

        // A malformed note in a discussion is dropped; the rest of it survives.
        let discussion: GitlabDiscussion = de(json!({
            "id": "d1",
            "notes": [7, null, { "id": 2, "body": "kept", "created_at": "2026-08-04T08:00:00Z" }],
        }));
        assert_eq!(
            comment_bodies(&gitlab_threads(&[discussion], None)[0]),
            ["kept"]
        );
    }

    #[test]
    fn flags_keep_the_typescripts_two_readings() {
        // Truthiness for `system`, `resolvable` and `deleted`...
        let note: GitlabNote =
            de(json!({ "id": 1, "system": 1, "resolvable": "yes", "resolved": "true" }));
        assert!(note.system);
        assert!(note.resolvable);
        // ...and `=== true` for `resolved` and GitHub's thread flags.
        assert!(!note.resolved);
        let thread: GithubReviewThread =
            de(json!({ "id": "RT", "isResolved": 1, "viewerCanReply": "true" }));
        assert!(!thread.is_resolved);
        assert!(!thread.viewer_can_reply);
        let gone: BitbucketComment = de(json!({ "id": 1, "deleted": 0 }));
        assert!(!gone.deleted);
    }

    #[test]
    fn forgejo_thread_id_keeps_multibyte_paths_and_refuses_what_it_never_wrote() {
        let path = "src/ünï cødé/文件.ts";
        assert_eq!(
            parse_forgejo_thread_id(&forgejo_thread_id(path, Side::Old, 3)),
            Some(ForgejoAnchor {
                path: path.into(),
                side: Side::Old,
                line: 3
            })
        );

        // JavaScript's `.` does not match a line terminator, so neither does this.
        assert_eq!(parse_forgejo_thread_id("fj:new:1:a\nb.ts"), None);
        assert_eq!(parse_forgejo_thread_id("fj:new:1:a\u{2028}b.ts"), None);
        // `\d` is ASCII in JavaScript: an Arabic-Indic digit is not a line number.
        assert_eq!(parse_forgejo_thread_id("fj:new:\u{0663}:a.ts"), None);
        // A line past u32 was never written by this app.
        assert_eq!(parse_forgejo_thread_id("fj:new:99999999999:a.ts"), None);
        assert_eq!(parse_forgejo_thread_id("fj:new::a.ts"), None);
        assert_eq!(
            parse_forgejo_thread_id("fj:old:007:a.ts").map(|anchor| anchor.line),
            Some(7)
        );
    }

    #[test]
    fn forgejo_threads_ignores_a_negative_extra_line_count() {
        let threads = forgejo_threads(
            &[],
            &[de(json!({
                "id": 1, "body": "x", "created_at": "2026-08-02T09:00:00Z",
                "path": "a.ts", "position": 5, "extra_lines_count": -4,
            }))],
        );
        assert_eq!(threads[0].line, Some(5));
        assert_eq!(threads[0].start_line, None);
    }

    #[test]
    fn bitbucket_threads_walks_a_very_long_reply_chain_without_recursing() {
        let mut comments = vec![json!({ "id": 0, "created_on": "2026-08-03T08:00:00Z" })];
        for id in 1..20_000 {
            comments.push(json!({
                "id": id,
                "created_on": "2026-08-03T09:00:00Z",
                "parent": { "id": id - 1 },
            }));
        }
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(Value::Array(comments)));
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comments.len(), 20_000);
        assert_eq!(threads[0].comments[19_999].id, "19999");
    }

    #[test]
    fn bitbucket_threads_drops_a_cycle_that_has_no_root() {
        // Each names the other as its parent: neither opens a thread, exactly as in the
        // TypeScript, and nothing loops.
        let threads = bitbucket_threads(&de::<Vec<BitbucketComment>>(json!([
            { "id": 1, "created_on": "2026-08-03T08:00:00Z", "parent": { "id": 2 } },
            { "id": 2, "created_on": "2026-08-03T09:00:00Z", "parent": { "id": 1 } },
        ])));
        assert!(threads.is_empty(), "{threads:?}");
    }
}
