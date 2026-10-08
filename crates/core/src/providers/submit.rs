//! Turning a set of drafts into whatever each host wants, and being honest when a
//! host that has no batch call stops part-way: a port of
//! src/main/providers/submit.ts.
//!
//! Two of the four take a review and its comments in one request. The other two
//! take them one at a time, which means a submission can half succeed - and the
//! reviewer has to be told which half, because the difference decides whether
//! retrying repeats themselves or finishes the job.
//!
//! A comment covering several lines is the same draft with a `range`, and every
//! host takes the range differently - by start line, by extra line count, or by
//! line code - so each payload builder here reads the same draft its own way.
//!
//! Nothing here does any I/O, so both the payload shapes and the sequential
//! semantics stay reachable from the tests.
//!
//! The payloads are [`serde_json::Value`] objects with exactly the keys the
//! TypeScript builds, in the same order. Where the TypeScript sets a key to
//! `undefined`, the key is left out here: that is what `JSON.stringify` (and the
//! form encoder, for GitLab) put on the wire.

use std::future::Future;

use serde_json::{Map, Value, json};

use crate::error::{Error, Result, msg};
use crate::model::{DraftComment, EdgeKind, LineCommentDraft, LineRange, RangeEdge, ReviewVerdict};

/// The fields that place a comment, shared by a draft and a comment sent directly
/// (the TypeScript's `Pick<LineCommentDraft, 'path' | 'body' | 'newLine' | 'oldLine' | 'range'>`).
///
/// Borrowed, so a builder can be handed a [`DraftComment`] or a [`LineCommentDraft`]
/// with `.into()`, or one put together on the spot (Forgejo's reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed<'a> {
    pub path: &'a str,
    pub body: &'a str,
    pub new_line: Option<u32>,
    pub old_line: Option<u32>,
    pub range: Option<LineRange>,
}

impl<'a> From<&'a DraftComment> for Placed<'a> {
    fn from(draft: &'a DraftComment) -> Self {
        Placed {
            path: &draft.path,
            body: &draft.body,
            new_line: draft.new_line,
            old_line: draft.old_line,
            range: draft.range,
        }
    }
}

impl<'a> From<&'a LineCommentDraft> for Placed<'a> {
    fn from(draft: &'a LineCommentDraft) -> Self {
        Placed {
            path: &draft.path,
            body: &draft.body,
            new_line: draft.new_line,
            old_line: draft.old_line,
            range: draft.range,
        }
    }
}

/// JavaScript truthiness of an optional line: `0` reads as no line, as `undefined` does.
fn is_set(line: Option<u32>) -> bool {
    line.is_some_and(|line| line != 0)
}

/// `src/a.ts:12`, `src/a.ts:10-12`, or just the path for a comment whose line is gone.
fn location(comment: &DraftComment) -> String {
    let Some(line) = comment.new_line.or(comment.old_line) else {
        return comment.path.clone();
    };
    match comment.range {
        None => format!("{}:{line}", comment.path),
        Some(range) => format!("{}:{}-{line}", comment.path, range.start_line),
    }
}

/// How many drafts a message names before it only counts the rest.
const NAMED: usize = 4;

fn list(comments: &[DraftComment]) -> String {
    let named = comments
        .iter()
        .take(NAMED)
        .map(location)
        .collect::<Vec<_>>()
        .join(", ");
    match comments.len().saturating_sub(NAMED) {
        0 => named,
        rest => format!("{named} and {rest} more"),
    }
}

/// A submission that got some of the way (the TypeScript's `PartialSubmitError`).
/// Carries both halves, so the caller can drop the drafts that landed and keep the
/// ones that did not - which is what makes retrying finish the review rather than
/// say everything twice.
///
/// Comments are posted in order, so what landed is always a prefix of the drafts and
/// what did not is the rest of them; when only the verdict failed, `unposted` is
/// empty. It becomes [`Error::PartialSubmit`] (with the ids of `posted`) through
/// [`PartialSubmit::into_error`] or `From`.
#[derive(Debug, Clone)]
pub struct PartialSubmit<'a> {
    pub posted: &'a [DraftComment],
    pub unposted: &'a [DraftComment],
    /// What stopped the submission, as the host reported it.
    pub cause: Error,
}

