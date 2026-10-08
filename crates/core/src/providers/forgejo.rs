//! Port of src/main/providers/forgejo.ts.
//!
//! Forgejo and Gitea share an API surface, so one adapter covers both.
//!
//! `/repos/issues/search?review_requested=true` aggregates across every repo the
//! token can see, which keeps the sync to a single round trip plus per-PR detail.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use url::Url;

use crate::diff::parse_unified_diff;
use crate::error::{Result, msg};
use crate::http::{Http, Method, RequestOptions, to_origin};
use crate::model::{
    AccountDraft, ApprovalSummary, CheckRun, CheckStatus, CheckSummary, CommentThread, DiffRefs,
    DraftComment, LineCommentDraft, MyReviewState, NewAccount, ProviderKind, PullDetail,
    ReviewItem, ReviewVerdict, Side, User, count_approvers, make_item_id, summarise_approvals,
    summarise_checks,
};
use crate::providers::Session;
use crate::providers::limit_concurrency;
use crate::providers::submit::{Placed, forgejo_inline_comment, forgejo_review_payload};
use crate::providers::threads::{
    ForgejoComment, ForgejoReviewComment, forgejo_threads, parse_forgejo_thread_id,
};

/// Every pull request awaiting the token's review, across the repos it can see.
const SEARCH_PATH: &str =
    "/repos/issues/search?type=pulls&state=open&review_requested=true&limit=50&sort=recentupdate";

/// Concurrency limits from the TypeScript: pulls, then review comment batches.
const PULL_CONCURRENCY: usize = 5;
const REVIEW_COMMENT_CONCURRENCY: usize = 4;

