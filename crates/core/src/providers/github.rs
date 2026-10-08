//! Port of src/main/providers/github.ts.
//!
//! Stubs with the final signatures, so the dispatch in `providers` compiles; the
//! port replaces their bodies.

use crate::error::{Result, msg};
use crate::http::Http;
use crate::model::{
    AccountDraft, CheckSummary, CommentThread, DiffRefs, DraftComment, LineCommentDraft,
    NewAccount, PullDetail, ReviewItem, ReviewVerdict,
};
use crate::providers::Session;

fn not_implemented<T>() -> Result<T> {
    Err(msg("not implemented yet"))
}

pub async fn connect(_http: &Http, _draft: &AccountDraft) -> Result<NewAccount> {
    not_implemented()
}

pub async fn list_review_requests(_http: &Http, _s: &Session) -> Result<Vec<ReviewItem>> {
    not_implemented()
}

pub async fn load_detail(_http: &Http, _s: &Session, _item: &ReviewItem) -> Result<PullDetail> {
    not_implemented()
}

pub async fn refresh_checks(
    _http: &Http,
    _s: &Session,
    _item: &ReviewItem,
) -> Result<CheckSummary> {
    not_implemented()
}

pub async fn load_threads(
    _http: &Http,
    _s: &Session,
    _item: &ReviewItem,
) -> Result<Vec<CommentThread>> {
    not_implemented()
}

pub async fn submit_review(
    _http: &Http,
    _s: &Session,
    _item: &ReviewItem,
    _verdict: ReviewVerdict,
    _body: &str,
    _comments: &[DraftComment],
) -> Result<()> {
    not_implemented()
}

pub async fn add_comment(
    _http: &Http,
    _s: &Session,
    _item: &ReviewItem,
    _body: &str,
) -> Result<()> {
    not_implemented()
}

pub async fn add_line_comment(
    _http: &Http,
    _s: &Session,
    _item: &ReviewItem,
    _draft: &LineCommentDraft,
    _refs: &DiffRefs,
) -> Result<()> {
    not_implemented()
}

pub async fn reply_to_thread(
    _http: &Http,
    _s: &Session,
    _item: &ReviewItem,
    _thread_id: &str,
    _body: &str,
) -> Result<()> {
    not_implemented()
}

pub async fn set_thread_resolved(
    _http: &Http,
    _s: &Session,
    _item: &ReviewItem,
    _thread_id: &str,
    _resolved: bool,
) -> Result<()> {
    not_implemented()
}