impl PartialSubmit<'_> {
    /// The sentence the reviewer reads: how far it got, why it stopped, and which
    /// remarks are already out there.
    pub fn message(&self) -> String {
        let total = self.posted.len() + self.unposted.len();
        let tail = if self.unposted.is_empty() {
            "Every comment landed; only the verdict did not.".to_owned()
        } else {
            format!("Still drafted: {}.", list(self.unposted))
        };
        format!(
            "Posted {} of {total} comment{}, then stopped: {}. Posted: {}. {tail}",
            self.posted.len(),
            if total == 1 { "" } else { "s" },
            self.cause,
            list(self.posted),
        )
    }

    pub fn into_error(self) -> Error {
        Error::PartialSubmit {
            message: self.message(),
            posted: self
                .posted
                .iter()
                .map(|comment| comment.id.clone())
                .collect(),
        }
    }
}

impl From<PartialSubmit<'_>> for Error {
    fn from(partial: PartialSubmit<'_>) -> Self {
        partial.into_error()
    }
}

/// How a sequential submission stopped, before it is flattened into an [`Error`].
#[derive(Debug)]
enum Stopped<'a> {
    /// Nothing had landed; the host's own error stands.
    Host(Error),
    Partial(PartialSubmit<'a>),
}

/// Posts each comment in order and then applies the verdict, for a host with no call
/// that takes both.
///
/// The verdict goes last so the author's one notification about it arrives with the
/// comments already in place. If anything fails once a comment has landed, the
/// failure names what got through - [`Error::PartialSubmit`], whose `posted` lists
/// the draft ids that landed - including the case where every comment posted and
/// only the verdict did not, which still leaves the reviewer needing to know their
/// remarks are already out there. When nothing landed, the host's own error is
/// returned unchanged.
pub async fn submit_sequentially<'a, P, PostFut, V, VerdictFut>(
    comments: &'a [DraftComment],
    post: P,
    verdict: V,
) -> Result<()>
where
    P: FnMut(&'a DraftComment) -> PostFut,
    PostFut: Future<Output = Result<()>>,
    V: FnOnce() -> VerdictFut,
    VerdictFut: Future<Output = Result<()>>,
{
    run_sequentially(comments, post, verdict)
        .await
        .map_err(|stopped| match stopped {
            Stopped::Host(error) => error,
            Stopped::Partial(partial) => partial.into_error(),
        })
}

async fn run_sequentially<'a, P, PostFut, V, VerdictFut>(
    comments: &'a [DraftComment],
    mut post: P,
    verdict: V,
) -> std::result::Result<(), Stopped<'a>>
where
    P: FnMut(&'a DraftComment) -> PostFut,
    PostFut: Future<Output = Result<()>>,
    V: FnOnce() -> VerdictFut,
    VerdictFut: Future<Output = Result<()>>,
{
    for (index, comment) in comments.iter().enumerate() {
        if let Err(error) = post(comment).await {
            // Nothing landed, so nothing needs explaining - the host's own error is the
            // clearest thing the reviewer can be told.
            if index == 0 {
                return Err(Stopped::Host(error));
            }
            return Err(Stopped::Partial(PartialSubmit {
                posted: &comments[..index],
                unposted: &comments[index..],
                cause: error,
            }));
        }
    }

    verdict().await.map_err(|error| {
        if comments.is_empty() {
            Stopped::Host(error)
        } else {
            Stopped::Partial(PartialSubmit {
                posted: comments,
                unposted: &[],
                cause: error,
            })
        }
    })
}

fn object(entries: Map<String, Value>) -> Value {
    Value::Object(entries)
}

/// How GitHub addresses a comment: by side and line, plus the start of the range
/// when there is one. The same shape serves a review's comment list and a comment
/// posted on its own. A range stays on one side here, so the start side is the
/// side.
pub fn github_comment_position(comment: Placed<'_>) -> Value {
    let side = if is_set(comment.new_line) {
        "RIGHT"
    } else {
        "LEFT"
    };
    let mut out = Map::new();
    out.insert("path".into(), json!(comment.path));
    out.insert("body".into(), json!(comment.body));
    out.insert("side".into(), json!(side));
    if let Some(line) = comment.new_line.or(comment.old_line) {
        out.insert("line".into(), json!(line));
    }
    if let Some(range) = comment.range {
        out.insert("start_side".into(), json!(side));
        out.insert("start_line".into(), json!(range.start_line));
    }
    object(out)
}

/// GitHub takes its comments on review creation, addressed by side and line.
pub fn github_review_comments(comments: &[DraftComment]) -> Vec<Value> {
    comments
        .iter()
        .map(|comment| github_comment_position(comment.into()))
        .collect()
}

/// Forgejo addresses a line by position, zero meaning not on that side, and a range
/// the other way round from everyone else: anchored at its first line, with a count
/// of how many more follow it on the same side. The comment is still shown on the
/// last line, so a draft converts rather than changing where it is.
///
/// A Forgejo or Gitea too old to know the count ignores it, and the comment lands
/// on the first line of the range as a single-line comment - the nearest thing it
/// can do.
pub fn forgejo_inline_comment(comment: Placed<'_>) -> Value {
    let start = comment.range.map(|range| range.start_line);
    let new_line = comment.new_line.map(|line| start.unwrap_or(line));
    let old_line = comment.old_line.map(|line| start.unwrap_or(line));
    let last = comment.new_line.or(comment.old_line);

    let mut out = Map::new();
    out.insert("path".into(), json!(comment.path));
    out.insert("body".into(), json!(comment.body));
    out.insert("new_position".into(), json!(new_line.unwrap_or(0)));
    out.insert("old_position".into(), json!(old_line.unwrap_or(0)));
    if let (Some(start), Some(last)) = (start, last) {
        // Signed, as in the TypeScript: a range that somehow ends before it starts is
        // sent as it is rather than wrapping around.
        out.insert(
            "extra_lines_count".into(),
            json!(i64::from(last) - i64::from(start)),
        );
    }
    object(out)
}

/// Forgejo takes them the same way, addressed by position instead: zero means the
/// comment is not on that side.
pub fn forgejo_review_payload(
    verdict: ReviewVerdict,
    body: &str,
    comments: &[DraftComment],
) -> Value {
    let event = match verdict {
        ReviewVerdict::Approve => "APPROVED",
        ReviewVerdict::RequestChanges => "REQUEST_CHANGES",
        ReviewVerdict::Comment => "COMMENT",
    };

    // A review with neither a body nor comments is rejected, so always say something.
    let body = if !body.is_empty() {
        body
    } else if !comments.is_empty() {
        ""
    } else if verdict == ReviewVerdict::Approve {
        "Approved."
    } else {
        "Reviewed."
    };

    let mut out = Map::new();
    out.insert("event".into(), json!(event));
    out.insert("body".into(), json!(body));
    if let Some(head) = comments
        .first()
        .and_then(|comment| comment.refs.head_sha.as_deref())
    {
        out.insert("commit_id".into(), json!(head));
    }
    out.insert(
        "comments".into(),
        Value::Array(
            comments
                .iter()
                .map(|comment| forgejo_inline_comment(comment.into()))
                .collect(),
        ),
    );
    object(out)
}

/// GitLab's name for a line: the file hashed, then where the line sits in the old
/// file and in the new one as the diff counts it - which is exactly what a
/// [`RangeEdge`] carries.
pub fn gitlab_line_code(path: &str, edge: RangeEdge) -> String {
    format!(
        "{}_{}_{}",
        sha1_hex(path.as_bytes()),
        edge.old_pos,
        edge.new_pos
    )
}

/// One end of a GitLab line range, typed the way its own diff view sends it.
fn gitlab_range_edge(path: &str, edge: RangeEdge) -> Value {
    let mut out = Map::new();
    out.insert("line_code".into(), json!(gitlab_line_code(path, edge)));
    // An unchanged line has no type, as in the browser: it is neither side's.
    match edge.kind {
        EdgeKind::Add => {
            out.insert("type".into(), json!("new"));
        }
        EdgeKind::Del => {
            out.insert("type".into(), json!("old"));
        }
        EdgeKind::Context => {}
    }
    if edge.kind != EdgeKind::Add {
        out.insert("old_line".into(), json!(edge.old_pos));
    }
    if edge.kind != EdgeKind::Del {
        out.insert("new_line".into(), json!(edge.new_pos));
    }
    object(out)
}

/// GitLab addresses a line by the three shas the diff was read at, which is why a
/// draft records them: without them the comment cannot be placed at all.
pub fn gitlab_discussion_payload(comment: &DraftComment) -> Result<Value> {
    let sha = |sha: &Option<String>| sha.clone().filter(|sha| !sha.is_empty());
    let (Some(base_sha), Some(start_sha), Some(head_sha)) = (
        sha(&comment.refs.base_sha),
        sha(&comment.refs.start_sha),
        sha(&comment.refs.head_sha),
    ) else {
        return Err(msg(
            "GitLab needs the merge request diff refs; reload the merge request and try again.",
        ));
    };

    let mut position = Map::new();
    position.insert("position_type".into(), json!("text"));
    position.insert("base_sha".into(), json!(base_sha));
    position.insert("start_sha".into(), json!(start_sha));
    position.insert("head_sha".into(), json!(head_sha));
    position.insert("new_path".into(), json!(comment.path));
    position.insert("old_path".into(), json!(comment.path));
    // Added lines carry only new_line, removed lines only old_line, context both.
    if let Some(line) = comment.new_line {
        position.insert("new_line".into(), json!(line));
    }
    if let Some(line) = comment.old_line {
        position.insert("old_line".into(), json!(line));
    }
    if let Some(range) = comment.range {
        position.insert(
            "line_range".into(),
            json!({
                "start": gitlab_range_edge(&comment.path, range.start),
                "end": gitlab_range_edge(&comment.path, range.end),
            }),
        );
    }

    Ok(json!({
        "body": comment.body,
        "position": object(position),
    }))
}

/// Bitbucket addresses the new file with `to` and the old one with `from`, and a
/// range by where it starts on the same side.
pub fn bitbucket_comment_payload(comment: Placed<'_>) -> Value {
    let start = comment.range.map(|range| range.start_line);
    let mut inline = Map::new();
    inline.insert("path".into(), json!(comment.path));
    match comment.new_line.filter(|line| *line != 0) {
        Some(to) => {
            inline.insert("to".into(), json!(to));
            if let Some(start) = start {
                inline.insert("start_to".into(), json!(start));
            }
        }
        None => {
            if let Some(from) = comment.old_line {
                inline.insert("from".into(), json!(from));
            }
            if let Some(start) = start {
                inline.insert("start_from".into(), json!(start));
            }
        }
    }
    json!({
        "content": { "raw": comment.body },
        "inline": object(inline),
    })
}

/// SHA-1 of `data` as lowercase hex, for GitLab's line codes (FIPS 180-4).
///
/// Hand-written rather than a new dependency: it is a few dozen lines, it hashes a
/// file path, and nothing here relies on SHA-1 for security - GitLab simply names
/// its lines that way.
fn sha1_hex(data: &[u8]) -> String {
    let mut state: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    // Padding: a single 1 bit, zeros up to 56 bytes mod 64, then the length in bits.
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    // The padded message is a whole number of 64-byte blocks, so nothing is left over.
    for block in message.as_chunks::<64>().0 {
        let mut schedule = [0u32; 80];
        for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for t in 16..80 {
            schedule[t] = (schedule[t - 3] ^ schedule[t - 8] ^ schedule[t - 14] ^ schedule[t - 16])
                .rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (t, word) in schedule.iter().enumerate() {
            let (f, k) = match t {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }

        for (value, add) in state.iter_mut().zip([a, b, c, d, e]) {
            *value = value.wrapping_add(add);
        }
    }

    state.iter().map(|word| format!("{word:08x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DiffRefs;
    use futures::executor::block_on;
    use std::cell::RefCell;

    fn refs() -> DiffRefs {
        DiffRefs {
            base_sha: Some("base1".into()),
            start_sha: Some("start1".into()),
            head_sha: Some("head1".into()),
        }
    }

    fn draft() -> DraftComment {
        DraftComment {
            id: "d1".into(),
            item_id: "acct:repo:1".into(),
            body: "a remark".into(),
            path: "src/a.ts".into(),
            new_line: Some(12),
            old_line: None,
            range: None,
            created_at: "2026-08-05T10:00:00Z".into(),
            refs: refs(),
        }
    }

    fn at(id: &str, path: &str, new_line: u32) -> DraftComment {
        DraftComment {
            id: id.into(),
            path: path.into(),
            new_line: Some(new_line),
            ..draft()
        }
    }

    fn ids(comments: &[DraftComment]) -> Vec<&str> {
        comments.iter().map(|comment| comment.id.as_str()).collect()
    }

    /// The test's `.then(() => null, (error) => error)`.
    fn failure(result: Result<()>) -> Error {
        match result {
            Ok(()) => panic!("the submission should have failed"),
            Err(error) => error,
        }
    }

    fn partial(stopped: std::result::Result<(), Stopped<'_>>) -> PartialSubmit<'_> {
        match stopped {
            Err(Stopped::Partial(partial)) => partial,
            other => panic!("expected a partial submission, got {other:?}"),
        }
    }

    #[test]
    fn github_review_comments_addresses_each_line_by_side() {
        assert_eq!(
            github_review_comments(&[
                DraftComment {
                    id: "d1".into(),
                    new_line: Some(12),
                    ..draft()
                },
                DraftComment {
                    id: "d2".into(),
                    new_line: None,
                    old_line: Some(30),
                    path: "src/b.ts".into(),
                    body: "gone why?".into(),
                    ..draft()
                },
            ]),
            [
                json!({ "path": "src/a.ts", "body": "a remark", "side": "RIGHT", "line": 12 }),
                json!({ "path": "src/b.ts", "body": "gone why?", "side": "LEFT", "line": 30 }),
            ]
        );
    }

    #[test]
    fn forgejo_review_payload_carries_the_comments_with_the_verdict_in_one_request() {
        let payload = forgejo_review_payload(
            ReviewVerdict::RequestChanges,
            "Please fix.",
            &[
                DraftComment {
                    id: "d1".into(),
                    new_line: Some(12),
                    ..draft()
                },
                DraftComment {
                    id: "d2".into(),
                    new_line: None,
                    old_line: Some(30),
                    path: "src/b.ts".into(),
                    body: "gone why?".into(),
                    ..draft()
                },
            ],
        );

        assert_eq!(payload["event"], "REQUEST_CHANGES");
        assert_eq!(payload["body"], "Please fix.");
        // Against the commit the drafts were written on, not whatever the branch is now.
        assert_eq!(payload["commit_id"], "head1");
        assert_eq!(
            payload["comments"],
            json!([
                { "path": "src/a.ts", "body": "a remark", "new_position": 12, "old_position": 0 },
                { "path": "src/b.ts", "body": "gone why?", "new_position": 0, "old_position": 30 },
            ])
        );
    }

    #[test]
    fn forgejo_review_payload_says_something_when_there_is_nothing_else_to_say() {
        // A review with neither a body nor comments is rejected outright.
        assert_eq!(
            forgejo_review_payload(ReviewVerdict::Approve, "", &[])["body"],
            "Approved."
        );
        assert_eq!(
            forgejo_review_payload(ReviewVerdict::Comment, "", &[])["body"],
            "Reviewed."
        );

        // With comments there is content, so nothing has to be invented.
        assert_eq!(
            forgejo_review_payload(ReviewVerdict::Comment, "", &[draft()])["body"],
            ""
        );
    }

    #[test]
    fn gitlab_discussion_payload_places_a_comment_with_the_shas_the_draft_recorded() {
        let payload = gitlab_discussion_payload(&draft()).expect("refs are recorded");

        assert_eq!(payload["body"], "a remark");
        assert_eq!(
            payload["position"],
            json!({
                "position_type": "text",
                "base_sha": "base1",
                "start_sha": "start1",
                "head_sha": "head1",
                "new_path": "src/a.ts",
                "old_path": "src/a.ts",
                "new_line": 12,
            })
        );
    }

    #[test]
    fn gitlab_discussion_payload_refuses_a_draft_it_cannot_place() {
        // Without the refs the comment has nowhere to go, and guessing would put someone
        // else's words on a line they never read.
        let only_head = DraftComment {
            refs: DiffRefs {
                head_sha: Some("head1".into()),
                ..DiffRefs::default()
            },
            ..draft()
        };
        let none = DraftComment {
            refs: DiffRefs::default(),
            ..draft()
        };
        for comment in [only_head, none] {
            let error = gitlab_discussion_payload(&comment).expect_err("no refs, no payload");
            assert!(error.to_string().contains("diff refs"), "{error}");
        }
    }

    #[test]
    fn bitbucket_comment_payload_addresses_the_new_file_with_to_and_the_old_with_from() {
        assert_eq!(
            bitbucket_comment_payload((&draft()).into()),
            json!({ "content": { "raw": "a remark" }, "inline": { "path": "src/a.ts", "to": 12 } })
        );
        let old = DraftComment {
            new_line: None,
            old_line: Some(30),
            ..draft()
        };
        assert_eq!(
            bitbucket_comment_payload((&old).into()),
            json!({ "content": { "raw": "a remark" }, "inline": { "path": "src/a.ts", "from": 30 } })
        );
    }

    #[test]
    fn submit_sequentially_posts_every_comment_in_order_then_the_verdict() {
        let order = RefCell::new(Vec::new());
        let comments = [
            at("d1", "src/a.ts", 12),
            at("d2", "src/a.ts", 12),
            at("d3", "src/a.ts", 12),
        ];

        block_on(submit_sequentially(
            &comments,
            |comment| {
                order.borrow_mut().push(comment.id.clone());
                async { Ok(()) }
            },
            || {
                order.borrow_mut().push("verdict".to_owned());
                async { Ok(()) }
            },
        ))
        .expect("everything lands");

        // The verdict last, so the author's notification about it finds the comments there.
        assert_eq!(*order.borrow(), ["d1", "d2", "d3", "verdict"]);
    }

    #[test]
    fn submit_sequentially_reports_which_comments_landed_and_which_did_not() {
        let comments = [
            at("d1", "src/a.ts", 1),
            at("d2", "src/b.ts", 2),
            at("d3", "src/c.ts", 3),
            at("d4", "src/d.ts", 4),
        ];
        let post = |comment: &DraftComment| {
            let fails = comment.id == "d3";
            async move {
                if fails {
                    Err(msg("Bitbucket returned 429."))
                } else {
                    Ok(())
                }
            }
        };
        let verdict = || async { panic!("the verdict should not be reached") };

        let stopped = partial(block_on(run_sequentially(&comments, post, verdict)));
        assert_eq!(ids(stopped.posted), ["d1", "d2"]);
        assert_eq!(ids(stopped.unposted), ["d3", "d4"]);

        let failure = failure(block_on(submit_sequentially(&comments, post, verdict)));
        let Error::PartialSubmit { message, posted } = &failure else {
            panic!("expected a partial submission, got {failure:?}");
        };
        assert_eq!(posted, &["d1", "d2"]);

        // The message has to be enough on its own to know what to do next.
        assert!(message.contains("Posted 2 of 4 comments"), "{message}");
        assert!(message.contains("Bitbucket returned 429."), "{message}");
        assert!(
            message.contains("Posted: src/a.ts:1, src/b.ts:2"),
            "{message}"
        );
        assert!(
            message.contains("Still drafted: src/c.ts:3, src/d.ts:4"),
            "{message}"
        );
        assert_eq!(failure.to_string(), *message);
    }

    #[test]
    fn submit_sequentially_lets_the_host_speak_for_itself_when_nothing_landed() {
        // No half state to explain, so the reviewer gets the host's own error.
        let failure = failure(block_on(submit_sequentially(
            &[draft()],
            |_| async { Err(msg("Not authorised on bitbucket.org.")) },
            || async { panic!("the verdict should not be reached") },
        )));

        assert!(!matches!(failure, Error::PartialSubmit { .. }));
        assert_eq!(failure.to_string(), "Not authorised on bitbucket.org.");
    }

    #[test]
    fn submit_sequentially_still_explains_itself_when_only_the_verdict_fails() {
        // Every remark is already out there; the reviewer needs to know that before they
        // decide whether to try again.
        let comments = [at("d1", "src/a.ts", 1)];
        let post = |_: &DraftComment| async { Ok(()) };
        let verdict = || async { Err(msg("Approval was refused.")) };

        let stopped = partial(block_on(run_sequentially(&comments, post, verdict)));
        assert_eq!(ids(stopped.posted), ["d1"]);
        assert!(stopped.unposted.is_empty());

        let failure = failure(block_on(submit_sequentially(&comments, post, verdict)));
        let Error::PartialSubmit { message, posted } = &failure else {
            panic!("expected a partial submission, got {failure:?}");
        };
        assert_eq!(posted, &["d1"]);
        assert!(
            message.contains("Every comment landed; only the verdict did not."),
            "{message}"
        );
    }

    #[test]
    fn submit_sequentially_with_no_comments_is_just_the_verdict() {
        let applied = RefCell::new(false);
        block_on(submit_sequentially(
            &[],
            |_| async { panic!("nothing to post") },
            || {
                *applied.borrow_mut() = true;
                async { Ok(()) }
            },
        ))
        .expect("the verdict lands");
        assert!(*applied.borrow());

        // And a verdict that fails on its own stays the host's error, not a partial one.
        let failure = failure(block_on(submit_sequentially(
            &[],
            |_| async { Ok(()) },
            || async { Err(msg("nope")) },
        )));
        assert!(!matches!(failure, Error::PartialSubmit { .. }));
    }

    #[test]
    fn a_long_partial_failure_names_a_few_and_counts_the_rest() {
        let comments: Vec<DraftComment> = (0..9)
            .map(|index| at(&format!("d{index}"), &format!("src/f{index}.ts"), index))
            .collect();

        let failure = failure(block_on(submit_sequentially(
            &comments,
            |comment| {
                let fails = comment.id == "d6";
                async move { if fails { Err(msg("stopped")) } else { Ok(()) } }
            },
            || async { Ok(()) },
        )));
        let message = failure.to_string();

        assert!(message.contains("Posted 6 of 9 comments"), "{message}");
        assert!(message.contains("and 2 more"), "{message}");
    }

    // --- ranges ---

    /// Lines 10 to 12 on the new side: a context line, then two added ones.
    const NEW_RANGE: LineRange = LineRange {
        start_line: 10,
        start: RangeEdge {
            kind: EdgeKind::Context,
            old_pos: 10,
            new_pos: 10,
        },
        end: RangeEdge {
            kind: EdgeKind::Add,
            old_pos: 11,
            new_pos: 12,
        },
    };

    /// Lines 11 to 12 on the old side: a removed line, then a context one.
    const OLD_RANGE: LineRange = LineRange {
        start_line: 11,
        start: RangeEdge {
            kind: EdgeKind::Del,
            old_pos: 11,
            new_pos: 11,
        },
        end: RangeEdge {
            kind: EdgeKind::Context,
            old_pos: 12,
            new_pos: 13,
        },
    };

    fn new_range() -> DraftComment {
        DraftComment {
            new_line: Some(12),
            range: Some(NEW_RANGE),
            ..draft()
        }
    }

    fn old_range() -> DraftComment {
        DraftComment {
            new_line: None,
            old_line: Some(12),
            range: Some(OLD_RANGE),
            ..draft()
        }
    }

    /// The path's hash, kept independent of the module under test (it is what
    /// `shasum` prints for `src/a.ts`), so the test says what the code is.
    const SHA1_OF_PATH: &str = "21ff24dd18cc19d35de15a120639695824655b58";

    #[test]
    fn github_comment_position_names_the_start_of_a_range_on_the_same_side() {
        assert_eq!(
            github_review_comments(&[new_range()]),
            [json!({
                "path": "src/a.ts", "body": "a remark", "side": "RIGHT", "line": 12,
                "start_side": "RIGHT", "start_line": 10,
            })]
        );
        assert_eq!(
            github_review_comments(&[old_range()]),
            [json!({
                "path": "src/a.ts", "body": "a remark", "side": "LEFT", "line": 12,
                "start_side": "LEFT", "start_line": 11,
            })]
        );
    }

    #[test]
    fn forgejo_inline_comment_anchors_a_range_at_its_first_line_and_counts_the_rest() {
        assert_eq!(
            forgejo_inline_comment((&new_range()).into()),
            json!({
                "path": "src/a.ts",
                "body": "a remark",
                "new_position": 10,
                "old_position": 0,
                "extra_lines_count": 2,
            })
        );
        assert_eq!(
            forgejo_inline_comment((&old_range()).into()),
            json!({
                "path": "src/a.ts",
                "body": "a remark",
                "new_position": 0,
                "old_position": 11,
                "extra_lines_count": 1,
            })
        );
        // A single line says nothing about a count, so an older host sees the same request as before.
        assert_eq!(
            forgejo_inline_comment((&draft()).into()),
            json!({
                "path": "src/a.ts",
                "body": "a remark",
                "new_position": 12,
                "old_position": 0,
            })
        );
    }

    #[test]
    fn gitlab_line_code_hashes_the_path_and_names_both_positions_old_first() {
        assert_eq!(
            gitlab_line_code(
                "src/a.ts",
                RangeEdge {
                    kind: EdgeKind::Add,
                    old_pos: 11,
                    new_pos: 12
                }
            ),
            format!("{SHA1_OF_PATH}_11_12")
        );
    }

    #[test]
    fn gitlab_discussion_payload_adds_a_line_range_keyed_by_line_codes_typed_as_the_browser_types_them()
     {
        let payload = gitlab_discussion_payload(&new_range()).expect("refs are recorded");
        assert_eq!(payload["position"]["new_line"], 12);
        assert_eq!(
            payload["position"]["line_range"],
            json!({
                "start": {
                    "line_code": format!("{SHA1_OF_PATH}_10_10"),
                    // An unchanged line is neither side's, so it has no type.
                    "old_line": 10,
                    "new_line": 10,
                },
                "end": {
                    "line_code": format!("{SHA1_OF_PATH}_11_12"),
                    "type": "new",
                    "new_line": 12,
                },
            })
        );

        let old = gitlab_discussion_payload(&old_range()).expect("refs are recorded");
        assert_eq!(
            old["position"]["line_range"]["start"],
            json!({
                "line_code": format!("{SHA1_OF_PATH}_11_11"),
                "type": "old",
                "old_line": 11,
            })
        );
    }

    #[test]
    fn gitlab_discussion_payload_sends_no_line_range_for_a_single_line() {
        let payload = gitlab_discussion_payload(&draft()).expect("refs are recorded");
        assert!(payload["position"].get("line_range").is_none());
    }

    #[test]
    fn bitbucket_comment_payload_names_where_a_range_starts_on_the_same_side() {
        assert_eq!(
            bitbucket_comment_payload((&new_range()).into()),
            json!({
                "content": { "raw": "a remark" },
                "inline": { "path": "src/a.ts", "to": 12, "start_to": 10 },
            })
        );
        assert_eq!(
            bitbucket_comment_payload((&old_range()).into()),
            json!({
                "content": { "raw": "a remark" },
                "inline": { "path": "src/a.ts", "from": 12, "start_from": 11 },
            })
        );
    }

    #[test]
    fn a_partial_submission_names_a_range_by_both_its_ends() {
        let comments = [new_range(), at("d2", "src/b.ts", 3)];
        let failure = failure(block_on(submit_sequentially(
            &comments,
            |comment| {
                let fails = comment.id == "d2";
                async move { if fails { Err(msg("nope")) } else { Ok(()) } }
            },
            || async { Ok(()) },
        )));
        let message = failure.to_string();
        assert!(
            message.contains("Posted: src/a.ts:10-12. Still drafted: src/b.ts:3."),
            "{message}"
        );
    }

    // --- Rust-specific ---

    #[test]
    fn the_partial_message_reads_in_full_and_counts_one_comment_in_the_singular() {
        let comments = [at("d1", "src/a.ts", 1)];
        let partial = PartialSubmit {
            posted: &comments,
            unposted: &[],
            cause: msg("Approval was refused."),
        };
        assert_eq!(
            partial.message(),
            "Posted 1 of 1 comment, then stopped: Approval was refused.. Posted: src/a.ts:1. \
             Every comment landed; only the verdict did not."
        );

        // A draft whose line has gone is named by its path alone.
        let lineless = DraftComment {
            new_line: None,
            old_line: None,
            ..draft()
        };
        assert_eq!(location(&lineless), "src/a.ts");
    }

    #[test]
    fn payloads_leave_out_what_the_typescript_leaves_undefined() {
        // No line at all: no `line` key for GitHub, no `from` for Bitbucket, zero
        // positions for Forgejo.
        let lineless = DraftComment {
            new_line: None,
            old_line: None,
            ..draft()
        };
        assert_eq!(
            github_comment_position((&lineless).into()),
            json!({ "path": "src/a.ts", "body": "a remark", "side": "LEFT" })
        );
        assert_eq!(
            bitbucket_comment_payload((&lineless).into()),
            json!({ "content": { "raw": "a remark" }, "inline": { "path": "src/a.ts" } })
        );

        // No drafts, so no commit to pin the review to.
        let payload = forgejo_review_payload(ReviewVerdict::Approve, "Nice.", &[]);
        assert!(payload.get("commit_id").is_none());
        assert_eq!(payload["comments"], json!([]));

        // An empty sha is as missing as an absent one.
        let blank = DraftComment {
            refs: DiffRefs {
                base_sha: Some(String::new()),
                ..refs()
            },
            ..draft()
        };
        assert!(gitlab_discussion_payload(&blank).is_err());
    }

    #[test]
    fn placed_reads_a_line_comment_draft_the_same_as_a_draft() {
        let direct = LineCommentDraft {
            item_id: "acct:repo:1".into(),
            body: "a remark".into(),
            path: "src/a.ts".into(),
            new_line: Some(12),
            old_line: None,
            range: Some(NEW_RANGE),
        };
        assert_eq!(Placed::from(&direct), Placed::from(&new_range()));
    }

    #[test]
    fn sha1_matches_the_standard_vectors_and_every_padding_boundary() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(sha1_hex(b"src/a.ts"), SHA1_OF_PATH);
        // Paths are hashed as UTF-8, as Node's `update(string)` does.
        assert_eq!(
            sha1_hex("src/ünï cødé/文件.ts".as_bytes()),
            "2d5aae3ee4422bc41064d3a8f9dc398bee793f7a"
        );
        for (length, digest) in [
            (55, "cef734ba81a024479e09eb5a75b6ddae62e6abf1"),
            (56, "901305367c259952f4e7af8323f480d59f81335b"),
            (63, "0ddc4e0cccd9a12850deb5abb0853a4425559fec"),
            (64, "bb2fa3ee7afb9f54c6dfb5d021f14b1ffe40c163"),
            (65, "78c741ddc482e4cdf8c474a0876347a0905b6233"),
            (119, "4300320394f7ee239bcdce7d3b8bcee173a0cd5c"),
            (120, "ceb2821639c4b6dcb10bce0e522ca2e608ce056d"),
        ] {
            assert_eq!(sha1_hex(&vec![b'x'; length]), digest, "{length} bytes");
        }
        assert_eq!(
            sha1_hex(&vec![b'a'; 1_000_000]),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }
}