// ---------------------------------------------------------------------------
// The host shapes. Lenient like the threads: every field may be missing or null,
// and unknown fields are ignored. A list entry that does not read as its shape is
// dropped rather than failing the whole list.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FjUser {
    login: Option<String>,
    full_name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FjRepository {
    full_name: Option<String>,
    /// A string on every version the search has shipped, but read leniently.
    owner: Option<Value>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FjIssue {
    number: Option<u64>,
    html_url: Option<String>,
    repository: Option<FjRepository>,
}

#[derive(Debug, Deserialize)]
struct FjLabel {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FjRef {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FjPull {
    title: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    draft: Option<bool>,
    created_at: Option<String>,
    updated_at: Option<String>,
    user: Option<FjUser>,
    head: Option<FjRef>,
    base: Option<FjRef>,
    additions: Option<u32>,
    deletions: Option<u32>,
    changed_files: Option<u32>,
    labels: Option<Vec<FjLabel>>,
}

impl FjPull {
    fn head_sha(&self) -> String {
        self.head
            .as_ref()
            .and_then(|head| head.sha.clone())
            .unwrap_or_default()
    }

    fn head_branch(&self) -> String {
        self.head
            .as_ref()
            .and_then(|head| head.git_ref.clone())
            .unwrap_or_default()
    }

    fn base_sha(&self) -> Option<String> {
        self.base.as_ref().and_then(|base| base.sha.clone())
    }

    fn base_branch(&self) -> String {
        self.base
            .as_ref()
            .and_then(|base| base.git_ref.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct FjReview {
    /// Numeric on the host; read as text because the review comments URL needs it so.
    id: Option<Value>,
    state: Option<String>,
    user: Option<FjUser>,
    dismissed: Option<bool>,
    comments_count: Option<i64>,
}

impl FjReview {
    fn login(&self) -> Option<&str> {
        self.user.as_ref().and_then(|user| user.login.as_deref())
    }
}

#[derive(Debug, Deserialize)]
struct FjBranchProtection {
    required_approvals: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct FjStatus {
    id: Option<Value>,
    status: Option<String>,
    context: Option<String>,
    target_url: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FjCombinedStatus {
    state: Option<String>,
    statuses: Option<Vec<Value>>,
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// Forgejo's auth header is `token <value>`, not a bearer.
fn request_options(method: Method, token: &str) -> RequestOptions {
    RequestOptions::new(method).headers([
        ("Authorization", format!("token {token}")),
        ("Accept", "application/json".to_string()),
        ("User-Agent", "Reviewdeck".to_string()),
    ])
}

fn api(s: &Session, path: &str) -> String {
    format!("{}{}", s.account.base_url, path)
}

/// Entries that do not read as their shape are dropped, so one odd entry costs only
/// itself.
fn lenient<T: DeserializeOwned>(values: impl IntoIterator<Item = Value>) -> Vec<T> {
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

/// An id as text, whether the host sent it as a number or a string.
fn id_text(id: Option<&Value>) -> Option<String> {
    match id? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn status_state(status: &str) -> CheckStatus {
    match status {
        "success" => CheckStatus::Passed,
        "failure" | "error" => CheckStatus::Failed,
        "pending" => CheckStatus::Running,
        // "warning" and anything else the host invents.
        _ => CheckStatus::Unknown,
    }
}

fn empty_checks() -> CheckSummary {
    CheckSummary {
        status: CheckStatus::Unknown,
        passed: 0,
        failed: 0,
        running: 0,
        total: 0,
        runs: Vec::new(),
    }
}

/// The search result nests repo differently across Gitea/Forgejo versions. With a
/// repository object, its full name is used; without one, the first two segments of
/// the pull request's own web address are.
fn repo_name(issue: &FjIssue) -> String {
    if let Some(repo) = &issue.repository {
        if let Some(full_name) = &repo.full_name {
            return full_name.clone();
        }
        let owner = repo.owner.as_ref().and_then(Value::as_str);
        if let (Some(owner), Some(name)) = (owner, &repo.name) {
            return format!("{owner}/{name}");
        }
    }
    // Fall back to slicing the html_url: https://host/owner/repo/pulls/12
    if let Some(parts) = issue
        .html_url
        .as_deref()
        .and_then(|raw| Url::parse(raw).ok())
        .map(|url| {
            url.path_segments()
                .map(|segments| {
                    segments
                        .filter(|segment| !segment.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        && parts.len() >= 2
    {
        return format!("{}/{}", parts[0], parts[1]);
    }
    "unknown/unknown".to_string()
}

/// The checks on one commit: every individual status, or the combined verdict when
/// the host sent no individual ones. Any failure reads as no checks at all.
async fn load_checks(http: &Http, s: &Session, repo: &str, sha: &str) -> CheckSummary {
    if sha.is_empty() {
        // A pull request without a head commit has nothing to ask about.
        return empty_checks();
    }
    fetch_checks(http, s, repo, sha)
        .await
        .unwrap_or_else(|_| empty_checks())
}

async fn fetch_checks(http: &Http, s: &Session, repo: &str, sha: &str) -> Result<CheckSummary> {
    let combined: FjCombinedStatus = http
        .json(
            &api(s, &format!("/repos/{repo}/commits/{sha}/status")),
            request_options(Method::Get, &s.token),
        )
        .await?;

    let runs: Vec<CheckRun> = lenient::<FjStatus>(combined.statuses.unwrap_or_default())
        .into_iter()
        .enumerate()
        .map(|(index, status)| CheckRun {
            id: format!(
                "status-{}",
                id_text(status.id.as_ref()).unwrap_or_else(|| index.to_string())
            ),
            name: non_empty(status.context).unwrap_or_else(|| "check".to_string()),
            status: status_state(status.status.as_deref().unwrap_or_default()),
            url: non_empty(status.target_url),
            description: non_empty(status.description),
        })
        .collect();

    if runs.is_empty() {
        // No individual statuses but a combined verdict still tells us something.
        let rolled = status_state(combined.state.as_deref().unwrap_or_default());
        if rolled == CheckStatus::Unknown {
            return Ok(empty_checks());
        }
        let run = CheckRun {
            id: "combined".to_string(),
            name: "Checks".to_string(),
            status: rolled,
            url: None,
            description: None,
        };
        return Ok(summarise_checks(vec![run]));
    }
    Ok(summarise_checks(runs))
}

/// JavaScript's `x || undefined` for an optional string.
fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.is_empty())
}

/// How many approvals the target branch wants, or nothing when the token cannot
/// ask. Listing branch protection is an administrator's call on Forgejo, so most
/// reviewers get a 403 here and a card that says nothing is required - which is
/// the honest answer, since nothing the app can see says otherwise.
///
/// Rules are named after the branch they cover and can also be globs; only the
/// exact name is looked up, and a glob rule reads as no rule at all.
async fn required_approvals(http: &Http, s: &Session, repo: &str, branch: &str) -> Option<u32> {
    let url = api(
        s,
        &format!(
            "/repos/{repo}/branch_protections/{}",
            encode_uri_component(branch)
        ),
    );
    http.json::<FjBranchProtection>(&url, request_options(Method::Get, &s.token))
        .await
        .ok()
        .and_then(|rule| rule.required_approvals)
}

/// The token's own standing verdict, and how many reviewers stand approving.
async fn load_approvals(http: &Http, s: &Session, repo: &str, number: u64) -> (MyReviewState, u32) {
    let url = api(s, &format!("/repos/{repo}/pulls/{number}/reviews"));
    let reviews: Vec<FjReview> = match http
        .json::<Vec<Value>>(&url, request_options(Method::Get, &s.token))
        .await
    {
        Ok(values) => lenient(values),
        // Not fatal: the card shows the item as pending with nothing given.
        Err(_) => return (MyReviewState::Pending, 0),
    };

    let given = count_approvers(reviews.iter().map(|review| {
        let state = if review.dismissed == Some(true) {
            "DISMISSED"
        } else {
            review.state.as_deref().unwrap_or_default()
        };
        (review.login(), state)
    }));

    let mine: Vec<&FjReview> = reviews
        .iter()
        .filter(|review| review.login() == Some(s.account.username.as_str()))
        .collect();
    // The last verdict the token gave is the standing one.
    for review in mine.iter().rev() {
        match review.state.as_deref() {
            Some("APPROVED") => return (MyReviewState::Approved, given),
            Some("REQUEST_CHANGES") => return (MyReviewState::ChangesRequested, given),
            _ => {}
        }
    }
    if mine
        .iter()
        .any(|review| review.state.as_deref() == Some("COMMENT"))
    {
        return (MyReviewState::Commented, given);
    }
    (MyReviewState::Pending, given)
}

/// The whole conversation: the comments on the pull request and the inline ones,
/// which live in different places here and become one set of threads. Neither
/// fetch can fail the conversation, as in the TypeScript.
async fn fetch_threads(http: &Http, s: &Session, repo: &str, number: u64) -> Vec<CommentThread> {
    let comments_url = api(s, &format!("/repos/{repo}/issues/{number}/comments"));
    let comments = async {
        lenient::<ForgejoComment>(
            http.json::<Vec<Value>>(&comments_url, request_options(Method::Get, &s.token))
                .await
                .unwrap_or_default(),
        )
    };
    let (comments, review_comments) =
        futures::join!(comments, load_review_comments(http, s, repo, number));
    forgejo_threads(&comments, &review_comments)
}

/// Inline comments hang off reviews rather than off the pull request, so they take a
/// call per review that has any. Reviews with none are skipped, which is most of
/// them, and a review that will not load costs its own comments and nothing else.
async fn load_review_comments(
    http: &Http,
    s: &Session,
    repo: &str,
    number: u64,
) -> Vec<ForgejoReviewComment> {
    let reviews_url = api(s, &format!("/repos/{repo}/pulls/{number}/reviews"));
    let reviews: Vec<FjReview> = lenient(
        http.json::<Vec<Value>>(&reviews_url, request_options(Method::Get, &s.token))
            .await
            .unwrap_or_default(),
    );

    // A review without a comments count is treated as having some, as the TypeScript does.
    let review_ids: Vec<String> = reviews
        .iter()
        .filter(|review| review.comments_count.unwrap_or(1) > 0)
        .filter_map(|review| id_text(review.id.as_ref()))
        .collect();

    let batches = limit_concurrency(
        review_ids,
        REVIEW_COMMENT_CONCURRENCY,
        move |review_id| async move {
            let url = api(
                s,
                &format!("/repos/{repo}/pulls/{number}/reviews/{review_id}/comments"),
            );
            lenient::<ForgejoReviewComment>(
                http.json::<Vec<Value>>(&url, request_options(Method::Get, &s.token))
                    .await
                    .unwrap_or_default(),
            )
        },
    )
    .await;

    batches.into_iter().flatten().collect()
}

/// One inline comment, sent as a review of its own. `commit_id` is left out when
/// unknown, as JSON.stringify would leave an undefined key out.
async fn inline_comment(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    commit_id: Option<&str>,
    comment: Placed<'_>,
) -> Result<()> {
    let mut payload = json!({
        "event": "COMMENT",
        "body": "",
        "comments": [forgejo_inline_comment(comment)],
    });
    // Left off, Forgejo uses the pull request's head, which is what we want anyway.
    if let (Some(commit_id), Some(object)) = (commit_id, payload.as_object_mut()) {
        object.insert("commit_id".to_string(), json!(commit_id));
    }
    http.request(
        &api(
            s,
            &format!("/repos/{}/pulls/{}/reviews", item.repo_key, item.number),
        ),
        request_options(Method::Post, &s.token).json(payload),
    )
    .await?;
    Ok(())
}

/// Builds the item from the pull request and what was read about it.
fn review_item(
    account_id: &str,
    candidate: Candidate,
    approvals: ApprovalSummary,
    checks: CheckSummary,
    review_state: MyReviewState,
) -> ReviewItem {
    let Candidate { repo, number, pull } = candidate;
    ReviewItem {
        id: make_item_id(account_id, &repo, number),
        account_id: account_id.to_string(),
        provider: ProviderKind::Forgejo,
        repo_key: repo.clone(),
        repo,
        number,
        title: pull.title.clone().unwrap_or_default(),
        url: pull.html_url.clone().unwrap_or_default(),
        author: User {
            name: pull
                .user
                .as_ref()
                .and_then(|user| user.login.clone())
                .unwrap_or_default(),
            avatar_url: pull
                .user
                .as_ref()
                .and_then(|user| user.avatar_url.clone())
                .unwrap_or_default(),
        },
        created_at: pull.created_at.clone().unwrap_or_default(),
        updated_at: pull.updated_at.clone().unwrap_or_default(),
        draft: pull.draft.unwrap_or(false),
        source_branch: pull.head_branch(),
        target_branch: pull.base_branch(),
        labels: pull
            .labels
            .as_ref()
            .map(|labels| {
                labels
                    .iter()
                    .filter_map(|label| label.name.clone())
                    .collect()
            })
            .unwrap_or_default(),
        my_review_state: review_state,
        approvals,
        checks,
        additions: pull.additions,
        deletions: pull.deletions,
        changed_files: pull.changed_files,
    }
}

/// A pull request the search named, with its own detail read.
struct Candidate {
    repo: String,
    number: u64,
    pull: FjPull,
}

pub async fn connect(http: &Http, draft: &AccountDraft) -> Result<NewAccount> {
    let origin = to_origin(&draft.host)?;
    let base_url = format!("{origin}/api/v1");
    let user: FjUser = http
        .json(
            &format!("{base_url}/user"),
            request_options(Method::Get, &draft.token),
        )
        .await?;
    let login = user.login.unwrap_or_default();
    let label = if draft.label.is_empty() {
        format!("{} ({login})", host_of(&origin))
    } else {
        draft.label.clone()
    };
    let display_name = non_empty(user.full_name).unwrap_or_else(|| login.clone());
    Ok(NewAccount {
        kind: ProviderKind::Forgejo,
        label,
        base_url,
        web_url: origin,
        username: login,
        display_name,
        avatar_url: user.avatar_url.unwrap_or_default(),
        agent_command: None,
    })
}

/// The host as `new URL(origin).host` writes it: the name, and the port when it is
/// not the scheme's default.
fn host_of(origin: &str) -> String {
    match Url::parse(origin) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_string(),
            _ => origin.to_string(),
        },
        Err(_) => origin.to_string(),
    }
}

pub async fn list_review_requests(http: &Http, s: &Session) -> Result<Vec<ReviewItem>> {
    let issues: Vec<FjIssue> = lenient(
        http.json::<Vec<Value>>(&api(s, SEARCH_PATH), request_options(Method::Get, &s.token))
            .await?,
    );
    let wanted: Vec<(String, u64)> = issues
        .iter()
        .filter_map(|issue| Some((repo_name(issue), issue.number?)))
        .collect();

    // The pull request itself carries the head and base the rest is keyed by.
    let candidates =
        limit_concurrency(wanted, PULL_CONCURRENCY, move |(repo, number)| async move {
            let url = api(s, &format!("/repos/{repo}/pulls/{number}"));
            let pull: Result<FjPull> = http
                .json(&url, request_options(Method::Get, &s.token))
                .await;
            pull.map(|pull| Candidate { repo, number, pull })
        })
        .await
        .into_iter()
        .collect::<Result<Vec<Candidate>>>()?;

    // One protection lookup per branch across the sync, not one per pull request.
    let branches: Vec<(String, String)> = candidates
        .iter()
        .map(|candidate| (candidate.repo.clone(), candidate.pull.base_branch()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let protections: HashMap<(String, String), Option<u32>> = limit_concurrency(
        branches,
        PULL_CONCURRENCY,
        move |(repo, branch)| async move {
            let required = required_approvals(http, s, &repo, &branch).await;
            ((repo, branch), required)
        },
    )
    .await
    .into_iter()
    .collect();

    let protections = &protections;
    let items = limit_concurrency(candidates, PULL_CONCURRENCY, move |candidate| async move {
        let branch = candidate.pull.base_branch();
        let required = protections
            .get(&(candidate.repo.clone(), branch))
            .copied()
            .flatten();
        let head_sha = candidate.pull.head_sha();
        let (checks, (review_state, given)) = futures::join!(
            load_checks(http, s, &candidate.repo, &head_sha),
            load_approvals(http, s, &candidate.repo, candidate.number),
        );
        review_item(
            &s.account.id,
            candidate,
            summarise_approvals(given, required),
            checks,
            review_state,
        )
    })
    .await;
    Ok(items)
}

pub async fn load_detail(http: &Http, s: &Session, item: &ReviewItem) -> Result<PullDetail> {
    let pull_url = api(
        s,
        &format!("/repos/{}/pulls/{}", item.repo_key, item.number),
    );
    let (pull, diff, threads) = futures::join!(
        http.json::<FjPull>(&pull_url, request_options(Method::Get, &s.token)),
        // A diff that will not load leaves the files empty rather than failing the page.
        async {
            http.text(
                &format!("{pull_url}.diff"),
                request_options(Method::Get, &s.token),
            )
            .await
            .unwrap_or_default()
        },
        fetch_threads(http, s, &item.repo_key, item.number),
    );
    let pull = pull?;

    let files = parse_unified_diff(&diff);
    let (added, removed) = files.iter().fold((0u32, 0u32), |(added, removed), file| {
        (
            added.saturating_add(file.additions),
            removed.saturating_add(file.deletions),
        )
    });

    let mut detail_item = item.clone();
    detail_item.additions = Some(pull.additions.unwrap_or(added));
    detail_item.deletions = Some(pull.deletions.unwrap_or(removed));
    detail_item.changed_files = Some(
        pull.changed_files
            .unwrap_or_else(|| u32::try_from(files.len()).unwrap_or(u32::MAX)),
    );

    Ok(PullDetail {
        item: detail_item,
        description: pull.body.clone().unwrap_or_default(),
        files,
        threads,
        refs: DiffRefs {
            head_sha: Some(pull.head_sha()),
            base_sha: pull.base_sha(),
            start_sha: None,
        },
    })
}

pub async fn refresh_checks(http: &Http, s: &Session, item: &ReviewItem) -> Result<CheckSummary> {
    let pull: FjPull = http
        .json(
            &api(
                s,
                &format!("/repos/{}/pulls/{}", item.repo_key, item.number),
            ),
            request_options(Method::Get, &s.token),
        )
        .await?;
    Ok(load_checks(http, s, &item.repo_key, &pull.head_sha()).await)
}

pub async fn load_threads(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
) -> Result<Vec<CommentThread>> {
    Ok(fetch_threads(http, s, &item.repo_key, item.number).await)
}

pub async fn submit_review(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    verdict: ReviewVerdict,
    body: &str,
    comments: &[DraftComment],
) -> Result<()> {
    // A review here takes its comments with it, so the whole thing is one request:
    // either all of it lands or none of it does, and there is no half state to
    // report on.
    http.request(
        &api(
            s,
            &format!("/repos/{}/pulls/{}/reviews", item.repo_key, item.number),
        ),
        request_options(Method::Post, &s.token)
            .json(forgejo_review_payload(verdict, body, comments)),
    )
    .await?;
    Ok(())
}

pub async fn add_comment(http: &Http, s: &Session, item: &ReviewItem, body: &str) -> Result<()> {
    http.request(
        &api(
            s,
            &format!("/repos/{}/issues/{}/comments", item.repo_key, item.number),
        ),
        request_options(Method::Post, &s.token).json(json!({ "body": body })),
    )
    .await?;
    Ok(())
}

pub async fn add_line_comment(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    draft: &LineCommentDraft,
    refs: &DiffRefs,
) -> Result<()> {
    // Forgejo attaches inline comments to a review rather than to the PR directly.
    inline_comment(http, s, item, refs.head_sha.as_deref(), draft.into()).await
}

/// A reply is another comment on the same line of the same file, because that is
/// what a conversation is here - so this is the same call `add_line_comment` makes,
/// aimed at the anchor the thread id carries.
pub async fn reply_to_thread(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    thread_id: &str,
    body: &str,
) -> Result<()> {
    let anchor = parse_forgejo_thread_id(thread_id)
        .ok_or_else(|| msg("That thread can no longer be replied to; reload and try again."))?;
    let placed = Placed {
        path: &anchor.path,
        body,
        new_line: (anchor.side == Side::New).then_some(anchor.line),
        old_line: (anchor.side == Side::Old).then_some(anchor.line),
        range: None,
    };
    inline_comment(http, s, item, None, placed).await
}

/// `encodeURIComponent`: everything but the RFC 3986 unreserved marks and the few
/// the JavaScript function leaves alone is percent-encoded, byte by byte.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{MockRequest, MockResponse};
    use crate::model::{Account, EdgeKind, LineRange, RangeEdge};
    use crate::providers::threads::forgejo_thread_id;
    use parking_lot::Mutex;
    use std::sync::Arc;

    const BASE: &str = "https://git.example.com/api/v1";

    type Log = Arc<Mutex<Vec<MockRequest>>>;

    /// Answers `"METHOD url"` keys from `routes` and 404s everything else, recording
    /// every request so a test can assert the exact sequence.
    fn mock(routes: Vec<(String, MockResponse)>) -> (Http, Log) {
        let routes: HashMap<String, MockResponse> = routes.into_iter().collect();
        let log: Log = Arc::default();
        let recorder = Arc::clone(&log);
        let http = Http::mock(move |request| {
            recorder.lock().push(request.clone());
            let key = format!("{} {}", request.method.as_str(), request.url);
            routes
                .get(&key)
                .cloned()
                .unwrap_or_else(|| MockResponse::json(404, &json!({ "message": "Not found" })))
        });
        (http, log)
    }

    fn get(path: &str, body: Value) -> (String, MockResponse) {
        (format!("GET {BASE}{path}"), MockResponse::json(200, &body))
    }

    fn get_text(path: &str, body: &str) -> (String, MockResponse) {
        (format!("GET {BASE}{path}"), MockResponse::new(200, body))
    }

    fn fail(method: &str, path: &str, status: u16, body: Value) -> (String, MockResponse) {
        (
            format!("{method} {BASE}{path}"),
            MockResponse::json(status, &body),
        )
    }

    /// A write the host accepts with an empty answer.
    fn ok_empty(method: &str, path: &str) -> (String, MockResponse) {
        (
            format!("{method} {BASE}{path}"),
            MockResponse::json(200, &json!({})),
        )
    }

    fn session() -> Session {
        Session {
            account: Account {
                id: "acc-1".into(),
                kind: ProviderKind::Forgejo,
                label: "Work Forgejo".into(),
                base_url: BASE.into(),
                web_url: "https://git.example.com".into(),
                username: "alice".into(),
                display_name: "Alice".into(),
                avatar_url: String::new(),
                added_at: "2026-08-01T10:00:00.000Z".into(),
                agent_command: None,
            },
            token: "abc".into(),
        }
    }

    fn draft(host: &str, label: &str) -> AccountDraft {
        AccountDraft {
            kind: ProviderKind::Forgejo,
            label: label.into(),
            host: host.into(),
            token: "abc".into(),
            username: None,
            agent_command: None,
        }
    }

    fn tracked_item() -> ReviewItem {
        ReviewItem {
            id: "acc-1:acme/app:12".into(),
            account_id: "acc-1".into(),
            provider: ProviderKind::Forgejo,
            repo_key: "acme/app".into(),
            repo: "acme/app".into(),
            number: 12,
            title: "Add search".into(),
            url: "https://git.example.com/acme/app/pulls/12".into(),
            author: User {
                name: "bob".into(),
                avatar_url: String::new(),
            },
            created_at: "2026-08-01T09:00:00Z".into(),
            updated_at: "2026-08-02T09:00:00Z".into(),
            draft: false,
            source_branch: "feature/search".into(),
            target_branch: "main".into(),
            labels: Vec::new(),
            my_review_state: MyReviewState::Pending,
            approvals: summarise_approvals(0, None),
            checks: summarise_checks(Vec::new()),
            additions: None,
            deletions: None,
            changed_files: None,
        }
    }

    fn comment_draft(new_line: Option<u32>, old_line: Option<u32>) -> DraftComment {
        DraftComment {
            id: "d1".into(),
            item_id: "acc-1:acme/app:12".into(),
            body: "Nit".into(),
            path: "src/search.ts".into(),
            new_line,
            old_line,
            range: None,
            created_at: "2026-08-02T11:00:00.000Z".into(),
            refs: DiffRefs {
                head_sha: Some("aaa111".into()),
                ..DiffRefs::default()
            },
        }
    }

    /// The pull request `acme/app#12` as Forgejo answers it.
    fn app_pull() -> Value {
        json!({
            "number": 12,
            "title": "Add search",
            "body": "Adds **search**.",
            "html_url": "https://git.example.com/acme/app/pulls/12",
            "draft": false,
            "created_at": "2026-08-01T09:00:00Z",
            "updated_at": "2026-08-02T09:00:00Z",
            "user": {"id": 2, "login": "bob", "full_name": "Bob", "avatar_url": "https://git.example.com/avatars/bob"},
            "head": {"ref": "feature/search", "sha": "aaa111", "repo": {"id": 7, "name": "app"}},
            "base": {"ref": "main", "sha": "bbb222"},
            "additions": 40,
            "deletions": 3,
            "changed_files": 4,
            "labels": [{"name": "backend"}, {"name": "needs-review"}]
        })
    }

    /// `acme/tools#3`, with only the fields Forgejo always sends.
    fn tools_pull() -> Value {
        json!({
            "title": "Bump deps",
            "html_url": "https://git.example.com/acme/tools/pulls/3",
            "user": {"login": "carol"},
            "head": {"ref": "bump", "sha": "ccc333"},
            "base": {"ref": "release/*"},
            "created_at": "2026-08-03T09:00:00Z",
            "updated_at": "2026-08-03T10:00:00Z"
        })
    }

    fn app_reviews() -> Value {
        json!([
            {"id": 0, "state": "APPROVED", "user": {"login": "alice"}, "dismissed": false, "comments_count": 0},
            {"id": 1, "state": "APPROVED", "user": {"login": "dana"}, "dismissed": false, "comments_count": 1},
            {"id": 2, "state": "REQUEST_CHANGES", "user": {"login": "alice"}, "dismissed": false, "comments_count": 2},
            {"id": 3, "state": "APPROVED", "user": {"login": "erin"}, "dismissed": true, "comments_count": 0},
            {"id": 4, "state": "COMMENT", "user": {"login": "frank"}, "dismissed": false, "comments_count": 0}
        ])
    }

    fn app_diff() -> &'static str {
        "diff --git a/src/search.ts b/src/search.ts\n\
index 1111..2222 100644\n\
--- a/src/search.ts\n\
+++ b/src/search.ts\n\
@@ -1,2 +1,3 @@\n\
 keep\n\
-old\n\
+new\n\
+extra\n\
diff --git a/docs/old.md b/docs/new.md\n\
similarity index 90%\n\
rename from docs/old.md\n\
rename to docs/new.md\n\
diff --git a/logo.png b/logo.png\n\
new file mode 100644\n\
index 0000000..3333\n\
Binary files /dev/null and b/logo.png differ\n"
    }

    /// Everything `list_review_requests` reads for the two fixture pull requests.
    fn listing_routes() -> Vec<(String, MockResponse)> {
        vec![
            get(
                "/repos/issues/search?type=pulls&state=open&review_requested=true&limit=50&sort=recentupdate",
                json!([
                    {
                        "number": 12,
                        "title": "Add search",
                        "html_url": "https://git.example.com/acme/app/pulls/12",
                        "repository": {"id": 7, "name": "app", "owner": "acme", "full_name": "acme/app"}
                    },
                    {
                        "number": 3,
                        "title": "Bump deps",
                        "html_url": "https://git.example.com/acme/tools/pulls/3"
                    }
                ]),
            ),
            get("/repos/acme/app/pulls/12", app_pull()),
            get("/repos/acme/tools/pulls/3", tools_pull()),
            get(
                "/repos/acme/app/commits/aaa111/status",
                json!({
                    "state": "pending",
                    "statuses": [
                        {"id": 5, "status": "success", "context": "ci/build", "target_url": "https://ci.example/5", "description": "Built"},
                        {"id": 6, "status": "pending", "context": "", "target_url": "", "description": ""},
                        {"id": 7, "status": "warning", "context": "lint", "target_url": "", "description": "Style"}
                    ]
                }),
            ),
            // The tools repo's head has no statuses the token may read.
            get("/repos/acme/app/pulls/12/reviews", app_reviews()),
            get(
                "/repos/acme/tools/pulls/3/reviews",
                json!({"message": "forbidden"}),
            ),
            get(
                "/repos/acme/app/branch_protections/main",
                json!({
                    "rule_name": "main", "branch_name": "main", "required_approvals": 2
                }),
            ),
            fail(
                "GET",
                "/repos/acme/tools/branch_protections/release%2F*",
                403,
                json!({"message": "no admin"}),
            ),
        ]
    }

    // ----- connect ---------------------------------------------------------

    #[test]
    fn connect_resolves_the_identity_and_names_the_account() {
        let (http, log) = mock(vec![get(
            "/user",
            json!({"id": 2, "login": "alice", "full_name": "Alice Able", "avatar_url": "https://git.example.com/avatars/alice"}),
        )]);
        let account = futures::executor::block_on(connect(&http, &draft("git.example.com/", "")))
            .expect("connects");

        assert_eq!(account.kind, ProviderKind::Forgejo);
        assert_eq!(account.label, "git.example.com (alice)");
        assert_eq!(account.base_url, BASE);
        assert_eq!(account.web_url, "https://git.example.com");
        assert_eq!(account.username, "alice");
        assert_eq!(account.display_name, "Alice Able");
        assert_eq!(account.avatar_url, "https://git.example.com/avatars/alice");
        assert_eq!(account.agent_command, None);

        let requests = log.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, format!("{BASE}/user"));
        assert_eq!(requests[0].header("Authorization"), Some("token abc"));
        assert_eq!(requests[0].header("User-Agent"), Some("Reviewdeck"));
        assert_eq!(requests[0].header("Accept"), Some("application/json"));
    }

    #[test]
    fn connect_keeps_a_port_and_a_path_in_the_origin() {
        let base = "http://localhost:3000/gitea/api/v1";
        let (http, _) = mock(vec![(
            format!("GET {base}/user"),
            MockResponse::json(200, &json!({"login": "alice", "full_name": ""})),
        )]);
        let account = futures::executor::block_on(connect(
            &http,
            &draft("  http://localhost:3000/gitea/  ", ""),
        ))
        .expect("connects");

        assert_eq!(account.base_url, base);
        assert_eq!(account.web_url, "http://localhost:3000/gitea");
        assert_eq!(account.label, "localhost:3000 (alice)");
        // An empty full name falls back to the login, as `||` does.
        assert_eq!(account.display_name, "alice");
    }

    #[test]
    fn connect_uses_the_typed_label_when_there_is_one() {
        let (http, _) = mock(vec![get("/user", json!({"login": "alice"}))]);
        let account = futures::executor::block_on(connect(
            &http,
            &draft("git.example.com", "Personal Forgejo"),
        ))
        .expect("connects");
        assert_eq!(account.label, "Personal Forgejo");
    }

    #[test]
    fn connect_reports_a_rejected_token_in_the_user_facing_words() {
        let (http, _) = mock(vec![fail(
            "GET",
            "/user",
            401,
            json!({"message": "invalid token"}),
        )]);
        let error = futures::executor::block_on(connect(&http, &draft("git.example.com", "")))
            .expect_err("rejected");
        assert_eq!(
            error.to_string(),
            "Not authorised on git.example.com - the token is invalid or expired."
        );
    }

    #[test]
    fn connect_needs_a_host() {
        let (http, log) = mock(Vec::new());
        let error =
            futures::executor::block_on(connect(&http, &draft("  ", ""))).expect_err("no host");
        assert_eq!(error.to_string(), "A host is required.");
        assert!(log.lock().is_empty());
    }

    // ----- list_review_requests --------------------------------------------

    #[test]
    fn list_aggregates_every_field_of_each_review_item() {
        let (http, log) = mock(listing_routes());
        let items =
            futures::executor::block_on(list_review_requests(&http, &session())).expect("lists");

        assert_eq!(items.len(), 2);
        let app = &items[0];
        assert_eq!(app.id, "acc-1:acme/app:12");
        assert_eq!(app.account_id, "acc-1");
        assert_eq!(app.provider, ProviderKind::Forgejo);
        assert_eq!(app.repo_key, "acme/app");
        assert_eq!(app.repo, "acme/app");
        assert_eq!(app.number, 12);
        assert_eq!(app.title, "Add search");
        assert_eq!(app.url, "https://git.example.com/acme/app/pulls/12");
        assert_eq!(app.author.name, "bob");
        assert_eq!(app.author.avatar_url, "https://git.example.com/avatars/bob");
        assert_eq!(app.created_at, "2026-08-01T09:00:00Z");
        assert_eq!(app.updated_at, "2026-08-02T09:00:00Z");
        assert!(!app.draft);
        assert_eq!(app.source_branch, "feature/search");
        assert_eq!(app.target_branch, "main");
        assert_eq!(app.labels, vec!["backend", "needs-review"]);
        // alice's last verdict is "request changes"; dana's approval counts, erin's
        // dismissed one does not.
        assert_eq!(app.my_review_state, MyReviewState::ChangesRequested);
        assert_eq!(app.approvals.given, 1);
        assert_eq!(app.approvals.required, Some(2));
        assert_eq!(
            app.approvals.outcome,
            crate::model::ApprovalOutcome::Pending
        );
        assert_eq!(app.checks.status, CheckStatus::Running);
        assert_eq!(app.checks.passed, 1);
        assert_eq!(app.checks.failed, 0);
        assert_eq!(app.checks.running, 1);
        assert_eq!(app.checks.total, 3);
        assert_eq!(app.checks.runs.len(), 3);
        assert_eq!(app.additions, Some(40));
        assert_eq!(app.deletions, Some(3));
        assert_eq!(app.changed_files, Some(4));

        let tools = &items[1];
        assert_eq!(tools.id, "acc-1:acme/tools:3");
        assert_eq!(tools.repo, "acme/tools");
        assert_eq!(tools.title, "Bump deps");
        assert_eq!(tools.author.name, "carol");
        assert_eq!(tools.author.avatar_url, "");
        assert!(!tools.draft);
        assert!(tools.labels.is_empty());
        assert_eq!(tools.additions, None);
        assert_eq!(tools.deletions, None);
        assert_eq!(tools.changed_files, None);
        // No review access and no branch protection: nothing asked, nothing given.
        assert_eq!(tools.my_review_state, MyReviewState::Pending);
        assert_eq!(tools.approvals.given, 0);
        assert_eq!(tools.approvals.required, None);
        assert_eq!(
            tools.approvals.outcome,
            crate::model::ApprovalOutcome::NoneRequired
        );
        // Its head commit has no status endpoint answer, so the checks are empty.
        assert_eq!(tools.checks.status, CheckStatus::Unknown);
        assert_eq!(tools.checks.total, 0);

        let requests = log.lock();
        let search = requests
            .iter()
            .filter(|request| request.url.contains("/repos/issues/search"))
            .count();
        assert_eq!(search, 1, "the search is one round trip");
        assert!(requests.iter().all(|request| {
            request.header("Authorization") == Some("token abc")
                && request.header("User-Agent") == Some("Reviewdeck")
        }));
    }

    #[test]
    fn list_reads_each_branch_protection_once() {
        // Two open pull requests on the same branch of the same repo.
        let (http, log) = mock(vec![
            get(
                "/repos/issues/search?type=pulls&state=open&review_requested=true&limit=50&sort=recentupdate",
                json!([
                    {"number": 12, "html_url": "https://git.example.com/acme/app/pulls/12", "repository": {"full_name": "acme/app"}},
                    {"number": 13, "html_url": "https://git.example.com/acme/app/pulls/13", "repository": {"full_name": "acme/app"}}
                ]),
            ),
            get("/repos/acme/app/pulls/12", app_pull()),
            get("/repos/acme/app/pulls/13", app_pull()),
            get(
                "/repos/acme/app/commits/aaa111/status",
                json!({"state": "success", "statuses": []}),
            ),
            get("/repos/acme/app/pulls/12/reviews", json!([])),
            get("/repos/acme/app/pulls/13/reviews", json!([])),
            get(
                "/repos/acme/app/branch_protections/main",
                json!({"required_approvals": 2}),
            ),
        ]);
        let items =
            futures::executor::block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.approvals.required == Some(2)));
        let protections = log
            .lock()
            .iter()
            .filter(|request| request.url.contains("/branch_protections/"))
            .count();
        assert_eq!(protections, 1, "one lookup per branch across the sync");
    }

    #[test]
    fn list_takes_a_combined_verdict_when_there_are_no_statuses() {
        let (http, _) = mock(vec![
            get(
                "/repos/issues/search?type=pulls&state=open&review_requested=true&limit=50&sort=recentupdate",
                json!([
                    {"number": 12, "html_url": "https://git.example.com/acme/app/pulls/12", "repository": {"full_name": "acme/app"}}
                ]),
            ),
            get("/repos/acme/app/pulls/12", app_pull()),
            get(
                "/repos/acme/app/commits/aaa111/status",
                json!({"state": "success", "statuses": []}),
            ),
            get("/repos/acme/app/pulls/12/reviews", json!([])),
            get(
                "/repos/acme/app/branch_protections/main",
                json!({"required_approvals": 0}),
            ),
        ]);
        let items =
            futures::executor::block_on(list_review_requests(&http, &session())).expect("lists");
        let checks = &items[0].checks;
        assert_eq!(checks.status, CheckStatus::Passed);
        assert_eq!(checks.total, 1);
        assert_eq!(checks.runs.len(), 1);
        assert_eq!(checks.runs[0].id, "combined");
        assert_eq!(checks.runs[0].name, "Checks");
        assert_eq!(checks.runs[0].url, None);
        assert_eq!(items[0].my_review_state, MyReviewState::Pending);
        assert_eq!(
            items[0].approvals.outcome,
            crate::model::ApprovalOutcome::NoneRequired
        );
    }

    #[test]
    fn list_gives_unknown_checks_for_an_unknown_combined_state() {
        let http = mock(vec![]).0;
        let checks = futures::executor::block_on(load_checks(&http, &session(), "acme/app", "zzz"));
        assert_eq!(checks.status, CheckStatus::Unknown);
        assert!(checks.runs.is_empty());
    }

    #[test]
    fn check_statuses_map_to_the_states_the_deck_shows() {
        let (http, _) = mock(vec![get(
            "/repos/acme/app/commits/aaa111/status",
            json!({
                "state": "failure",
                "statuses": [
                    {"id": "9", "status": "failure", "context": "test"},
                    {"id": 10, "status": "error", "context": "deploy", "target_url": "https://ci/10"},
                    {"status": "success"}
                ]
            }),
        )]);
        let checks =
            futures::executor::block_on(load_checks(&http, &session(), "acme/app", "aaa111"));
        assert_eq!(checks.status, CheckStatus::Failed);
        assert_eq!(checks.failed, 2);
        assert_eq!(checks.passed, 1);
        assert_eq!(checks.runs[0].id, "status-9");
        assert_eq!(checks.runs[1].url.as_deref(), Some("https://ci/10"));
        // A status without a context is named "check", and the missing id falls back to its position.
        assert_eq!(checks.runs[2].name, "check");
        assert_eq!(checks.runs[2].id, "status-2");
    }

    #[test]
    fn a_failing_checks_read_costs_only_the_checks() {
        let (http, _) = mock(vec![fail(
            "GET",
            "/repos/acme/app/commits/aaa111/status",
            500,
            json!({"message": "boom"}),
        )]);
        let checks =
            futures::executor::block_on(load_checks(&http, &session(), "acme/app", "aaa111"));
        assert_eq!(checks.status, CheckStatus::Unknown);
        assert_eq!(checks.total, 0);
    }

    #[test]
    fn a_failing_pull_request_read_fails_the_sync() {
        let mut routes = listing_routes();
        routes.retain(|(key, _)| !key.ends_with("/pulls/3"));
        routes.push(fail(
            "GET",
            "/repos/acme/tools/pulls/3",
            500,
            json!({"message": "boom"}),
        ));
        let (http, _) = mock(routes);
        let error = futures::executor::block_on(list_review_requests(&http, &session()))
            .expect_err("a pull request that will not load fails the sync");
        assert_eq!(
            error.to_string(),
            "git.example.com returned a server error (500)."
        );
    }

    #[test]
    fn a_missing_optional_field_never_fails_the_sync() {
        let (http, _) = mock(vec![
            get(
                "/repos/issues/search?type=pulls&state=open&review_requested=true&limit=50&sort=recentupdate",
                json!([{"number": 1, "repository": null}, {"title": "no number"}, "garbage"]),
            ),
            get(
                "/repos/unknown/unknown/pulls/1",
                json!({"title": null, "head": null, "base": null, "labels": null}),
            ),
        ]);
        let items = futures::executor::block_on(list_review_requests(&http, &session()))
            .expect("lists what it can");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].repo, "unknown/unknown");
        assert_eq!(items[0].source_branch, "");
        assert_eq!(items[0].target_branch, "");
        assert_eq!(items[0].title, "");
        assert_eq!(items[0].checks.status, CheckStatus::Unknown);
    }

    #[test]
    fn a_repo_is_named_from_the_web_address_without_a_repository_object() {
        let issue: FjIssue = serde_json::from_value(json!({
            "number": 4,
            "html_url": "https://git.example.com/acme/tools/pulls/4"
        }))
        .expect("parses");
        assert_eq!(repo_name(&issue), "acme/tools");

        let owned: FjIssue = serde_json::from_value(json!({
            "number": 4,
            "repository": {"name": "tools", "owner": "acme"}
        }))
        .expect("parses");
        assert_eq!(repo_name(&owned), "acme/tools");

        let nothing: FjIssue = serde_json::from_value(json!({"number": 4})).expect("parses");
        assert_eq!(repo_name(&nothing), "unknown/unknown");
    }

    // ----- load_detail -----------------------------------------------------

    #[test]
    fn load_detail_reads_the_pull_request_diff_threads_and_refs() {
        let (http, log) = mock(vec![
            get("/repos/acme/app/pulls/12", app_pull()),
            get_text("/repos/acme/app/pulls/12.diff", app_diff()),
            get(
                "/repos/acme/app/issues/12/comments",
                json!([
                    {"id": 90, "user": {"login": "dana", "avatar_url": "https://git.example.com/avatars/dana"}, "body": "Looks good overall", "created_at": "2026-08-02T10:00:00Z"}
                ]),
            ),
            get(
                "/repos/acme/app/pulls/12/reviews",
                json!([
                    {"id": 1, "state": "APPROVED", "user": {"login": "dana"}, "comments_count": 1},
                    {"id": 2, "state": "COMMENT", "user": {"login": "frank"}, "comments_count": 0}
                ]),
            ),
            get(
                "/repos/acme/app/pulls/12/reviews/1/comments",
                json!([
                    {"id": 501, "user": {"login": "dana"}, "body": "Rename this", "created_at": "2026-08-02T10:05:00Z", "path": "src/search.ts", "position": 2, "original_position": 0}
                ]),
            ),
        ]);
        let detail = futures::executor::block_on(load_detail(&http, &session(), &tracked_item()))
            .expect("loads");

        assert_eq!(detail.description, "Adds **search**.");
        assert_eq!(detail.refs.head_sha.as_deref(), Some("aaa111"));
        assert_eq!(detail.refs.base_sha.as_deref(), Some("bbb222"));
        assert_eq!(detail.refs.start_sha, None);

        // The pull request's own counts win over the diff's.
        assert_eq!(detail.item.additions, Some(40));
        assert_eq!(detail.item.deletions, Some(3));
        assert_eq!(detail.item.changed_files, Some(4));

        assert_eq!(detail.files.len(), 3);
        let search = &detail.files[0];
        assert_eq!(search.path, "src/search.ts");
        assert_eq!(search.status, crate::model::FileStatus::Modified);
        assert_eq!((search.additions, search.deletions), (2, 1));
        assert!(!search.binary);
        assert!(search.patch.is_some());
        let renamed = &detail.files[1];
        assert_eq!(renamed.path, "docs/new.md");
        assert_eq!(renamed.old_path, "docs/old.md");
        assert_eq!(renamed.status, crate::model::FileStatus::Renamed);
        let binary = &detail.files[2];
        assert_eq!(binary.path, "logo.png");
        assert_eq!(binary.status, crate::model::FileStatus::Added);
        assert!(binary.binary);
        assert_eq!(binary.patch, None);

        // The general comment and the inline one, which share no anchor.
        assert_eq!(detail.threads.len(), 2);
        assert!(
            detail
                .threads
                .iter()
                .any(|thread| thread.path.as_deref() == Some("src/search.ts"))
        );
        assert!(detail.threads.iter().any(|thread| {
            thread
                .comments
                .iter()
                .any(|comment| comment.body == "Looks good overall")
        }));

        let requests = log.lock();
        // A review with no comments is not asked about.
        assert!(
            !requests
                .iter()
                .any(|request| request.url.ends_with("/reviews/2/comments"))
        );
        assert!(
            requests
                .iter()
                .any(|request| request.url == format!("{BASE}/repos/acme/app/pulls/12.diff"))
        );
    }

    #[test]
    fn load_detail_counts_the_diff_when_the_pull_request_does_not_say() {
        let (http, _) = mock(vec![
            get("/repos/acme/tools/pulls/3", tools_pull()),
            get_text("/repos/acme/tools/pulls/3.diff", app_diff()),
            get("/repos/acme/tools/issues/3/comments", json!([])),
            get("/repos/acme/tools/pulls/3/reviews", json!([])),
        ]);
        let item = ReviewItem {
            id: "acc-1:acme/tools:3".into(),
            repo_key: "acme/tools".into(),
            repo: "acme/tools".into(),
            number: 3,
            ..tracked_item()
        };
        let detail =
            futures::executor::block_on(load_detail(&http, &session(), &item)).expect("loads");
        assert_eq!(detail.item.additions, Some(2));
        assert_eq!(detail.item.deletions, Some(1));
        assert_eq!(detail.item.changed_files, Some(3));
        assert_eq!(detail.description, "");
        assert_eq!(detail.refs.base_sha, None);
        assert!(detail.threads.is_empty());
    }

    #[test]
    fn a_diff_that_will_not_load_leaves_the_files_empty() {
        let (http, _) = mock(vec![
            get("/repos/acme/app/pulls/12", app_pull()),
            fail(
                "GET",
                "/repos/acme/app/pulls/12.diff",
                413,
                json!({"message": "too large"}),
            ),
        ]);
        let detail = futures::executor::block_on(load_detail(&http, &session(), &tracked_item()))
            .expect("the page still loads");
        assert!(detail.files.is_empty());
        assert_eq!(detail.item.additions, Some(40));
        assert_eq!(detail.item.changed_files, Some(4));
    }

    #[test]
    fn a_failing_pull_request_read_fails_the_detail() {
        let (http, _) = mock(vec![fail(
            "GET",
            "/repos/acme/app/pulls/12",
            404,
            json!({"message": "gone"}),
        )]);
        let error = futures::executor::block_on(load_detail(&http, &session(), &tracked_item()))
            .expect_err("no pull request, no detail");
        assert_eq!(
            error.to_string(),
            "Not found on git.example.com (/api/v1/repos/acme/app/pulls/12)."
        );
    }

    #[test]
    fn a_review_whose_comments_will_not_load_costs_only_its_own() {
        let (http, _) = mock(vec![
            get("/repos/acme/app/issues/12/comments", json!([])),
            get(
                "/repos/acme/app/pulls/12/reviews",
                json!([
                    {"id": 1, "user": {"login": "dana"}, "comments_count": 1},
                    {"id": 2, "user": {"login": "erin"}}
                ]),
            ),
            fail(
                "GET",
                "/repos/acme/app/pulls/12/reviews/1/comments",
                500,
                json!({}),
            ),
            get(
                "/repos/acme/app/pulls/12/reviews/2/comments",
                json!([
                    {"id": 7, "user": {"login": "erin"}, "body": "Still here", "created_at": "2026-08-02T10:05:00Z", "path": "src/a.ts", "position": 4}
                ]),
            ),
        ]);
        let threads = futures::executor::block_on(load_threads(&http, &session(), &tracked_item()))
            .expect("threads");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].path.as_deref(), Some("src/a.ts"));
        assert_eq!(threads[0].comments[0].body, "Still here");
    }

    #[test]
    fn refresh_checks_reads_the_head_of_the_pull_request_again() {
        let (http, log) = mock(vec![
            get("/repos/acme/app/pulls/12", app_pull()),
            get(
                "/repos/acme/app/commits/aaa111/status",
                json!({
                    "state": "success",
                    "statuses": [{"id": 1, "status": "success", "context": "ci"}]
                }),
            ),
        ]);
        let checks =
            futures::executor::block_on(refresh_checks(&http, &session(), &tracked_item()))
                .expect("refreshes");
        assert_eq!(checks.status, CheckStatus::Passed);
        assert_eq!(checks.total, 1);
        assert_eq!(log.lock().len(), 2);
    }

    // ----- submit_review ---------------------------------------------------

    #[test]
    fn approve_with_drafts_is_one_review_request() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/pulls/12/reviews")]);
        let mut comment = comment_draft(Some(2), None);
        comment.body = "Nit".into();
        futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Approve,
            "Thanks",
            &[comment],
        ))
        .expect("submits");

        let requests = log.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, Method::Post);
        assert_eq!(
            requests[0].url,
            format!("{BASE}/repos/acme/app/pulls/12/reviews")
        );
        assert_eq!(
            requests[0].json_body(),
            Some(json!({
                "event": "APPROVED",
                "body": "Thanks",
                "commit_id": "aaa111",
                "comments": [{"path": "src/search.ts", "body": "Nit", "new_position": 2, "old_position": 0}]
            }))
        );
        assert_eq!(requests[0].header("Content-Type"), Some("application/json"));
    }

    #[test]
    fn request_changes_without_drafts_says_something_and_sends_no_comments() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/pulls/12/reviews")]);
        futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::RequestChanges,
            "",
            &[],
        ))
        .expect("submits");
        let requests = log.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].json_body(),
            Some(json!({"event": "REQUEST_CHANGES", "body": "Reviewed.", "comments": []}))
        );
    }

    #[test]
    fn a_plain_comment_with_drafts_and_no_text_keeps_the_text_empty() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/pulls/12/reviews")]);
        futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Comment,
            "",
            &[comment_draft(None, Some(5))],
        ))
        .expect("submits");
        let body = log.lock()[0].json_body().expect("json");
        assert_eq!(body["event"], "COMMENT");
        assert_eq!(body["body"], "");
        assert_eq!(body["comments"][0]["old_position"], 5);
        assert_eq!(body["comments"][0]["new_position"], 0);
    }

    #[test]
    fn a_rejected_review_reports_the_hosts_own_sentence() {
        let (http, _) = mock(vec![fail(
            "POST",
            "/repos/acme/app/pulls/12/reviews",
            422,
            json!({"message": "You cannot approve your own pull request"}),
        )]);
        let error = futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Approve,
            "",
            &[],
        ))
        .expect_err("rejected");
        assert_eq!(
            error.to_string(),
            "You cannot approve your own pull request"
        );
    }

    // ----- comments and replies --------------------------------------------

    #[test]
    fn add_comment_posts_the_body_to_the_issue_thread() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/issues/12/comments")]);
        futures::executor::block_on(add_comment(&http, &session(), &tracked_item(), "Ship it"))
            .expect("posts");
        let requests = log.lock();
        assert_eq!(
            requests[0].url,
            format!("{BASE}/repos/acme/app/issues/12/comments")
        );
        assert_eq!(requests[0].json_body(), Some(json!({"body": "Ship it"})));
    }

    #[test]
    fn add_line_comment_is_a_review_comment_on_the_head_commit() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/pulls/12/reviews")]);
        let draft = LineCommentDraft {
            item_id: "acc-1:acme/app:12".into(),
            body: "Why?".into(),
            path: "src/search.ts".into(),
            new_line: Some(7),
            old_line: None,
            range: None,
        };
        let refs = DiffRefs {
            head_sha: Some("aaa111".into()),
            ..DiffRefs::default()
        };
        futures::executor::block_on(add_line_comment(
            &http,
            &session(),
            &tracked_item(),
            &draft,
            &refs,
        ))
        .expect("posts");
        assert_eq!(
            log.lock()[0].json_body(),
            Some(json!({
                "event": "COMMENT",
                "body": "",
                "commit_id": "aaa111",
                "comments": [{"path": "src/search.ts", "body": "Why?", "new_position": 7, "old_position": 0}]
            }))
        );
    }

    #[test]
    fn add_line_comment_leaves_the_commit_out_when_it_is_unknown() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/pulls/12/reviews")]);
        let draft = LineCommentDraft {
            item_id: "acc-1:acme/app:12".into(),
            body: "Range".into(),
            path: "src/search.ts".into(),
            new_line: Some(9),
            old_line: None,
            range: Some(LineRange {
                start_line: 7,
                start: RangeEdge {
                    kind: EdgeKind::Add,
                    old_pos: 3,
                    new_pos: 7,
                },
                end: RangeEdge {
                    kind: EdgeKind::Add,
                    old_pos: 5,
                    new_pos: 9,
                },
            }),
        };
        futures::executor::block_on(add_line_comment(
            &http,
            &session(),
            &tracked_item(),
            &draft,
            &DiffRefs::default(),
        ))
        .expect("posts");
        let body = log.lock()[0].json_body().expect("json");
        assert!(body.get("commit_id").is_none());
        // A range is anchored at its first line and counts the rest.
        assert_eq!(body["comments"][0]["new_position"], 7);
        assert_eq!(body["comments"][0]["extra_lines_count"], 2);
    }

    #[test]
    fn reply_to_thread_comments_on_the_same_anchor() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/pulls/12/reviews")]);
        let thread_id = forgejo_thread_id("src/search.ts", Side::New, 7);
        futures::executor::block_on(reply_to_thread(
            &http,
            &session(),
            &tracked_item(),
            &thread_id,
            "Agreed",
        ))
        .expect("replies");
        assert_eq!(
            log.lock()[0].json_body(),
            Some(json!({
                "event": "COMMENT",
                "body": "",
                "comments": [{"path": "src/search.ts", "body": "Agreed", "new_position": 7, "old_position": 0}]
            }))
        );
    }

    #[test]
    fn reply_to_an_old_side_thread_addresses_the_old_file() {
        let (http, log) = mock(vec![ok_empty("POST", "/repos/acme/app/pulls/12/reviews")]);
        let thread_id = forgejo_thread_id("src/a:b.ts", Side::Old, 12);
        futures::executor::block_on(reply_to_thread(
            &http,
            &session(),
            &tracked_item(),
            &thread_id,
            "Old",
        ))
        .expect("replies");
        let body = log.lock()[0].json_body().expect("json");
        assert_eq!(body["comments"][0]["path"], "src/a:b.ts");
        assert_eq!(body["comments"][0]["old_position"], 12);
        assert_eq!(body["comments"][0]["new_position"], 0);
    }

    #[test]
    fn reply_to_an_unknown_thread_sends_nothing() {
        let (http, log) = mock(Vec::new());
        let error = futures::executor::block_on(reply_to_thread(
            &http,
            &session(),
            &tracked_item(),
            "gh:12",
            "Hello",
        ))
        .expect_err("not a Forgejo thread");
        assert_eq!(
            error.to_string(),
            "That thread can no longer be replied to; reload and try again."
        );
        assert!(log.lock().is_empty());
    }

    #[test]
    fn encode_uri_component_matches_the_javascript_function() {
        assert_eq!(encode_uri_component("release/*"), "release%2F*");
        assert_eq!(encode_uri_component("feature/x y"), "feature%2Fx%20y");
        assert_eq!(encode_uri_component("ünï"), "%C3%BCn%C3%AF");
        assert_eq!(
            encode_uri_component("a-b_c.d!e~f*g'h(i)"),
            "a-b_c.d!e~f*g'h(i)"
        );
    }

    #[test]
    fn host_of_drops_only_the_default_port() {
        assert_eq!(host_of("https://git.example.com:443"), "git.example.com");
        assert_eq!(
            host_of("https://git.example.com:8443"),
            "git.example.com:8443"
        );
        assert_eq!(host_of("http://[::1]:3000"), "[::1]:3000");
    }
}
