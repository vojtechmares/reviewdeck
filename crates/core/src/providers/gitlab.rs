//! Port of src/main/providers/gitlab.ts.
//!
//! GitLab.com and self-hosted GitLab. `GET /merge_requests?scope=reviews_for_me` does
//! the cross-project aggregation for us, which is the one thing GitLab makes easier
//! than everyone else.
//!
//! As in the GitHub adapter, host JSON is read leniently: optional fields are
//! `Option`s and list entries that do not parse are dropped one at a time.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use crate::diff::count_changes;
use crate::error::Result;
use crate::http::{Http, Method, RequestOptions, to_origin};
use crate::model::{
    AccountDraft, ApprovalOutcome, ApprovalSummary, CheckRun, CheckStatus, CheckSummary,
    CommentThread, DiffFile, DiffRefs, DraftComment, FileStatus, LineCommentDraft, MyReviewState,
    NewAccount, ProviderKind, PullDetail, ReviewItem, ReviewVerdict, User, make_item_id,
    no_approvals, summarise_checks,
};
use crate::providers::Session;
use crate::providers::limit_concurrency;
use crate::providers::submit::{gitlab_discussion_payload, submit_sequentially};
use crate::providers::threads::{GitlabDiscussion, gitlab_threads};

// --- Requests ------------------------------------------------------------------

fn headers(token: &str) -> Vec<(String, String)> {
    vec![
        ("PRIVATE-TOKEN".into(), token.to_string()),
        ("User-Agent".into(), "Reviewdeck".into()),
    ]
}

fn get(token: &str) -> RequestOptions {
    RequestOptions::new(Method::Get).headers(headers(token))
}

fn post(token: &str) -> RequestOptions {
    RequestOptions::new(Method::Post).headers(headers(token))
}

fn put(token: &str) -> RequestOptions {
    RequestOptions::new(Method::Put).headers(headers(token))
}

fn api(session: &Session, path: &str) -> String {
    format!("{}{path}", session.account.base_url)
}

/// The project in a path: GitLab takes the numeric id or the url-encoded path.
fn project(item: &ReviewItem) -> String {
    encode_uri_component(&item.repo_key)
}

/// `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is escaped.
fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Entries of a host list that parse as `T`; the ones that do not are dropped.
fn lenient<T: DeserializeOwned>(entries: Vec<Value>) -> Vec<T> {
    entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect()
}

/// Every page of the conversation. GitLab hands it over a page at a time, and a merge
/// request argued over for a few weeks runs past the first one - so reading only that
/// page drops whole threads and the replies inside them, which looks from here exactly
/// like nobody having written them.
///
/// A failure reads as no conversation at all, as the TypeScript does.
async fn load_discussions(http: &Http, s: &Session, item: &ReviewItem) -> Vec<GitlabDiscussion> {
    let url = api(
        s,
        &format!(
            "/projects/{}/merge_requests/{}/discussions?per_page=100",
            project(item),
            item.number
        ),
    );
    http.paginate::<Value>(&url, get(&s.token), 5)
        .await
        .map(lenient)
        .unwrap_or_default()
}

/// The merge request's versions, newest first. A failure reads as none.
async fn load_versions(http: &Http, s: &Session, item: &ReviewItem) -> Vec<GlVersion> {
    let url = api(
        s,
        &format!(
            "/projects/{}/merge_requests/{}/versions",
            project(item),
            item.number
        ),
    );
    http.json::<Vec<Value>>(&url, get(&s.token))
        .await
        .map(lenient)
        .unwrap_or_default()
}

