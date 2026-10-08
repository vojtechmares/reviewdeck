//! Whether a re-read of the conversation says anything the app is not already
//! showing. A port of src/shared/threads.ts.
//!
//! The sync behind an open pull request comes round every poll, and nearly every one
//! of them hands back the conversation exactly as it stands. Passing those on is not
//! free: the diff rebuilds around its inline threads and the page the reviewer was
//! reading moves under them, which is precisely what a background refresh must never
//! do. So a re-read only reaches the view when something in it changed.
//!
//! Everything the app draws from a thread goes into the comparison - what was said as
//! much as who said it, since a comment can be edited after the fact - and nothing
//! else does.

use crate::model::{CommentThread, PullComment, Side};

/// The drawn fields of one comment.
type CommentSignature<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str);

/// The drawn fields of one thread, in the order the TypeScript signature lists them.
type ThreadSignature<'a> = (
    &'a str,
    bool,
    bool,
    &'a str,
    Option<u32>,
    Option<Side>,
    bool,
    bool,
    Vec<CommentSignature<'a>>,
);

fn comment_signature(comment: &PullComment) -> CommentSignature<'_> {
    (
        &comment.id,
        &comment.author.name,
        &comment.author.avatar_url,
        &comment.created_at,
        &comment.body,
    )
}

/// Every drawn field of a thread, as one comparable value.
///
/// The TypeScript serialises these as JSON arrays so that fields cannot run together
/// into a false match; comparing them field by field gives the same guarantee
/// without building a string. A missing path reads as an empty one, as `?? ''` reads
/// it there.
fn signature(thread: &CommentThread) -> ThreadSignature<'_> {
    (
        &thread.id,
        thread.resolved,
        thread.outdated,
        thread.path.as_deref().unwrap_or(""),
        thread.line,
        thread.side,
        thread.can_reply,
        thread.can_resolve,
        thread.comments.iter().map(comment_signature).collect(),
    )
}

/// Whether two reads of a conversation draw the same thing.
pub fn same_conversation(a: &[CommentThread], b: &[CommentThread]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(left, right)| signature(left) == signature(right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::User;

    fn comment(id: &str, name: &str, avatar_url: &str, body: &str, at: &str) -> PullComment {
        PullComment {
            id: id.into(),
            author: User {
                name: name.into(),
                avatar_url: avatar_url.into(),
            },
            body: body.into(),
            created_at: at.into(),
        }
    }

    fn thread() -> CommentThread {
        CommentThread {
            id: "t1".into(),
            resolved: false,
            outdated: false,
            can_reply: true,
            can_resolve: true,
            comments: vec![comment(
                "c1",
                "mnovotna",
                "https://gl.test/m.png",
                "Can this go into config?",
                "2026-08-04T08:00:00Z",
            )],
            path: None,
            line: None,
            start_line: None,
            side: None,
        }
    }

    fn with_id(id: &str) -> CommentThread {
        CommentThread {
            id: id.into(),
            ..thread()
        }
    }

    #[test]
    fn same_conversation_holds_a_re_read_that_says_nothing_new() {
        assert!(same_conversation(&[thread()], &[thread()]));
        assert!(same_conversation(&[], &[]));
    }

    #[test]
    fn same_conversation_spots_a_reply_arriving_in_a_thread() {
        let mut answered = thread();
        answered.comments.push(comment(
            "c2",
            "hkramer",
            "",
            "Good call, in a follow-up.",
            "2026-08-04T09:00:00Z",
        ));

        assert!(!same_conversation(&[thread()], &[answered]));
    }

    #[test]
    fn same_conversation_spots_a_whole_thread_arriving_or_leaving() {
        assert!(!same_conversation(&[thread()], &[thread(), with_id("t2")]));
        assert!(!same_conversation(&[thread(), with_id("t2")], &[thread()]));
    }

    #[test]
    fn same_conversation_spots_a_thread_being_resolved_without_a_word_said() {
        let resolved = CommentThread {
            resolved: true,
            ..thread()
        };
        assert!(!same_conversation(&[thread()], &[resolved]));
    }

    #[test]
    fn same_conversation_spots_a_comment_edited_in_place() {
        let mut edited = thread();
        edited.comments[0].body = "Can this go into config, please?".into();

        assert!(!same_conversation(&[thread()], &[edited]));
    }

    #[test]
    fn same_conversation_spots_a_thread_that_moved_in_the_diff() {
        let on_line = |line: u32| CommentThread {
            line: Some(line),
            ..thread()
        };
        let on_path = |path: &str| CommentThread {
            path: Some(path.into()),
            ..thread()
        };
        assert!(!same_conversation(&[on_line(12)], &[on_line(40)]));
        assert!(!same_conversation(&[on_path("a.ts")], &[on_path("b.ts")]));
    }

    /// Two threads whose fields run together must not compare equal just because the
    /// concatenation matches - the separators are what stop that.
    #[test]
    fn same_conversation_does_not_confuse_two_threads_whose_text_runs_together() {
        let split = [with_id("ab"), with_id("c")];
        let joined = [with_id("a"), with_id("bc")];

        assert!(!same_conversation(&split, &joined));
    }

    #[test]
    fn same_conversation_spots_a_side_or_a_capability_changing() {
        let on_side = |side: Side| CommentThread {
            side: Some(side),
            ..thread()
        };
        assert!(!same_conversation(
            &[on_side(Side::Old)],
            &[on_side(Side::New)]
        ));
        let locked = CommentThread {
            can_reply: false,
            ..thread()
        };
        assert!(!same_conversation(&[thread()], &[locked]));
    }

    #[test]
    fn same_conversation_reads_a_missing_path_as_an_empty_one_like_the_typescript() {
        let empty = CommentThread {
            path: Some(String::new()),
            ..thread()
        };
        assert!(same_conversation(&[thread()], &[empty]));
    }
}
