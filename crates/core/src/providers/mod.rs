//! The provider adapters, and dispatch to them by [`ProviderKind`]: a port of
//! src/main/providers/index.ts and the non-pure half of
//! src/main/providers/types.ts. The pure helpers of that file live in
//! [`crate::model`].
//!
//! Everything a provider must do for Reviewdeck to be useful is a free async
//! function here, taking the [`Http`] client and a [`Session`], which matches on the
//! account's kind and calls the adapter's function of the same name. No trait
//! objects: four providers, known at compile time.

use std::future::Future;

use futures::StreamExt;

use crate::error::{Result, msg};
use crate::http::Http;
use crate::model::{
    Account, AccountDraft, CheckSummary, CommentThread, DiffRefs, DraftComment, LineCommentDraft,
    NewAccount, ProviderKind, PullDetail, ReviewItem, ReviewVerdict,
};

pub mod bitbucket;
pub mod forgejo;
pub mod github;
pub mod gitlab;
pub mod submit;
pub mod threads;

/// An account and the token that acts for it.
#[derive(Clone)]
pub struct Session {
    pub account: Account,
    pub token: String,
}

impl std::fmt::Debug for Session {
    /// The token stays out of logs and panic messages.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("account", &self.account)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// Verify the token and resolve the identity behind it.
pub async fn connect(http: &Http, draft: &AccountDraft) -> Result<NewAccount> {
    match draft.kind {
        ProviderKind::Github => github::connect(http, draft).await,
        ProviderKind::Gitlab => gitlab::connect(http, draft).await,
        ProviderKind::Forgejo => forgejo::connect(http, draft).await,
        ProviderKind::Bitbucket => bitbucket::connect(http, draft).await,
    }
}

/// Open PRs/MRs awaiting this user's review.
pub async fn list_review_requests(http: &Http, s: &Session) -> Result<Vec<ReviewItem>> {
    match s.account.kind {
        ProviderKind::Github => github::list_review_requests(http, s).await,
        ProviderKind::Gitlab => gitlab::list_review_requests(http, s).await,
        ProviderKind::Forgejo => forgejo::list_review_requests(http, s).await,
        ProviderKind::Bitbucket => bitbucket::list_review_requests(http, s).await,
    }
}

/// Diff, description and existing comments for one item.
pub async fn load_detail(http: &Http, s: &Session, item: &ReviewItem) -> Result<PullDetail> {
    match s.account.kind {
        ProviderKind::Github => github::load_detail(http, s, item).await,
        ProviderKind::Gitlab => gitlab::load_detail(http, s, item).await,
        ProviderKind::Forgejo => forgejo::load_detail(http, s, item).await,
        ProviderKind::Bitbucket => bitbucket::load_detail(http, s, item).await,
    }
}

/// Re-read just the CI status, for the running-checks poll.
pub async fn refresh_checks(http: &Http, s: &Session, item: &ReviewItem) -> Result<CheckSummary> {
    match s.account.kind {
        ProviderKind::Github => github::refresh_checks(http, s, item).await,
        ProviderKind::Gitlab => gitlab::refresh_checks(http, s, item).await,
        ProviderKind::Forgejo => forgejo::refresh_checks(http, s, item).await,
        ProviderKind::Bitbucket => bitbucket::refresh_checks(http, s, item).await,
    }
}

/// Re-read just the conversation, for the poll behind an open pull request.
///
/// The diff is the expensive half of `load_detail` - a merge request in a monorepo
/// is megabytes of it - and none of it moves when someone answers a thread. So a
/// reply arriving while the diff is being read costs the comments and nothing else.
pub async fn load_threads(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
) -> Result<Vec<CommentThread>> {
    match s.account.kind {
        ProviderKind::Github => github::load_threads(http, s, item).await,
        ProviderKind::Gitlab => gitlab::load_threads(http, s, item).await,
        ProviderKind::Forgejo => forgejo::load_threads(http, s, item).await,
        ProviderKind::Bitbucket => bitbucket::load_threads(http, s, item).await,
    }
}

/// Submit a review. `comments` are the drafts written against this pull request,
/// which each adapter maps onto its host's batch call where one exists and onto
/// sequential posts where it does not.
pub async fn submit_review(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    verdict: ReviewVerdict,
    body: &str,
    comments: &[DraftComment],
) -> Result<()> {
    match s.account.kind {
        ProviderKind::Github => github::submit_review(http, s, item, verdict, body, comments).await,
        ProviderKind::Gitlab => gitlab::submit_review(http, s, item, verdict, body, comments).await,
        ProviderKind::Forgejo => {
            forgejo::submit_review(http, s, item, verdict, body, comments).await
        }
        ProviderKind::Bitbucket => {
            bitbucket::submit_review(http, s, item, verdict, body, comments).await
        }
    }
}

pub async fn add_comment(http: &Http, s: &Session, item: &ReviewItem, body: &str) -> Result<()> {
    match s.account.kind {
        ProviderKind::Github => github::add_comment(http, s, item, body).await,
        ProviderKind::Gitlab => gitlab::add_comment(http, s, item, body).await,
        ProviderKind::Forgejo => forgejo::add_comment(http, s, item, body).await,
        ProviderKind::Bitbucket => bitbucket::add_comment(http, s, item, body).await,
    }
}

pub async fn add_line_comment(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    draft: &LineCommentDraft,
    refs: &DiffRefs,
) -> Result<()> {
    match s.account.kind {
        ProviderKind::Github => github::add_line_comment(http, s, item, draft, refs).await,
        ProviderKind::Gitlab => gitlab::add_line_comment(http, s, item, draft, refs).await,
        ProviderKind::Forgejo => forgejo::add_line_comment(http, s, item, draft, refs).await,
        ProviderKind::Bitbucket => bitbucket::add_line_comment(http, s, item, draft, refs).await,
    }
}

/// Whether the host's adapter can reply into an existing thread at all. Every one
/// can; whether a particular thread takes a reply is its own `can_reply` flag.
pub fn can_reply(kind: ProviderKind) -> bool {
    match kind {
        ProviderKind::Github
        | ProviderKind::Gitlab
        | ProviderKind::Forgejo
        | ProviderKind::Bitbucket => true,
    }
}

/// Whether the host's adapter can resolve or reopen a thread. Forgejo's API has no
/// way to; per thread, `can_resolve` says the same.
pub fn can_resolve(kind: ProviderKind) -> bool {
    match kind {
        ProviderKind::Github | ProviderKind::Gitlab | ProviderKind::Bitbucket => true,
        ProviderKind::Forgejo => false,
    }
}

/// Reply into an existing thread.
pub async fn reply_to_thread(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    thread_id: &str,
    body: &str,
) -> Result<()> {
    match s.account.kind {
        ProviderKind::Github => github::reply_to_thread(http, s, item, thread_id, body).await,
        ProviderKind::Gitlab => gitlab::reply_to_thread(http, s, item, thread_id, body).await,
        ProviderKind::Forgejo => forgejo::reply_to_thread(http, s, item, thread_id, body).await,
        ProviderKind::Bitbucket => bitbucket::reply_to_thread(http, s, item, thread_id, body).await,
    }
}

/// Resolve or reopen a thread.
///
/// The UI only offers this where the thread says it can, so reaching a host without
/// support is a bug rather than something the user did - and it says so in the
/// words of src/main/ipc.ts.
pub async fn set_thread_resolved(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    thread_id: &str,
    resolved: bool,
) -> Result<()> {
    match s.account.kind {
        ProviderKind::Github => {
            github::set_thread_resolved(http, s, item, thread_id, resolved).await
        }
        ProviderKind::Gitlab => {
            gitlab::set_thread_resolved(http, s, item, thread_id, resolved).await
        }
        ProviderKind::Bitbucket => {
            bitbucket::set_thread_resolved(http, s, item, thread_id, resolved).await
        }
        kind @ ProviderKind::Forgejo => Err(msg(format!(
            "{} cannot resolve a thread from here.",
            kind.label()
        ))),
    }
}

/// Run `worker` over `items` with at most `limit` in flight, preserving input order.
///
/// A `limit` of 0 is read as 1: the TypeScript would start no workers at all and
/// hand back an array of holes.
pub async fn limit_concurrency<T, R, F, Fut>(items: Vec<T>, limit: usize, worker: F) -> Vec<R>
where
    F: FnMut(T) -> Fut,
    Fut: Future<Output = R>,
{
    futures::stream::iter(items)
        .map(worker)
        .buffered(limit.max(1))
        .collect()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::cell::Cell;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    /// Pending a few times before it is ready, waking itself each time, so the
    /// workers genuinely interleave.
    struct Yield(u32);

    impl Future for Yield {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 == 0 {
                return Poll::Ready(());
            }
            self.0 -= 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    #[test]
    fn limit_concurrency_preserves_order_and_caps_parallelism() {
        let active = Cell::new(0);
        let peak = Cell::new(0);
        let input: Vec<u32> = (0..12).collect();

        let result = block_on(limit_concurrency(input.clone(), 3, |value| {
            let (active, peak) = (&active, &peak);
            async move {
                active.set(active.get() + 1);
                peak.set(peak.get().max(active.get()));
                // Later items finish sooner, so order has to be restored.
                Yield(12 - value).await;
                active.set(active.get() - 1);
                value * 2
            }
        }));

        assert_eq!(
            result,
            input.iter().map(|value| value * 2).collect::<Vec<_>>()
        );
        assert!(peak.get() <= 3, "peak concurrency was {}", peak.get());
        assert_eq!(peak.get(), 3, "the workers should overlap");
    }

    #[test]
    fn limit_concurrency_handles_an_empty_list() {
        let result: Vec<u32> = block_on(limit_concurrency(Vec::<u32>::new(), 4, |_| async { 1 }));
        assert!(result.is_empty());
    }

    #[test]
    fn capabilities_follow_what_each_adapter_implements() {
        for kind in ProviderKind::ALL {
            assert!(can_reply(kind), "{kind:?}");
        }
        assert!(can_resolve(ProviderKind::Github));
        assert!(can_resolve(ProviderKind::Gitlab));
        assert!(can_resolve(ProviderKind::Bitbucket));
        assert!(!can_resolve(ProviderKind::Forgejo));
    }

    #[test]
    fn a_session_never_prints_its_token() {
        let session = Session {
            account: Account {
                id: "a".into(),
                kind: ProviderKind::Github,
                label: "GitHub".into(),
                base_url: "https://api.github.com".into(),
                web_url: "https://github.com".into(),
                username: "u".into(),
                display_name: "U".into(),
                avatar_url: String::new(),
                added_at: "2026-08-01T10:00:00.000Z".into(),
                agent_command: None,
            },
            token: "ghp_secret".into(),
        };
        assert!(!format!("{session:?}").contains("ghp_secret"));
    }
}