// --- Host shapes ------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlUser {
    username: Option<String>,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlReferences {
    full: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlMergeRequest {
    iid: Option<u64>,
    project_id: Option<u64>,
    title: Option<String>,
    description: Option<String>,
    draft: Option<bool>,
    work_in_progress: Option<bool>,
    created_at: Option<String>,
    updated_at: Option<String>,
    author: Option<GlUser>,
    source_branch: Option<String>,
    target_branch: Option<String>,
    labels: Option<Vec<String>>,
    web_url: Option<String>,
    sha: Option<String>,
    references: Option<GlReferences>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlDiff {
    old_path: Option<String>,
    new_path: Option<String>,
    new_file: Option<bool>,
    renamed_file: Option<bool>,
    deleted_file: Option<bool>,
    diff: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlChanges {
    diffs: Option<Vec<GlDiff>>,
    changes: Option<Vec<GlDiff>>,
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
struct GlVersion {
    head_commit_sha: Option<String>,
    base_commit_sha: Option<String>,
    start_commit_sha: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlPipeline {
    id: u64,
    status: Option<String>,
    web_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlJob {
    id: u64,
    name: Option<String>,
    status: Option<String>,
    web_url: Option<String>,
    stage: Option<String>,
    allow_failure: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlApprover {
    user: Option<GlUser>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GlApproval {
    approved_by: Option<Vec<GlApprover>>,
    approvals_required: Option<u32>,
    /// Kept raw: GitLab's rule is `approvals_left !== undefined ? === 0 : approved`, so
    /// an explicit null counts as present (see [`present`]).
    #[serde(default, deserialize_with = "present")]
    approvals_left: Option<Value>,
    approved: Option<bool>,
}

/// Reads a field that is present even when it is `null`: an `Option<Value>` on its own
/// turns `null` into `None`, which would make a missing field and a null one the same.
fn present<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

// --- Mapping ---------------------------------------------------------------------

fn pipeline_status(status: Option<&str>) -> CheckStatus {
    match status {
        Some("success") => CheckStatus::Passed,
        Some("failed") => CheckStatus::Failed,
        Some(
            "running" | "pending" | "created" | "waiting_for_resource" | "preparing" | "scheduled",
        ) => CheckStatus::Running,
        Some("canceled" | "canceling") => CheckStatus::Failed,
        _ => CheckStatus::Unknown,
    }
}

/// `gitlab-org/gitlab!123` -> `gitlab-org/gitlab`, falling back to the numeric id.
fn path_from_reference(mr: &GlMergeRequest, project_id: u64) -> String {
    if let Some(full) = mr
        .references
        .as_ref()
        .and_then(|references| references.full.as_deref())
        && let Some(at) = full.find('!')
        && at > 0
    {
        return full[..at].to_string();
    }
    project_id.to_string()
}

fn empty_checks() -> CheckSummary {
    summarise_checks(Vec::new())
}

/// The checks on a merge request, from its latest pipeline. Every read here is
/// best-effort: a hidden pipeline or job list reads as no checks, never as a failure.
async fn load_checks(http: &Http, s: &Session, project_id: &str, iid: u64) -> CheckSummary {
    let pipelines_url = api(
        s,
        &format!(
            "/projects/{}/merge_requests/{iid}/pipelines",
            encode_uri_component(project_id)
        ),
    );
    let pipelines: Vec<GlPipeline> = http
        .json::<Vec<Value>>(&pipelines_url, get(&s.token))
        .await
        .map(lenient)
        .unwrap_or_default();
    let Some(latest) = pipelines.first() else {
        return empty_checks();
    };

    // Jobs give the per-check breakdown; if they are hidden, fall back to the pipeline.
    let jobs_url = api(
        s,
        &format!(
            "/projects/{}/pipelines/{}/jobs?per_page=100",
            encode_uri_component(project_id),
            latest.id
        ),
    );
    let jobs: Vec<GlJob> = http
        .json::<Vec<Value>>(&jobs_url, get(&s.token))
        .await
        .map(lenient)
        .unwrap_or_default();

    if jobs.is_empty() {
        let run = CheckRun {
            id: format!("pipeline-{}", latest.id),
            name: format!("Pipeline #{}", latest.id),
            status: pipeline_status(latest.status.as_deref()),
            url: latest.web_url.clone(),
            description: None,
        };
        return summarise_checks(vec![run]);
    }

    let runs = jobs
        .into_iter()
        .map(|job| {
            let status = pipeline_status(job.status.as_deref());
            CheckRun {
                id: format!("job-{}", job.id),
                name: job.name.unwrap_or_default(),
                // An allow_failure job that failed should not paint the whole MR red.
                status: if job.status.as_deref() == Some("failed")
                    && job.allow_failure == Some(true)
                {
                    CheckStatus::Unknown
                } else {
                    status
                },
                url: job.web_url,
                description: job.stage,
            }
        })
        .collect();
    summarise_checks(runs)
}

/// The user's own verdict and the merge request's standing, both off one call.
///
/// GitLab settles its own approval rules - a rule naming who may approve, code owners,
/// the lot - so `approvals_left` is its verdict and the count is only what the card
/// shows beside it. A required count of zero is a project that asks for none, and the
/// call failing is read the same way: approvals are a paid feature on some tiers, and
/// a missing feature is not a review to hide.
async fn load_approvals(
    http: &Http,
    s: &Session,
    project_id: &str,
    iid: u64,
) -> (MyReviewState, ApprovalSummary) {
    let url = api(
        s,
        &format!(
            "/projects/{}/merge_requests/{iid}/approvals",
            encode_uri_component(project_id)
        ),
    );
    let Ok(result) = http.json::<GlApproval>(&url, get(&s.token)).await else {
        return (MyReviewState::Pending, no_approvals());
    };

    let approved_by = result.approved_by.unwrap_or_default();
    let mine = approved_by.iter().any(|entry| {
        entry
            .user
            .as_ref()
            .and_then(|user| user.username.as_deref())
            == Some(s.account.username.as_str())
    });
    let required = result.approvals_required.unwrap_or(0);
    let given = approved_by.len() as u32;
    let settled = match &result.approvals_left {
        Some(left) => left.as_f64() == Some(0.0),
        None => result.approved.unwrap_or(false),
    };
    let approvals = if required > 0 {
        ApprovalSummary {
            given,
            required: Some(required),
            outcome: if settled {
                ApprovalOutcome::Satisfied
            } else {
                ApprovalOutcome::Pending
            },
        }
    } else {
        ApprovalSummary {
            given,
            required: Some(0),
            outcome: ApprovalOutcome::NoneRequired,
        }
    };
    let review_state = if mine {
        MyReviewState::Approved
    } else {
        MyReviewState::Pending
    };
    (review_state, approvals)
}

/// One merge request as it appears in the deck.
async fn review_item(http: &Http, s: &Session, mr: GlMergeRequest) -> ReviewItem {
    let project_id = mr.project_id.unwrap_or_default().to_string();
    let iid = mr.iid.unwrap_or_default();
    let project_path = path_from_reference(&mr, mr.project_id.unwrap_or_default());
    let (checks, (review_state, approvals)) = futures::join!(
        load_checks(http, s, &project_id, iid),
        load_approvals(http, s, &project_id, iid),
    );

    let author = mr.author.unwrap_or_default();
    ReviewItem {
        id: make_item_id(&s.account.id, &project_id, iid),
        account_id: s.account.id.clone(),
        provider: ProviderKind::Gitlab,
        // API calls use the numeric id; the path is only for display.
        repo_key: project_id,
        repo: project_path,
        number: iid,
        title: mr.title.unwrap_or_default(),
        url: mr.web_url.unwrap_or_default(),
        author: User {
            name: author.username.unwrap_or_default(),
            avatar_url: author.avatar_url.unwrap_or_default(),
        },
        created_at: mr.created_at.unwrap_or_default(),
        updated_at: mr.updated_at.unwrap_or_default(),
        draft: mr.draft.or(mr.work_in_progress).unwrap_or(false),
        source_branch: mr.source_branch.unwrap_or_default(),
        target_branch: mr.target_branch.unwrap_or_default(),
        labels: mr.labels.unwrap_or_default(),
        my_review_state: review_state,
        approvals,
        checks,
        additions: None,
        deletions: None,
        changed_files: None,
    }
}

fn diff_file(diff: GlDiff) -> DiffFile {
    let old_path = diff.old_path.unwrap_or_default();
    let new_path = diff.new_path.unwrap_or_default();
    let counted = count_changes(diff.diff.as_deref().unwrap_or(""));
    let status = if diff.new_file == Some(true) {
        FileStatus::Added
    } else if diff.deleted_file == Some(true) {
        FileStatus::Removed
    } else if diff.renamed_file == Some(true) {
        FileStatus::Renamed
    } else {
        FileStatus::Modified
    };
    // GitLab omits the `@@` header on empty diffs and on binaries.
    let patch = diff.diff.filter(|patch| !patch.is_empty());
    DiffFile {
        path: if new_path.is_empty() {
            old_path.clone()
        } else {
            new_path.clone()
        },
        old_path: if old_path.is_empty() {
            new_path
        } else {
            old_path
        },
        status,
        additions: counted.additions,
        deletions: counted.deletions,
        binary: patch.is_none(),
        patch,
    }
}

/// One comment, placed by the three shas the draft recorded.
async fn post_discussion(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    comment: &DraftComment,
) -> Result<()> {
    let payload = gitlab_discussion_payload(comment)?;
    http.request(
        &api(
            s,
            &format!(
                "/projects/{}/merge_requests/{}/discussions",
                project(item),
                item.number
            ),
        ),
        post(&s.token).form(payload),
    )
    .await
    .map(|_| ())
}

// --- The provider ----------------------------------------------------------------

pub async fn connect(http: &Http, draft: &AccountDraft) -> Result<NewAccount> {
    let host = if draft.host.is_empty() {
        "gitlab.com"
    } else {
        draft.host.as_str()
    };
    let origin = to_origin(host)?;
    let base_url = format!("{origin}/api/v4");
    let user: GlUser = http
        .json(&format!("{base_url}/user"), get(&draft.token))
        .await?;
    let username = user.username.unwrap_or_default();
    let display_name = user
        .name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| username.clone());
    Ok(NewAccount {
        kind: ProviderKind::Gitlab,
        label: if draft.label.is_empty() {
            format!("GitLab ({username})")
        } else {
            draft.label.clone()
        },
        base_url,
        web_url: origin,
        username,
        display_name,
        avatar_url: user.avatar_url.unwrap_or_default(),
        agent_command: None,
    })
}

pub async fn list_review_requests(http: &Http, s: &Session) -> Result<Vec<ReviewItem>> {
    let merges: Vec<Value> = http
        .json(
            &api(
                s,
                "/merge_requests?scope=reviews_for_me&state=opened&per_page=50&order_by=updated_at",
            ),
            get(&s.token),
        )
        .await?;
    // An entry without its project or its iid cannot be addressed, so it is skipped.
    let merges: Vec<GlMergeRequest> = lenient(merges);
    let merges: Vec<GlMergeRequest> = merges
        .into_iter()
        .filter(|mr| mr.project_id.is_some() && mr.iid.is_some())
        .collect();

    Ok(limit_concurrency(merges, 5, move |mr| async move {
        review_item(http, s, mr).await
    })
    .await)
}

pub async fn load_detail(http: &Http, s: &Session, item: &ReviewItem) -> Result<PullDetail> {
    let mr_path = format!("/projects/{}/merge_requests/{}", project(item), item.number);
    let mr_url = api(s, &mr_path);
    let changes_url = api(s, &format!("{mr_path}/changes"));
    let (mr, changes, versions, discussions) = futures::join!(
        http.json::<GlMergeRequest>(&mr_url, get(&s.token)),
        http.json::<GlChanges>(&changes_url, get(&s.token)),
        load_versions(http, s, item),
        load_discussions(http, s, item),
    );
    let mr = mr?;
    let changes = changes?;

    let raw_diffs = changes.changes.or(changes.diffs).unwrap_or_default();
    let files: Vec<DiffFile> = raw_diffs.into_iter().map(diff_file).collect();

    let version = versions.first();
    let threads = gitlab_threads(
        &discussions,
        version
            .and_then(|version| version.head_commit_sha.as_deref())
            .or(mr.sha.as_deref()),
    );

    let (additions, deletions) = files.iter().fold((0, 0), |(a, d), file| {
        (a + file.additions, d + file.deletions)
    });

    let refs = match version {
        Some(version) => DiffRefs {
            base_sha: version.base_commit_sha.clone(),
            start_sha: version.start_commit_sha.clone(),
            head_sha: version.head_commit_sha.clone(),
        },
        None => DiffRefs {
            head_sha: mr.sha.clone(),
            ..DiffRefs::default()
        },
    };

    Ok(PullDetail {
        item: ReviewItem {
            additions: Some(additions),
            deletions: Some(deletions),
            changed_files: Some(files.len() as u32),
            ..item.clone()
        },
        description: mr.description.unwrap_or_default(),
        files,
        threads,
        refs,
    })
}

pub async fn refresh_checks(http: &Http, s: &Session, item: &ReviewItem) -> Result<CheckSummary> {
    Ok(load_checks(http, s, &item.repo_key, item.number).await)
}

pub async fn load_threads(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
) -> Result<Vec<CommentThread>> {
    // The versions come along because a thread left against an older diff is only
    // recognisable next to the head it was written on.
    let (versions, discussions) = futures::join!(
        load_versions(http, s, item),
        load_discussions(http, s, item),
    );
    Ok(gitlab_threads(
        &discussions,
        versions
            .first()
            .and_then(|version| version.head_commit_sha.as_deref()),
    ))
}

pub async fn submit_review(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    verdict: ReviewVerdict,
    body: &str,
    comments: &[DraftComment],
) -> Result<()> {
    // No call here takes a review and its comments together, and no server-side draft
    // mechanism is used, so the comments go one at a time and the verdict last -
    // reported honestly if it stops part-way.
    submit_sequentially(
        comments,
        move |comment| post_discussion(http, s, item, comment),
        move || async move {
            // GitLab has no single "submit review" call: approval and the note are separate.
            match verdict {
                ReviewVerdict::Approve => {
                    http.request(
                        &api(
                            s,
                            &format!(
                                "/projects/{}/merge_requests/{}/approve",
                                project(item),
                                item.number
                            ),
                        ),
                        post(&s.token),
                    )
                    .await?;
                    if !body.is_empty() {
                        add_comment(http, s, item, body).await?;
                    }
                    Ok(())
                }
                ReviewVerdict::RequestChanges => {
                    // Closest equivalent: drop any approval and say why. A failure to
                    // unapprove (nothing to drop, say) is not worth stopping for.
                    let _ = http
                        .request(
                            &api(
                                s,
                                &format!(
                                    "/projects/{}/merge_requests/{}/unapprove",
                                    project(item),
                                    item.number
                                ),
                            ),
                            post(&s.token),
                        )
                        .await;
                    let note = if body.is_empty() {
                        "Changes requested."
                    } else {
                        body
                    };
                    add_comment(http, s, item, note).await
                }
                ReviewVerdict::Comment => {
                    if !body.is_empty() {
                        add_comment(http, s, item, body).await?;
                    }
                    Ok(())
                }
            }
        },
    )
    .await
}

pub async fn add_comment(http: &Http, s: &Session, item: &ReviewItem, body: &str) -> Result<()> {
    http.request(
        &api(
            s,
            &format!(
                "/projects/{}/merge_requests/{}/notes",
                project(item),
                item.number
            ),
        ),
        post(&s.token).json(json!({ "body": body })),
    )
    .await
    .map(|_| ())
}

pub async fn add_line_comment(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    draft: &LineCommentDraft,
    refs: &DiffRefs,
) -> Result<()> {
    let comment = DraftComment {
        id: String::new(),
        item_id: item.id.clone(),
        body: draft.body.clone(),
        path: draft.path.clone(),
        new_line: draft.new_line,
        old_line: draft.old_line,
        range: draft.range,
        created_at: String::new(),
        refs: refs.clone(),
    };
    post_discussion(http, s, item, &comment).await
}

pub async fn reply_to_thread(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    thread_id: &str,
    body: &str,
) -> Result<()> {
    http.request(
        &api(
            s,
            &format!(
                "/projects/{}/merge_requests/{}/discussions/{}/notes",
                project(item),
                item.number,
                encode_uri_component(thread_id)
            ),
        ),
        post(&s.token).json(json!({ "body": body })),
    )
    .await
    .map(|_| ())
}

pub async fn set_thread_resolved(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    thread_id: &str,
    resolved: bool,
) -> Result<()> {
    http.request(
        &api(
            s,
            &format!(
                "/projects/{}/merge_requests/{}/discussions/{}",
                project(item),
                item.number,
                encode_uri_component(thread_id)
            ),
        ),
        put(&s.token).json(json!({ "resolved": resolved })),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{MockRequest, MockResponse};
    use crate::model::{Account, EdgeKind, LineRange, RangeEdge};
    use futures::executor::block_on;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const BASE: &str = "https://gitlab.com/api/v4";
    const PROJECT: &str = "42";
    const MR: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7";

    #[derive(Debug, Clone)]
    struct Call {
        key: String,
        body: Option<String>,
    }

    type Calls = Arc<Mutex<Vec<Call>>>;

    fn verb(method: Method) -> &'static str {
        match method {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }

    /// A mock host keyed by `"VERB url"`; anything else answers 599 so a stray request
    /// fails the test.
    fn serve(routes: Vec<(String, MockResponse)>) -> (Http, Calls) {
        let calls: Calls = Arc::default();
        let log = Arc::clone(&calls);
        let http = Http::mock(move |request: &MockRequest| {
            let key = format!("{} {}", verb(request.method), request.url);
            log.lock().push(Call {
                key: key.clone(),
                body: request.body.clone(),
            });
            routes
                .iter()
                .find(|(route, _)| *route == key)
                .map(|(_, response)| response.clone())
                .unwrap_or_else(|| MockResponse::new(599, format!("unexpected request {key}")))
        });
        (http, calls)
    }

    fn keys(calls: &Calls) -> Vec<String> {
        calls.lock().iter().map(|call| call.key.clone()).collect()
    }

    fn body_of(calls: &Calls, key: &str) -> Value {
        let calls = calls.lock();
        let call = calls
            .iter()
            .find(|call| call.key == key)
            .unwrap_or_else(|| panic!("no request {key}; saw {calls:?}"));
        serde_json::from_str(call.body.as_deref().unwrap_or("null")).unwrap_or(Value::Null)
    }

    /// The raw body of the one request sent to `key`, for form posts.
    fn raw_body_of(calls: &Calls, key: &str) -> String {
        let calls = calls.lock();
        calls
            .iter()
            .find(|call| call.key == key)
            .and_then(|call| call.body.clone())
            .unwrap_or_default()
    }

    fn ok(value: Value) -> MockResponse {
        MockResponse::json(200, &value)
    }

    fn session() -> Session {
        Session {
            account: Account {
                id: "acct-2".into(),
                kind: ProviderKind::Gitlab,
                label: "GitLab".into(),
                base_url: BASE.into(),
                web_url: "https://gitlab.com".into(),
                username: "octocat".into(),
                display_name: "Octo Cat".into(),
                avatar_url: String::new(),
                added_at: "2026-08-01T10:00:00.000Z".into(),
                agent_command: None,
            },
            token: "glpat-test".into(),
        }
    }

    fn item() -> ReviewItem {
        ReviewItem {
            id: "acct-2:42:7".into(),
            account_id: "acct-2".into(),
            provider: ProviderKind::Gitlab,
            repo_key: PROJECT.into(),
            repo: "gitlab-org/widgets".into(),
            number: 7,
            title: "Add retry".into(),
            url: "https://gitlab.com/gitlab-org/widgets/-/merge_requests/7".into(),
            author: User {
                name: "bob".into(),
                avatar_url: String::new(),
            },
            created_at: String::new(),
            updated_at: String::new(),
            draft: false,
            source_branch: "retry".into(),
            target_branch: "main".into(),
            labels: Vec::new(),
            my_review_state: MyReviewState::Pending,
            approvals: no_approvals(),
            checks: empty_checks(),
            additions: None,
            deletions: None,
            changed_files: None,
        }
    }

    fn mr_json() -> Value {
        json!({
            "id": 900,
            "iid": 7,
            "project_id": 42,
            "title": "Add retry to the sync loop",
            "description": "Retries three times.",
            "state": "opened",
            "draft": false,
            "work_in_progress": false,
            "created_at": "2026-08-01T09:00:00.000Z",
            "updated_at": "2026-08-03T12:00:00.000Z",
            "author": {"id": 3, "username": "bob", "name": "Bob", "avatar_url": "https://gitlab.example/uploads/bob.png", "web_url": "https://gitlab.com/bob"},
            "reviewers": [],
            "source_branch": "retry",
            "target_branch": "main",
            "labels": ["backend", "sync"],
            "web_url": "https://gitlab.com/gitlab-org/widgets/-/merge_requests/7",
            "sha": "head111",
            "references": {"short": "!7", "relative": "!7", "full": "gitlab-org/widgets!7"},
            "changes_count": "3"
        })
    }

    fn versions_json() -> Value {
        json!([{
            "id": 55,
            "head_commit_sha": "head222",
            "base_commit_sha": "base333",
            "start_commit_sha": "start444",
            "created_at": "2026-08-03T10:00:00.000Z",
            "merge_request_id": 900,
            "state": "collected"
        }])
    }

    fn pipelines_json() -> Value {
        json!([{"id": 5001, "iid": 9, "project_id": 42, "status": "running", "source": "push", "ref": "retry", "sha": "head222", "web_url": "https://gitlab.com/gitlab-org/widgets/-/pipelines/5001"}])
    }

    fn jobs_json() -> Value {
        json!([
            {"id": 7001, "name": "unit", "status": "success", "stage": "test", "allow_failure": false, "web_url": "https://gitlab.com/gitlab-org/widgets/-/jobs/7001"},
            {"id": 7002, "name": "lint", "status": "running", "stage": "test", "allow_failure": false, "web_url": "https://gitlab.com/gitlab-org/widgets/-/jobs/7002"},
            {"id": 7003, "name": "experiment", "status": "failed", "stage": "test", "allow_failure": true, "web_url": "https://gitlab.com/gitlab-org/widgets/-/jobs/7003"}
        ])
    }

    const APPROVALS_URL: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7/approvals";
    const LIST_URL: &str = "https://gitlab.com/api/v4/merge_requests?scope=reviews_for_me&state=opened&per_page=50&order_by=updated_at";
    const PIPELINES_URL: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7/pipelines";
    const JOBS_URL: &str = "https://gitlab.com/api/v4/projects/42/pipelines/5001/jobs?per_page=100";
    const VERSIONS_URL: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7/versions";
    const DISCUSSIONS_URL: &str =
        "https://gitlab.com/api/v4/projects/42/merge_requests/7/discussions?per_page=100";
    const CHANGES_URL: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7/changes";
    const DISCUSSIONS_POST: &str =
        "https://gitlab.com/api/v4/projects/42/merge_requests/7/discussions";
    const NOTES_URL: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7/notes";
    const APPROVE_URL: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7/approve";
    const UNAPPROVE_URL: &str = "https://gitlab.com/api/v4/projects/42/merge_requests/7/unapprove";

    fn get_key(url: &str) -> String {
        format!("GET {url}")
    }

    fn post_key(url: &str) -> String {
        format!("POST {url}")
    }

    /// Routes for a full merge request list entry: pipelines, jobs and approvals.
    fn standing_routes(approvals: Value) -> Vec<(String, MockResponse)> {
        vec![
            (get_key(PIPELINES_URL), ok(pipelines_json())),
            (get_key(JOBS_URL), ok(jobs_json())),
            (get_key(APPROVALS_URL), ok(approvals)),
        ]
    }

    fn list_routes(approvals: Value) -> Vec<(String, MockResponse)> {
        let mut routes = vec![(get_key(LIST_URL), ok(json!([mr_json()])))];
        routes.extend(standing_routes(approvals));
        routes
    }

    // --- Connect -----------------------------------------------------------------

    #[test]
    fn connect_resolves_the_identity_and_the_api_root() {
        let (http, calls) = serve(vec![(
            get_key(&format!("{BASE}/user")),
            ok(
                json!({"id": 3, "username": "octocat", "name": "Octo Cat", "avatar_url": "https://gitlab.com/uploads/octo.png", "web_url": "https://gitlab.com/octocat"}),
            ),
        )]);
        let draft = AccountDraft {
            kind: ProviderKind::Gitlab,
            label: String::new(),
            host: String::new(),
            token: "glpat-test".into(),
            username: None,
            agent_command: None,
        };
        let account = block_on(connect(&http, &draft)).expect("connects");
        assert_eq!(account.kind, ProviderKind::Gitlab);
        assert_eq!(account.label, "GitLab (octocat)");
        assert_eq!(account.base_url, BASE);
        assert_eq!(account.web_url, "https://gitlab.com");
        assert_eq!(account.username, "octocat");
        assert_eq!(account.display_name, "Octo Cat");
        assert_eq!(account.avatar_url, "https://gitlab.com/uploads/octo.png");
        assert_eq!(keys(&calls), vec![get_key(&format!("{BASE}/user"))]);
    }

    #[test]
    fn connect_uses_the_private_token_header_and_the_host_as_typed() {
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let log = Arc::clone(&seen);
        let http = Http::mock(move |request: &MockRequest| {
            log.lock().push((
                request
                    .header("private-token")
                    .unwrap_or_default()
                    .to_string(),
                request.url.clone(),
            ));
            ok(json!({"username": "carol", "name": ""}))
        });
        let draft = AccountDraft {
            kind: ProviderKind::Gitlab,
            label: "Work".into(),
            host: "gitlab.example.com/".into(),
            token: "glpat-x".into(),
            username: None,
            agent_command: None,
        };
        let account = block_on(connect(&http, &draft)).expect("connects");
        assert_eq!(account.label, "Work");
        assert_eq!(account.base_url, "https://gitlab.example.com/api/v4");
        assert_eq!(account.web_url, "https://gitlab.example.com");
        // An empty name falls back to the username, and a missing avatar to "".
        assert_eq!(account.display_name, "carol");
        assert_eq!(account.avatar_url, "");
        assert_eq!(
            seen.lock().first().cloned(),
            Some((
                "glpat-x".to_string(),
                "https://gitlab.example.com/api/v4/user".to_string()
            ))
        );
    }

    #[test]
    fn connect_keeps_a_path_on_a_self_hosted_instance() {
        let (http, calls) = serve(vec![(
            get_key("https://code.acme.dev/gitlab/api/v4/user"),
            ok(json!({"username": "dan"})),
        )]);
        let draft = AccountDraft {
            kind: ProviderKind::Gitlab,
            label: String::new(),
            host: "https://code.acme.dev/gitlab".into(),
            token: "t".into(),
            username: None,
            agent_command: None,
        };
        let account = block_on(connect(&http, &draft)).expect("connects");
        assert_eq!(account.base_url, "https://code.acme.dev/gitlab/api/v4");
        assert_eq!(account.web_url, "https://code.acme.dev/gitlab");
        assert_eq!(account.label, "GitLab (dan)");
        assert_eq!(keys(&calls).len(), 1);
    }

    #[test]
    fn connect_reports_a_rejected_token() {
        let (http, _) = serve(vec![(
            get_key(&format!("{BASE}/user")),
            MockResponse::json(401, &json!({"message": "401 Unauthorized"})),
        )]);
        let draft = AccountDraft {
            kind: ProviderKind::Gitlab,
            label: String::new(),
            host: String::new(),
            token: "bad".into(),
            username: None,
            agent_command: None,
        };
        assert_eq!(
            block_on(connect(&http, &draft))
                .expect_err("rejected")
                .to_string(),
            "Not authorised on gitlab.com - the token is invalid or expired."
        );
    }

    // --- Listing -----------------------------------------------------------------

    #[test]
    fn list_review_requests_builds_every_field_of_an_item() {
        let (http, calls) = serve(list_routes(json!({
            "approved_by": [{"user": {"username": "alice"}}],
            "approvals_required": 2,
            "approvals_left": 1,
            "approved": false
        })));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items.len(), 1);
        let item = &items[0];

        assert_eq!(item.id, "acct-2:42:7");
        assert_eq!(item.account_id, "acct-2");
        assert_eq!(item.provider, ProviderKind::Gitlab);
        // The API id travels as repoKey; the path is for display.
        assert_eq!(item.repo_key, "42");
        assert_eq!(item.repo, "gitlab-org/widgets");
        assert_eq!(item.number, 7);
        assert_eq!(item.title, "Add retry to the sync loop");
        assert_eq!(
            item.url,
            "https://gitlab.com/gitlab-org/widgets/-/merge_requests/7"
        );
        assert_eq!(
            item.author,
            User {
                name: "bob".into(),
                avatar_url: "https://gitlab.example/uploads/bob.png".into(),
            }
        );
        assert_eq!(item.created_at, "2026-08-01T09:00:00.000Z");
        assert_eq!(item.updated_at, "2026-08-03T12:00:00.000Z");
        assert!(!item.draft);
        assert_eq!(item.source_branch, "retry");
        assert_eq!(item.target_branch, "main");
        assert_eq!(item.labels, vec!["backend".to_string(), "sync".to_string()]);
        assert_eq!(item.my_review_state, MyReviewState::Pending);
        assert_eq!(
            item.approvals,
            ApprovalSummary {
                given: 1,
                required: Some(2),
                outcome: ApprovalOutcome::Pending
            }
        );
        // The list does not carry the diff totals; they come with the detail load.
        assert_eq!(item.additions, None);
        assert_eq!(item.deletions, None);
        assert_eq!(item.changed_files, None);

        // The pipeline's jobs: the allow-failure job that failed reads as unknown.
        assert_eq!(item.checks.status, CheckStatus::Running);
        let runs: Vec<(&str, CheckStatus)> = item
            .checks
            .runs
            .iter()
            .map(|run| (run.id.as_str(), run.status))
            .collect();
        assert_eq!(
            runs,
            vec![
                ("job-7001", CheckStatus::Passed),
                ("job-7002", CheckStatus::Running),
                ("job-7003", CheckStatus::Unknown),
            ]
        );
        assert_eq!(item.checks.runs[0].name, "unit");
        assert_eq!(item.checks.runs[0].description.as_deref(), Some("test"));
        assert_eq!(
            item.checks.runs[1].url.as_deref(),
            Some("https://gitlab.com/gitlab-org/widgets/-/jobs/7002")
        );
        assert_eq!(item.checks.total, 3);
        assert_eq!(item.checks.passed, 1);
        assert_eq!(item.checks.running, 1);
        assert_eq!(item.checks.failed, 0);

        let sent = keys(&calls);
        assert!(sent.contains(&get_key(LIST_URL)), "{sent:?}");
        assert!(sent.contains(&get_key(APPROVALS_URL)), "{sent:?}");
    }

    #[test]
    fn list_review_requests_marks_the_reviewer_who_already_approved() {
        let (http, _) = serve(list_routes(json!({
            "approved_by": [{"user": {"username": "octocat"}}],
            "approvals_required": 1,
            "approvals_left": 0
        })));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].my_review_state, MyReviewState::Approved);
        assert_eq!(
            items[0].approvals,
            ApprovalSummary {
                given: 1,
                required: Some(1),
                outcome: ApprovalOutcome::Satisfied
            }
        );
    }

    #[test]
    fn a_project_that_requires_no_approvals_reads_as_none_required_with_zero() {
        let (http, _) = serve(list_routes(
            json!({"approved_by": [], "approvals_required": 0}),
        ));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(
            items[0].approvals,
            ApprovalSummary {
                given: 0,
                required: Some(0),
                outcome: ApprovalOutcome::NoneRequired
            }
        );
    }

    #[test]
    fn approval_settled_falls_back_to_the_approved_flag_when_left_is_absent() {
        // No approvals_left: the approved flag decides.
        let (http, _) = serve(list_routes(
            json!({"approved_by": [], "approvals_required": 1, "approved": true}),
        ));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].approvals.outcome, ApprovalOutcome::Satisfied);

        // An explicit null is present, not absent, so it is not settled.
        let (http, _) = serve(list_routes(
            json!({"approved_by": [], "approvals_required": 1, "approvals_left": null, "approved": true}),
        ));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].approvals.outcome, ApprovalOutcome::Pending);
    }

    #[test]
    fn a_refused_approvals_read_leaves_the_item_pending_with_no_approvals() {
        let (http, _) = serve(vec![
            (get_key(LIST_URL), ok(json!([mr_json()]))),
            (get_key(PIPELINES_URL), ok(pipelines_json())),
            (get_key(JOBS_URL), ok(jobs_json())),
            (
                get_key(APPROVALS_URL),
                MockResponse::json(403, &json!({"message": "403 Forbidden"})),
            ),
        ]);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].my_review_state, MyReviewState::Pending);
        assert_eq!(items[0].approvals, no_approvals());
    }

    #[test]
    fn hidden_pipelines_read_as_no_checks_and_a_pipeline_without_jobs_stands_alone() {
        let mut routes = vec![(get_key(LIST_URL), ok(json!([mr_json()])))];
        routes.push((
            get_key(PIPELINES_URL),
            MockResponse::json(403, &json!({"message": "403 Forbidden"})),
        ));
        routes.push((get_key(APPROVALS_URL), ok(json!({"approved_by": []}))));
        let (http, _) = serve(routes);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].checks, empty_checks());

        // Pipelines are visible, the jobs list is hidden: the pipeline itself is the check.
        let (http, _) = serve(vec![
            (get_key(LIST_URL), ok(json!([mr_json()]))),
            (get_key(PIPELINES_URL), ok(pipelines_json())),
            (
                get_key(JOBS_URL),
                MockResponse::json(403, &json!({"message": "403 Forbidden"})),
            ),
            (get_key(APPROVALS_URL), ok(json!({"approved_by": []}))),
        ]);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        let checks = &items[0].checks;
        assert_eq!(checks.total, 1);
        assert_eq!(checks.status, CheckStatus::Running);
        assert_eq!(checks.runs[0].id, "pipeline-5001");
        assert_eq!(checks.runs[0].name, "Pipeline #5001");
        assert_eq!(
            checks.runs[0].url.as_deref(),
            Some("https://gitlab.com/gitlab-org/widgets/-/pipelines/5001")
        );
    }

    #[test]
    fn pipeline_status_maps_each_gitlab_state() {
        assert_eq!(pipeline_status(Some("success")), CheckStatus::Passed);
        assert_eq!(pipeline_status(Some("failed")), CheckStatus::Failed);
        assert_eq!(pipeline_status(Some("canceled")), CheckStatus::Failed);
        assert_eq!(pipeline_status(Some("canceling")), CheckStatus::Failed);
        for status in [
            "running",
            "pending",
            "created",
            "waiting_for_resource",
            "preparing",
            "scheduled",
        ] {
            assert_eq!(
                pipeline_status(Some(status)),
                CheckStatus::Running,
                "{status}"
            );
        }
        for status in ["skipped", "manual", "something-new"] {
            assert_eq!(
                pipeline_status(Some(status)),
                CheckStatus::Unknown,
                "{status}"
            );
        }
        assert_eq!(pipeline_status(None), CheckStatus::Unknown);
    }

    #[test]
    fn a_merge_request_without_a_reference_is_named_by_its_project_id() {
        let mut mr = mr_json();
        mr["references"] = Value::Null;
        let (http, _) = serve(vec![
            (get_key(LIST_URL), ok(json!([mr]))),
            (get_key(PIPELINES_URL), ok(json!([]))),
            (get_key(APPROVALS_URL), ok(json!({"approved_by": []}))),
        ]);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].repo, "42");
        assert_eq!(items[0].checks, empty_checks());
    }

    #[test]
    fn draft_falls_back_to_work_in_progress_and_entries_without_ids_are_skipped() {
        let mut wip = mr_json();
        wip["draft"] = Value::Null;
        wip["work_in_progress"] = json!(true);
        let mut without_iid = mr_json();
        if let Some(object) = without_iid.as_object_mut() {
            object.remove("iid");
        }
        let (http, _) = serve(vec![
            (get_key(LIST_URL), ok(json!([wip, without_iid]))),
            (get_key(PIPELINES_URL), ok(json!([]))),
            (get_key(APPROVALS_URL), ok(json!({"approved_by": []}))),
        ]);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items.len(), 1);
        assert!(items[0].draft);
    }

    #[test]
    fn a_failing_list_is_an_error_with_the_host_sentence() {
        let (http, _) = serve(vec![(
            get_key(LIST_URL),
            MockResponse::json(502, &json!({"message": "502 Bad Gateway"})),
        )]);
        assert_eq!(
            block_on(list_review_requests(&http, &session()))
                .expect_err("fails")
                .to_string(),
            "gitlab.com returned a server error (502)."
        );
    }

    // --- Detail ------------------------------------------------------------------

    fn detail_routes(
        changes: Value,
        versions: Value,
        discussions: Value,
    ) -> Vec<(String, MockResponse)> {
        vec![
            (get_key(MR), ok(mr_json())),
            (get_key(CHANGES_URL), ok(changes)),
            (get_key(VERSIONS_URL), ok(versions)),
            (get_key(DISCUSSIONS_URL), ok(discussions)),
        ]
    }

    fn changes_json() -> Value {
        json!({
            "id": 900,
            "changes": [
                {"old_path": "src/sync.rs", "new_path": "src/sync.rs", "new_file": false, "renamed_file": false, "deleted_file": false,
                 "diff": "@@ -1,2 +1,3 @@\n fn sync() {\n-    once();\n+    retry(3);\n+    log();\n }\n"},
                {"old_path": "src/old.rs", "new_path": "src/moved.rs", "new_file": false, "renamed_file": true, "deleted_file": false,
                 "diff": ""},
                {"old_path": "assets/logo.png", "new_path": "assets/logo.png", "new_file": true, "renamed_file": false, "deleted_file": false,
                 "diff": ""},
                {"old_path": "docs/gone.md", "new_path": "docs/gone.md", "new_file": false, "renamed_file": false, "deleted_file": true,
                 "diff": "@@ -1 +0,0 @@\n-bye\n"}
            ]
        })
    }

    #[test]
    fn load_detail_maps_the_changes_renames_binaries_and_totals() {
        let (http, _) = serve(detail_routes(changes_json(), versions_json(), json!([])));
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");

        let files = &detail.files;
        assert_eq!(files.len(), 4);

        assert_eq!(files[0].path, "src/sync.rs");
        assert_eq!(files[0].old_path, "src/sync.rs");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!((files[0].additions, files[0].deletions), (2, 1));
        assert!(
            files[0]
                .patch
                .as_deref()
                .unwrap_or_default()
                .starts_with("@@ -1,2 +1,3 @@")
        );
        assert!(!files[0].binary);

        // A rename with an empty diff: no patch, so binary as GitLab omits it.
        assert_eq!(files[1].path, "src/moved.rs");
        assert_eq!(files[1].old_path, "src/old.rs");
        assert_eq!(files[1].status, FileStatus::Renamed);
        assert_eq!(files[1].patch, None);
        assert!(files[1].binary);

        assert_eq!(files[2].status, FileStatus::Added);
        assert!(files[2].binary);

        assert_eq!(files[3].status, FileStatus::Removed);
        assert_eq!((files[3].additions, files[3].deletions), (0, 1));

        // The totals are the sum of the files, and the changed count is their number.
        assert_eq!(detail.item.additions, Some(2));
        assert_eq!(detail.item.deletions, Some(2));
        assert_eq!(detail.item.changed_files, Some(4));
    }

    #[test]
    fn load_detail_takes_refs_from_the_latest_version() {
        let (http, _) = serve(detail_routes(
            json!({"changes": []}),
            versions_json(),
            json!([]),
        ));
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert_eq!(
            detail.refs,
            DiffRefs {
                base_sha: Some("base333".into()),
                start_sha: Some("start444".into()),
                head_sha: Some("head222".into()),
            }
        );
        assert_eq!(detail.description, "Retries three times.");
    }

    #[test]
    fn load_detail_without_versions_uses_the_merge_request_head() {
        let (http, _) = serve(detail_routes(json!({"diffs": []}), json!([]), json!([])));
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert_eq!(
            detail.refs,
            DiffRefs {
                base_sha: None,
                start_sha: None,
                head_sha: Some("head111".into()),
            }
        );
        assert!(detail.files.is_empty());
    }

    #[test]
    fn load_detail_reads_the_older_diffs_key_and_prefers_changes() {
        let (http, _) = serve(detail_routes(
            json!({"diffs": [{"old_path": "a.rs", "new_path": "a.rs", "diff": "@@ -0,0 +1 @@\n+x\n"}]}),
            json!([]),
            json!([]),
        ));
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert_eq!(detail.files.len(), 1);
        assert_eq!(detail.files[0].status, FileStatus::Modified);
        assert_eq!(detail.files[0].additions, 1);

        let (http, _) = serve(detail_routes(
            json!({"changes": [], "diffs": [{"old_path": "a.rs", "new_path": "a.rs", "diff": "+x"}]}),
            json!([]),
            json!([]),
        ));
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert!(
            detail.files.is_empty(),
            "an empty `changes` is still the answer"
        );
    }

    #[test]
    fn a_failing_changes_read_fails_the_detail_load() {
        let (http, _) = serve(vec![
            (get_key(MR), ok(mr_json())),
            (
                get_key(CHANGES_URL),
                MockResponse::json(500, &json!({"message": "500 Internal Server Error"})),
            ),
            (get_key(VERSIONS_URL), ok(json!([]))),
            (get_key(DISCUSSIONS_URL), ok(json!([]))),
        ]);
        assert_eq!(
            block_on(load_detail(&http, &session(), &item()))
                .expect_err("fails")
                .to_string(),
            "gitlab.com returned a server error (500)."
        );
    }

    #[test]
    fn load_detail_follows_discussion_pages_across_the_cap() {
        let note = |id: u64, body: &str| {
            json!({"id": id, "body": body, "author": {"username": "alice", "avatar_url": "https://gitlab.example/a.png"},
                   "created_at": "2026-08-02T10:00:00.000Z", "system": false, "resolvable": false, "resolved": false,
                   "individual_note": true})
        };
        let discussion = |id: &str, body: &str| json!({"id": id, "individual_note": true, "notes": [note(1, body)]});
        let first = MockResponse::json(200, &json!([discussion("d1", "first page")]))
            .header("Link", "<https://gitlab.com/api/v4/projects/42/merge_requests/7/discussions?per_page=100&page=2>; rel=\"next\"");
        let second = MockResponse::json(200, &json!([discussion("d2", "second page")]))
            .header("Link", "<https://gitlab.com/api/v4/projects/42/merge_requests/7/discussions?per_page=100&page=3>; rel=\"next\"");
        let third = MockResponse::json(200, &json!([discussion("d3", "third page")]));
        let page_two = format!("{DISCUSSIONS_URL}&page=2");
        let page_three = format!("{DISCUSSIONS_URL}&page=3");
        let (http, calls) = serve(vec![
            (get_key(MR), ok(mr_json())),
            (get_key(CHANGES_URL), ok(json!({"changes": []}))),
            (get_key(VERSIONS_URL), ok(json!([]))),
            (get_key(DISCUSSIONS_URL), first),
            (get_key(&page_two), second),
            (get_key(&page_three), third),
        ]);
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        let bodies: Vec<&str> = detail
            .threads
            .iter()
            .flat_map(|thread| thread.comments.iter().map(|comment| comment.body.as_str()))
            .collect();
        assert_eq!(bodies, vec!["first page", "second page", "third page"]);
        assert_eq!(keys(&calls).len(), 6);
    }

    #[test]
    fn a_failing_discussion_page_reads_as_no_conversation() {
        let (http, _) = serve(vec![
            (get_key(MR), ok(mr_json())),
            (get_key(CHANGES_URL), ok(json!({"changes": []}))),
            (get_key(VERSIONS_URL), ok(json!([]))),
            (
                get_key(DISCUSSIONS_URL),
                MockResponse::json(500, &json!({"message": "down"})),
            ),
        ]);
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert!(detail.threads.is_empty());
    }

    // --- Threads and checks ------------------------------------------------------

    #[test]
    fn load_threads_reads_the_discussions_against_the_latest_head() {
        let discussions = json!([{
            "id": "disc-1",
            "individual_note": false,
            "notes": [
                {"id": 11, "body": "Why retry?", "author": {"username": "alice", "avatar_url": "https://gitlab.example/a.png"},
                 "created_at": "2026-08-02T10:00:00.000Z", "system": false, "resolvable": true, "resolved": false,
                 "type": "DiffNote",
                 "position": {"new_path": "src/sync.rs", "new_line": 12, "head_sha": "head222", "base_sha": "base333", "start_sha": "start444"}},
                {"id": 12, "body": "changed the description", "author": {"username": "bob"}, "created_at": "2026-08-02T11:00:00.000Z", "system": true, "resolvable": false, "resolved": false}
            ]
        }]);
        let (http, calls) = serve(vec![
            (get_key(VERSIONS_URL), ok(versions_json())),
            (get_key(DISCUSSIONS_URL), ok(discussions)),
        ]);
        let threads = block_on(load_threads(&http, &session(), &item())).expect("loads");
        // The system note is chatter, not conversation.
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comments.len(), 1);
        assert_eq!(threads[0].comments[0].body, "Why retry?");
        assert_eq!(threads[0].path.as_deref(), Some("src/sync.rs"));
        assert_eq!(threads[0].line, Some(12));
        assert!(threads[0].can_resolve);
        assert_eq!(keys(&calls).len(), 2);
    }

    #[test]
    fn load_threads_with_no_versions_still_reads_the_conversation() {
        let (http, _) = serve(vec![
            (
                get_key(VERSIONS_URL),
                MockResponse::json(404, &json!({"message": "404"})),
            ),
            (get_key(DISCUSSIONS_URL), ok(json!([]))),
        ]);
        let threads = block_on(load_threads(&http, &session(), &item())).expect("loads");
        assert!(threads.is_empty());
    }

    #[test]
    fn refresh_checks_reads_the_latest_pipeline_of_the_merge_request() {
        let (http, calls) = serve(vec![
            (get_key(PIPELINES_URL), ok(pipelines_json())),
            (get_key(JOBS_URL), ok(jobs_json())),
        ]);
        let checks = block_on(refresh_checks(&http, &session(), &item())).expect("refreshes");
        assert_eq!(checks.total, 3);
        assert_eq!(
            keys(&calls),
            vec![get_key(PIPELINES_URL), get_key(JOBS_URL)]
        );
    }

    #[test]
    fn refresh_checks_with_no_pipelines_is_unknown() {
        let (http, _) = serve(vec![(get_key(PIPELINES_URL), ok(json!([])))]);
        let checks = block_on(refresh_checks(&http, &session(), &item())).expect("refreshes");
        assert_eq!(checks, empty_checks());
    }

    // --- Submitting --------------------------------------------------------------

    fn refs() -> DiffRefs {
        DiffRefs {
            base_sha: Some("base333".into()),
            start_sha: Some("start444".into()),
            head_sha: Some("head222".into()),
        }
    }

    fn draft(path: &str, body: &str, new_line: u32) -> DraftComment {
        DraftComment {
            id: format!("draft-{new_line}"),
            item_id: "acct-2:42:7".into(),
            body: body.into(),
            path: path.into(),
            new_line: Some(new_line),
            old_line: None,
            range: None,
            created_at: "2026-08-02T10:00:00.000Z".into(),
            refs: refs(),
        }
    }

    #[test]
    fn approve_sends_the_approval_and_then_the_note() {
        let (http, calls) = serve(vec![
            (post_key(APPROVE_URL), ok(json!({"approved": true}))),
            (post_key(NOTES_URL), ok(json!({"id": 1}))),
        ]);
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Approve,
            "Ship it",
            &[],
        ))
        .expect("submits");
        assert_eq!(
            keys(&calls),
            vec![post_key(APPROVE_URL), post_key(NOTES_URL)]
        );
        assert_eq!(
            body_of(&calls, &post_key(NOTES_URL)),
            json!({"body": "Ship it"})
        );
        assert_eq!(body_of(&calls, &post_key(APPROVE_URL)), Value::Null);
    }

    #[test]
    fn approve_without_a_body_sends_no_note() {
        let (http, calls) = serve(vec![(post_key(APPROVE_URL), ok(json!({"approved": true})))]);
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Approve,
            "",
            &[],
        ))
        .expect("submits");
        assert_eq!(keys(&calls), vec![post_key(APPROVE_URL)]);
    }

    #[test]
    fn request_changes_drops_the_approval_and_says_why() {
        let (http, calls) = serve(vec![
            (post_key(UNAPPROVE_URL), ok(json!({}))),
            (post_key(NOTES_URL), ok(json!({"id": 2}))),
        ]);
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::RequestChanges,
            "",
            &[],
        ))
        .expect("submits");
        assert_eq!(
            keys(&calls),
            vec![post_key(UNAPPROVE_URL), post_key(NOTES_URL)]
        );
        assert_eq!(
            body_of(&calls, &post_key(NOTES_URL)),
            json!({"body": "Changes requested."})
        );
    }

    #[test]
    fn request_changes_with_a_body_uses_the_body_and_survives_a_failed_unapprove() {
        let (http, calls) = serve(vec![
            (
                post_key(UNAPPROVE_URL),
                MockResponse::json(404, &json!({"message": "404 Not approved"})),
            ),
            (post_key(NOTES_URL), ok(json!({"id": 2}))),
        ]);
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::RequestChanges,
            "Fix the retry",
            &[],
        ))
        .expect("submits");
        assert_eq!(
            body_of(&calls, &post_key(NOTES_URL)),
            json!({"body": "Fix the retry"})
        );
    }

    #[test]
    fn a_plain_comment_sends_just_the_note() {
        let (http, calls) = serve(vec![(post_key(NOTES_URL), ok(json!({"id": 3})))]);
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Comment,
            "Hmm",
            &[],
        ))
        .expect("submits");
        assert_eq!(keys(&calls), vec![post_key(NOTES_URL)]);

        let (http, calls) = serve(Vec::new());
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Comment,
            "",
            &[],
        ))
        .expect("submits");
        assert!(calls.lock().is_empty());
    }

    #[test]
    fn drafts_go_one_discussion_at_a_time_before_the_verdict() {
        let (http, calls) = serve(vec![
            (post_key(DISCUSSIONS_POST), ok(json!({"id": "d1"}))),
            (post_key(APPROVE_URL), ok(json!({"approved": true}))),
        ]);
        let drafts = vec![
            draft("src/sync.rs", "Nit", 12),
            draft("src/other.rs", "Naming", 3),
        ];
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Approve,
            "",
            &drafts,
        ))
        .expect("submits");
        assert_eq!(
            keys(&calls),
            vec![
                post_key(DISCUSSIONS_POST),
                post_key(DISCUSSIONS_POST),
                post_key(APPROVE_URL),
            ]
        );
    }

    #[test]
    fn a_draft_is_posted_as_a_form_placed_by_the_three_shas() {
        let (http, calls) = serve(vec![(post_key(DISCUSSIONS_POST), ok(json!({"id": "d1"})))]);
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Comment,
            "",
            &[draft("src/sync.rs", "Nit", 12)],
        ))
        .expect("submits");
        let form = raw_body_of(&calls, &post_key(DISCUSSIONS_POST));
        assert!(form.contains("body=Nit"), "{form}");
        assert!(form.contains("position%5Bposition_type%5D=text"), "{form}");
        assert!(form.contains("position%5Bbase_sha%5D=base333"), "{form}");
        assert!(form.contains("position%5Bstart_sha%5D=start444"), "{form}");
        assert!(form.contains("position%5Bhead_sha%5D=head222"), "{form}");
        assert!(
            form.contains("position%5Bnew_path%5D=src%2Fsync.rs"),
            "{form}"
        );
        assert!(form.contains("position%5Bnew_line%5D=12"), "{form}");
    }

    #[test]
    fn a_draft_without_its_diff_refs_stops_before_anything_is_sent() {
        let mut bare = draft("src/sync.rs", "Nit", 12);
        bare.refs = DiffRefs::default();
        let (http, calls) = serve(Vec::new());
        assert_eq!(
            block_on(submit_review(
                &http,
                &session(),
                &item(),
                ReviewVerdict::Comment,
                "",
                &[bare]
            ))
            .expect_err("no refs")
            .to_string(),
            "GitLab needs the merge request diff refs; reload the merge request and try again."
        );
        assert!(calls.lock().is_empty());
    }

    #[test]
    fn a_draft_that_fails_halfway_reports_which_comments_landed() {
        // Both drafts post to the same URL, so the answer depends on the order of calls:
        // the first lands, the second is refused.
        let posts = AtomicUsize::new(0);
        let http = Http::mock(move |_request: &MockRequest| {
            if posts.fetch_add(1, Ordering::SeqCst) == 0 {
                ok(json!({"id": "d1"}))
            } else {
                MockResponse::json(400, &json!({"message": "Line is not part of the diff"}))
            }
        });
        let drafts = vec![
            draft("src/sync.rs", "Nit", 12),
            draft("src/other.rs", "Naming", 3),
        ];
        let error = block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Comment,
            "",
            &drafts,
        ))
        .expect_err("half done");
        match error {
            crate::error::Error::PartialSubmit { message, posted } => {
                assert_eq!(posted, vec!["draft-12".to_string()]);
                assert!(
                    message.starts_with(
                        "Posted 1 of 2 comments, then stopped: Line is not part of the diff."
                    ),
                    "{message}"
                );
            }
            other => panic!("expected a partial submit, got {other:?}"),
        }
    }

    #[test]
    fn add_comment_posts_a_merge_request_note() {
        let (http, calls) = serve(vec![(post_key(NOTES_URL), ok(json!({"id": 4})))]);
        block_on(add_comment(&http, &session(), &item(), "Thanks")).expect("posts");
        assert_eq!(
            body_of(&calls, &post_key(NOTES_URL)),
            json!({"body": "Thanks"})
        );
    }

    #[test]
    fn add_line_comment_posts_a_positioned_discussion() {
        let (http, calls) = serve(vec![(post_key(DISCUSSIONS_POST), ok(json!({"id": "d9"})))]);
        let line = LineCommentDraft {
            item_id: "acct-2:42:7".into(),
            body: "Range here".into(),
            path: "src/sync.rs".into(),
            new_line: Some(14),
            old_line: None,
            range: Some(LineRange {
                start_line: 10,
                start: RangeEdge {
                    kind: EdgeKind::Add,
                    old_pos: 9,
                    new_pos: 10,
                },
                end: RangeEdge {
                    kind: EdgeKind::Add,
                    old_pos: 11,
                    new_pos: 14,
                },
            }),
        };
        block_on(add_line_comment(&http, &session(), &item(), &line, &refs())).expect("posts");
        let form = raw_body_of(&calls, &post_key(DISCUSSIONS_POST));
        assert!(form.contains("body=Range%20here"), "{form}");
        assert!(
            form.contains("position%5Bline_range%5D%5Bstart%5D%5Btype%5D=new"),
            "{form}"
        );
        assert!(
            form.contains("position%5Bline_range%5D%5Bend%5D%5Bnew_line%5D=14"),
            "{form}"
        );
    }

    #[test]
    fn reply_to_thread_posts_a_note_into_the_discussion() {
        let url =
            "https://gitlab.com/api/v4/projects/42/merge_requests/7/discussions/abc%20123/notes";
        let (http, calls) = serve(vec![(post_key(url), ok(json!({"id": 5})))]);
        block_on(reply_to_thread(
            &http,
            &session(),
            &item(),
            "abc 123",
            "Done",
        ))
        .expect("replies");
        assert_eq!(body_of(&calls, &post_key(url)), json!({"body": "Done"}));
    }

    #[test]
    fn set_thread_resolved_puts_the_resolved_flag() {
        let url = "https://gitlab.com/api/v4/projects/42/merge_requests/7/discussions/d1";
        let (http, calls) = serve(vec![(format!("PUT {url}"), ok(json!({"id": "d1"})))]);
        block_on(set_thread_resolved(&http, &session(), &item(), "d1", true)).expect("resolves");
        assert_eq!(
            body_of(&calls, &format!("PUT {url}")),
            json!({"resolved": true})
        );

        let (http, calls) = serve(vec![(format!("PUT {url}"), ok(json!({"id": "d1"})))]);
        block_on(set_thread_resolved(&http, &session(), &item(), "d1", false)).expect("reopens");
        assert_eq!(
            body_of(&calls, &format!("PUT {url}")),
            json!({"resolved": false})
        );
    }

    #[test]
    fn a_failed_resolve_is_reported_in_the_hosts_words() {
        let url = "https://gitlab.com/api/v4/projects/42/merge_requests/7/discussions/d1";
        let (http, _) = serve(vec![(
            format!("PUT {url}"),
            MockResponse::json(403, &json!({"message": "403 Forbidden"})),
        )]);
        // A 403 has its own sentence in http.ts, which names the missing scope.
        assert_eq!(
            block_on(set_thread_resolved(&http, &session(), &item(), "d1", true))
                .expect_err("forbidden")
                .to_string(),
            "Forbidden on gitlab.com - the token is missing a required scope."
        );
    }

    #[test]
    fn encode_uri_component_escapes_project_paths_and_discussion_ids() {
        assert_eq!(
            encode_uri_component("gitlab-org/widgets"),
            "gitlab-org%2Fwidgets"
        );
        assert_eq!(encode_uri_component("42"), "42");
        assert_eq!(encode_uri_component("a b+c"), "a%20b%2Bc");
    }

    #[test]
    fn the_private_token_header_is_the_gitlab_auth_shape() {
        let headers = headers("glpat-test");
        assert_eq!(headers[0], ("PRIVATE-TOKEN".into(), "glpat-test".into()));
        assert_eq!(headers[1], ("User-Agent".into(), "Reviewdeck".into()));
    }

    #[test]
    fn notes_without_optional_fields_still_make_a_thread() {
        let (http, _) = serve(vec![
            (get_key(VERSIONS_URL), ok(json!([]))),
            (
                get_key(DISCUSSIONS_URL),
                ok(
                    json!([{"id": "d1", "individual_note": true, "notes": [{"id": 1, "body": "still here"}]}]),
                ),
            ),
        ]);
        let threads = block_on(load_threads(&http, &session(), &item())).expect("loads");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comments[0].body, "still here");
    }
}
