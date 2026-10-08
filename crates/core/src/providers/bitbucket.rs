//! Port of src/main/providers/bitbucket.ts.
//!
//! Bitbucket Cloud, authenticated with an app password (username + password pair).
//!
//! Bitbucket has no "PRs awaiting my review" endpoint - `/pullrequests/{user}` only
//! returns PRs the user *authored*. So we walk the workspaces the token can see,
//! list each repo's open PRs, and keep the ones naming us as a reviewer. The repo
//! fan-out is bounded and runs with limited concurrency to stay polite.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::join;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::diff::parse_unified_diff;
use crate::error::{Error, Result, msg};
use crate::http::{Http, Method, RequestOptions, to_origin};
use crate::model::{
    AccountDraft, CheckRun, CheckStatus, CheckSummary, CommentThread, DiffRefs, DraftComment,
    LineCommentDraft, MyReviewState, NewAccount, ProviderKind, PullDetail, ReviewItem,
    ReviewVerdict, User, make_item_id, restriction_covers, summarise_approvals, summarise_checks,
};
use crate::providers::Session;
use crate::providers::limit_concurrency;
use crate::providers::submit::{Placed, bitbucket_comment_payload, submit_sequentially};
use crate::providers::threads::{BitbucketComment, bitbucket_threads};

const API_ROOT: &str = "https://api.bitbucket.org/2.0";
const WEB_ROOT: &str = "https://bitbucket.org";
/// Guard rail: a token with access to hundreds of repos should not stall a sync.
const MAX_REPOS: usize = 120;
/// Bounds the `next` walk, so a host that keeps handing back a next page cannot hang
/// a sync. Every list this adapter reads fits in a handful of pages.
const MAX_PAGES: usize = 100;
/// Concurrency limits from the TypeScript: repo fan-out, then per-PR reads.
const REPO_CONCURRENCY: usize = 6;
const CHECK_CONCURRENCY: usize = 6;

// ---------------------------------------------------------------------------
// The host shapes. Lenient like the threads: every field may be missing or null,
// and unknown fields are ignored.
// ---------------------------------------------------------------------------

/// One page of Bitbucket's `next`-URL pagination. Entries that do not read as their
/// shape are dropped, so one odd entry costs only itself.
#[derive(Debug, Deserialize)]
struct BbPage {
    values: Option<Vec<Value>>,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BbHref {
    href: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BbUserLinks {
    avatar: Option<BbHref>,
}

#[derive(Debug, Deserialize)]
struct BbUser {
    uuid: Option<String>,
    display_name: Option<String>,
    nickname: Option<String>,
    links: Option<BbUserLinks>,
}

impl BbUser {
    fn avatar(&self) -> String {
        self.links
            .as_ref()
            .and_then(|links| links.avatar.as_ref())
            .and_then(|avatar| avatar.href.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct BbWorkspace {
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BbRepo {
    full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BbParticipant {
    user: Option<BbUser>,
    role: Option<String>,
    approved: Option<bool>,
    state: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct BbBranchRestriction {
    kind: Option<String>,
    pattern: Option<String>,
    branch_match_kind: Option<String>,
    /// A whole number on the host; read as a float so a `2.0` still counts.
    value: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct BbNamed {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BbCommit {
    hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BbBranchRef {
    branch: Option<BbNamed>,
    commit: Option<BbCommit>,
}

#[derive(Debug, Deserialize)]
struct BbPrLinks {
    html: Option<BbHref>,
}

#[derive(Debug, Deserialize)]
struct BbPullRequest {
    id: Option<u64>,
    title: Option<String>,
    description: Option<String>,
    created_on: Option<String>,
    updated_on: Option<String>,
    author: Option<BbUser>,
    reviewers: Option<Vec<BbUser>>,
    participants: Option<Vec<BbParticipant>>,
    source: Option<BbBranchRef>,
    destination: Option<BbBranchRef>,
    links: Option<BbPrLinks>,
    draft: Option<bool>,
}

impl BbPullRequest {
    fn source_branch(&self) -> String {
        branch_name(self.source.as_ref())
    }

    fn target_branch(&self) -> String {
        branch_name(self.destination.as_ref())
    }

    fn source_hash(&self) -> Option<String> {
        commit_hash(self.source.as_ref())
    }

    fn destination_hash(&self) -> Option<String> {
        commit_hash(self.destination.as_ref())
    }
}

fn branch_name(reference: Option<&BbBranchRef>) -> String {
    reference
        .and_then(|reference| reference.branch.as_ref())
        .and_then(|branch| branch.name.clone())
        .unwrap_or_default()
}

fn commit_hash(reference: Option<&BbBranchRef>) -> Option<String> {
    reference
        .and_then(|reference| reference.commit.as_ref())
        .and_then(|commit| commit.hash.clone())
}

#[derive(Debug, Deserialize)]
struct BbStatus {
    key: Option<String>,
    name: Option<String>,
    state: Option<String>,
    url: Option<String>,
    description: Option<String>,
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// The credentials a request is made with. Bitbucket's app passwords are HTTP Basic,
/// with the account's username alongside them.
#[derive(Clone, Copy)]
struct Creds<'a> {
    username: &'a str,
    token: &'a str,
}

impl Creds<'_> {
    fn options(self, method: Method) -> RequestOptions {
        let basic = STANDARD.encode(format!("{}:{}", self.username, self.token));
        RequestOptions::new(method).headers([
            ("Authorization", format!("Basic {basic}")),
            ("Accept", "application/json".to_string()),
            ("User-Agent", "Reviewdeck".to_string()),
        ])
    }
}

fn creds(s: &Session) -> Creds<'_> {
    Creds {
        username: &s.account.username,
        token: &s.token,
    }
}

/// Entries that do not read as their shape are dropped.
fn lenient<T: DeserializeOwned>(values: Vec<Value>) -> Vec<T> {
    values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

/// Walks Bitbucket's `next`-URL pagination, bounded by `max` items.
async fn collect<T: DeserializeOwned>(
    http: &Http,
    creds: Creds<'_>,
    url: &str,
    max: usize,
) -> Result<Vec<T>> {
    let mut out: Vec<T> = Vec::new();
    let mut next = Some(url.to_string());
    let mut pages = 0;
    while let Some(current) = next.take() {
        if out.len() >= max || pages >= MAX_PAGES {
            break;
        }
        pages += 1;
        let page: BbPage = http.json(&current, creds.options(Method::Get)).await?;
        out.extend(lenient(page.values.unwrap_or_default()));
        next = page.next.filter(|next| !next.is_empty());
    }
    out.truncate(max);
    Ok(out)
}

/// The whole conversation, which Bitbucket keeps as one flat list of comments. A
/// read that fails costs the conversation, not the page.
async fn fetch_threads(
    http: &Http,
    creds: Creds<'_>,
    repo: &str,
    number: u64,
) -> Vec<CommentThread> {
    let comments: Vec<BitbucketComment> = collect(
        http,
        creds,
        &format!("{API_ROOT}/repositories/{repo}/pullrequests/{number}/comments?pagelen=50"),
        100,
    )
    .await
    .unwrap_or_default();
    bitbucket_threads(&comments)
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

fn status_state(state: &str) -> CheckStatus {
    match state {
        "SUCCESSFUL" => CheckStatus::Passed,
        "FAILED" => CheckStatus::Failed,
        "INPROGRESS" => CheckStatus::Running,
        "STOPPED" => CheckStatus::Failed,
        _ => CheckStatus::Unknown,
    }
}

/// The build statuses on a pull request's head. Any failure reads as no checks.
async fn load_checks(http: &Http, creds: Creds<'_>, repo: &str, id: u64) -> CheckSummary {
    let statuses: Result<Vec<BbStatus>> = collect(
        http,
        creds,
        &format!("{API_ROOT}/repositories/{repo}/pullrequests/{id}/statuses?pagelen=50"),
        50,
    )
    .await;
    match statuses {
        Ok(statuses) => {
            let runs: Vec<CheckRun> = statuses
                .into_iter()
                .map(|status| {
                    let key = status.key.unwrap_or_default();
                    CheckRun {
                        id: format!("status-{key}"),
                        name: non_empty(status.name).unwrap_or_else(|| key.clone()),
                        status: status_state(status.state.as_deref().unwrap_or_default()),
                        url: non_empty(status.url),
                        description: non_empty(status.description),
                    }
                })
                .collect();
            summarise_checks(runs)
        }
        Err(_) => empty_checks(),
    }
}

/// JavaScript's `x || undefined` for an optional string.
fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.is_empty())
}

/// The approval counts a repository's branch restrictions ask for, keyed by
/// pattern. Reading restrictions takes repository admin, which a reviewer rarely
/// has; the failure reads as a repository that asks for nothing, which is all the
/// app can honestly say. One call per repository, whatever it holds.
async fn load_restrictions(http: &Http, creds: Creds<'_>, repo: &str) -> Vec<BbBranchRestriction> {
    collect(
        http,
        creds,
        &format!(
            "{API_ROOT}/repositories/{repo}/branch-restrictions?kind=require_approvals_to_merge&pagelen=50"
        ),
        50,
    )
    .await
    .unwrap_or_default()
}

/// The approvals a branch asks for: the largest value of the matching rules, or
/// nothing when no rule covers it.
fn required_for(restrictions: &[BbBranchRestriction], branch: &str) -> Option<u32> {
    let mut required: Option<u32> = None;
    for restriction in restrictions {
        if restriction.kind.as_deref() != Some("require_approvals_to_merge") {
            continue;
        }
        if restriction.branch_match_kind.as_deref() == Some("branching_model") {
            continue;
        }
        let Some(pattern) = non_empty(restriction.pattern.clone()) else {
            continue;
        };
        if !restriction_covers(&pattern, branch) {
            continue;
        }
        // Saturating float-to-int cast: negatives and NaN read as zero.
        let value = restriction.value.unwrap_or(0.0) as u32;
        required = Some(required.unwrap_or(0).max(value));
    }
    required
}

/// The token's own standing on a pull request, from its participant record.
fn review_state_for(pr: &BbPullRequest, uuid: &str) -> MyReviewState {
    let me = pr.participants.iter().flatten().find(|participant| {
        participant
            .user
            .as_ref()
            .and_then(|user| user.uuid.as_deref())
            == Some(uuid)
    });
    let Some(me) = me else {
        return MyReviewState::Pending;
    };
    if me.state.as_deref() == Some("approved") || me.approved == Some(true) {
        return MyReviewState::Approved;
    }
    if me.state.as_deref() == Some("changes_requested") {
        return MyReviewState::ChangesRequested;
    }
    MyReviewState::Pending
}

fn approvers_of(pr: &BbPullRequest) -> u32 {
    pr.participants
        .iter()
        .flatten()
        .filter(|participant| participant.approved == Some(true))
        .count() as u32
}

/// Whether the token is named as a reviewer, by the reviewer list or by role.
fn is_my_review(pr: &BbPullRequest, uuid: &str) -> bool {
    if pr
        .reviewers
        .iter()
        .flatten()
        .any(|reviewer| reviewer.uuid.as_deref() == Some(uuid))
    {
        return true;
    }
    pr.participants.iter().flatten().any(|participant| {
        participant.role.as_deref() == Some("REVIEWER")
            && participant
                .user
                .as_ref()
                .and_then(|user| user.uuid.as_deref())
                == Some(uuid)
    })
}

/// A pull request that names the token as a reviewer, with what the repository asks.
struct Candidate {
    repo: String,
    number: u64,
    pr: BbPullRequest,
    restrictions: Vec<BbBranchRestriction>,
}

fn review_item(
    account_id: &str,
    me_uuid: &str,
    candidate: Candidate,
    checks: CheckSummary,
) -> ReviewItem {
    let Candidate {
        repo,
        number,
        pr,
        restrictions,
    } = candidate;
    let target = pr.target_branch();
    let author = pr.author.as_ref();
    ReviewItem {
        id: make_item_id(account_id, &repo, number),
        account_id: account_id.to_string(),
        provider: ProviderKind::Bitbucket,
        repo_key: repo.clone(),
        url: pr
            .links
            .as_ref()
            .and_then(|links| links.html.as_ref())
            .and_then(|html| html.href.clone())
            .unwrap_or_else(|| format!("{WEB_ROOT}/{repo}/pull-requests/{number}")),
        repo,
        number,
        title: pr.title.clone().unwrap_or_default(),
        author: User {
            name: author
                .and_then(|author| author.display_name.clone())
                .unwrap_or_else(|| "unknown".to_string()),
            avatar_url: author.map(BbUser::avatar).unwrap_or_default(),
        },
        created_at: pr.created_on.clone().unwrap_or_default(),
        updated_at: pr.updated_on.clone().unwrap_or_default(),
        draft: pr.draft.unwrap_or(false),
        source_branch: pr.source_branch(),
        approvals: summarise_approvals(approvers_of(&pr), required_for(&restrictions, &target)),
        target_branch: target,
        labels: Vec::new(),
        my_review_state: review_state_for(&pr, me_uuid),
        checks,
        additions: None,
        deletions: None,
        changed_files: None,
    }
}

/// One inline comment, on the new file or the old one.
async fn post_comment(
    http: &Http,
    creds: Creds<'_>,
    item: &ReviewItem,
    comment: Placed<'_>,
) -> Result<()> {
    http.request(
        &pull_request_url(item, "/comments"),
        creds
            .options(Method::Post)
            .json(bitbucket_comment_payload(comment)),
    )
    .await?;
    Ok(())
}

/// Where the pull request's own resources live.
fn pull_request_url(item: &ReviewItem, suffix: &str) -> String {
    format!(
        "{API_ROOT}/repositories/{}/pullrequests/{}{suffix}",
        item.repo_key, item.number
    )
}

pub async fn connect(http: &Http, draft: &AccountDraft) -> Result<NewAccount> {
    let username = match draft.username.as_deref().filter(|name| !name.is_empty()) {
        Some(username) => username,
        None => {
            return Err(msg(
                "Bitbucket needs the username the app password belongs to.",
            ));
        }
    };
    // Bitbucket Cloud is the only host; a custom host means Bitbucket Server, which is a different API.
    if !draft.host.is_empty() {
        let origin = to_origin(&draft.host)?;
        if !origin.to_ascii_lowercase().contains("bitbucket.org") {
            return Err(msg(
                "Only Bitbucket Cloud (bitbucket.org) is supported right now.",
            ));
        }
    }
    let creds = Creds {
        username,
        token: &draft.token,
    };
    let user: BbUser = http
        .json(&format!("{API_ROOT}/user"), creds.options(Method::Get))
        .await?;
    let nickname = user
        .nickname
        .clone()
        .or_else(|| user.display_name.clone())
        .unwrap_or_default();
    let label = if draft.label.is_empty() {
        format!("Bitbucket ({nickname})")
    } else {
        draft.label.clone()
    };
    Ok(NewAccount {
        kind: ProviderKind::Bitbucket,
        label,
        base_url: API_ROOT.to_string(),
        web_url: WEB_ROOT.to_string(),
        // The uuid is what PR payloads identify people by, so keep it as the username.
        username: username.to_string(),
        display_name: user.display_name.clone().unwrap_or_default(),
        avatar_url: user.avatar(),
        agent_command: None,
    })
}

pub async fn list_review_requests(http: &Http, s: &Session) -> Result<Vec<ReviewItem>> {
    let c = creds(s);
    let me: BbUser = http
        .json(&format!("{API_ROOT}/user"), c.options(Method::Get))
        .await?;
    // Without a uuid nothing can be matched to the token, so nothing is listed.
    let me_uuid = me.uuid.clone().unwrap_or_default();
    let workspaces: Vec<BbWorkspace> = collect(
        http,
        c,
        &format!("{API_ROOT}/workspaces?pagelen=50&fields=values.slug,values.name,next"),
        50,
    )
    .await?;

    let mut repos: Vec<String> = Vec::new();
    for workspace in workspaces {
        if repos.len() >= MAX_REPOS {
            break;
        }
        let Some(slug) = workspace.slug else {
            continue;
        };
        let found: Vec<BbRepo> = collect(
            http,
            c,
            &format!(
                "{API_ROOT}/repositories/{slug}?role=member&sort=-updated_on&pagelen=50\
                 &fields=values.full_name,values.slug,values.workspace.slug,values.updated_on,next"
            ),
            MAX_REPOS - repos.len(),
        )
        .await
        .unwrap_or_default();
        repos.extend(found.into_iter().filter_map(|repo| repo.full_name));
    }

    let me_ref = me_uuid.as_str();
    let per_repo: Vec<Vec<Candidate>> = limit_concurrency(repos, REPO_CONCURRENCY, move |repo| {
        async move {
            let pulls: Vec<BbPullRequest> = collect(
                http,
                c,
                &format!("{API_ROOT}/repositories/{repo}/pullrequests?state=OPEN&pagelen=50"),
                50,
            )
            .await
            .unwrap_or_default();
            let mine: Vec<(u64, BbPullRequest)> = pulls
                .into_iter()
                .filter_map(|pr| {
                    let number = pr.id?;
                    let by_someone_else =
                        pr.author.as_ref().and_then(|author| author.uuid.as_deref())
                            != Some(me_ref);
                    (is_my_review(&pr, me_ref) && by_someone_else).then_some((number, pr))
                })
                .collect();
            if mine.is_empty() {
                // Nothing to ask the repository about.
                return Vec::new();
            }
            let restrictions = load_restrictions(http, c, &repo).await;
            mine.into_iter()
                .map(|(number, pr)| Candidate {
                    repo: repo.clone(),
                    number,
                    pr,
                    restrictions: restrictions.clone(),
                })
                .collect()
        }
    })
    .await;

    let candidates: Vec<Candidate> = per_repo.into_iter().flatten().collect();
    let items = limit_concurrency(candidates, CHECK_CONCURRENCY, move |candidate| async move {
        let checks = load_checks(http, c, &candidate.repo, candidate.number).await;
        review_item(&s.account.id, me_ref, candidate, checks)
    })
    .await;
    Ok(items)
}

pub async fn load_detail(http: &Http, s: &Session, item: &ReviewItem) -> Result<PullDetail> {
    let c = creds(s);
    let pr_url = pull_request_url(item, "");
    let (pr, diff, threads) = join!(
        http.json::<BbPullRequest>(&pr_url, c.options(Method::Get)),
        // A diff that will not load leaves the files empty rather than failing the page.
        async {
            http.text(&format!("{pr_url}/diff"), c.options(Method::Get))
                .await
                .unwrap_or_default()
        },
        fetch_threads(http, c, &item.repo_key, item.number),
    );
    let pr = pr?;

    let files = parse_unified_diff(&diff);
    let (added, removed) = files.iter().fold((0u32, 0u32), |(added, removed), file| {
        (
            added.saturating_add(file.additions),
            removed.saturating_add(file.deletions),
        )
    });

    let mut detail_item = item.clone();
    detail_item.additions = Some(added);
    detail_item.deletions = Some(removed);
    detail_item.changed_files = Some(u32::try_from(files.len()).unwrap_or(u32::MAX));

    Ok(PullDetail {
        item: detail_item,
        description: pr.description.clone().unwrap_or_default(),
        files,
        threads,
        refs: DiffRefs {
            head_sha: pr.source_hash(),
            base_sha: pr.destination_hash(),
            start_sha: None,
        },
    })
}

pub async fn refresh_checks(http: &Http, s: &Session, item: &ReviewItem) -> Result<CheckSummary> {
    Ok(load_checks(http, creds(s), &item.repo_key, item.number).await)
}

pub async fn load_threads(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
) -> Result<Vec<CommentThread>> {
    Ok(fetch_threads(http, creds(s), &item.repo_key, item.number).await)
}

pub async fn submit_review(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    verdict: ReviewVerdict,
    body: &str,
    comments: &[DraftComment],
) -> Result<()> {
    let c = creds(s);
    // No batch call at all here, so the comments go one at a time and the verdict
    // last - reported honestly if it stops part-way.
    let base = pull_request_url(item, "");

    submit_sequentially(
        comments,
        move |comment| post_comment(http, c, item, Placed::from(comment)),
        move || async move {
            match verdict {
                ReviewVerdict::Approve => {
                    // Clear any standing "request changes" first, otherwise both flags linger.
                    let _ = http
                        .request(
                            &format!("{base}/request-changes"),
                            c.options(Method::Delete),
                        )
                        .await;
                    http.request(&format!("{base}/approve"), c.options(Method::Post))
                        .await?;
                }
                ReviewVerdict::RequestChanges => {
                    let _ = http
                        .request(&format!("{base}/approve"), c.options(Method::Delete))
                        .await;
                    http.request(&format!("{base}/request-changes"), c.options(Method::Post))
                        .await?;
                }
                ReviewVerdict::Comment => {}
            }
            if !body.is_empty() {
                add_comment(http, s, item, body).await?;
            }
            Ok::<(), Error>(())
        },
    )
    .await
}

pub async fn add_comment(http: &Http, s: &Session, item: &ReviewItem, body: &str) -> Result<()> {
    http.request(
        &pull_request_url(item, "/comments"),
        creds(s)
            .options(Method::Post)
            .json(json!({ "content": { "raw": body } })),
    )
    .await?;
    Ok(())
}

pub async fn add_line_comment(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    draft: &LineCommentDraft,
    _refs: &DiffRefs,
) -> Result<()> {
    post_comment(http, creds(s), item, Placed::from(draft)).await
}

/// A reply is an ordinary comment naming the one that opened the thread.
pub async fn reply_to_thread(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    thread_id: &str,
    body: &str,
) -> Result<()> {
    // `Number(threadId)` in the TypeScript: an empty id is 0 and a non-numeric one
    // goes out as JSON null (NaN).
    let trimmed = thread_id.trim();
    let parent_id = if trimmed.is_empty() {
        Value::from(0)
    } else {
        trimmed
            .parse::<u64>()
            .map(Value::from)
            .unwrap_or(Value::Null)
    };
    http.request(
        &pull_request_url(item, "/comments"),
        creds(s)
            .options(Method::Post)
            .json(json!({ "content": { "raw": body }, "parent": { "id": parent_id } })),
    )
    .await?;
    Ok(())
}

/// Resolving posts to the thread's opening comment; reopening deletes the same.
pub async fn set_thread_resolved(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    thread_id: &str,
    resolved: bool,
) -> Result<()> {
    let method = if resolved {
        Method::Post
    } else {
        Method::Delete
    };
    http.request(
        &pull_request_url(
            item,
            &format!("/comments/{}/resolve", encode_uri_component(thread_id)),
        ),
        creds(s).options(method),
    )
    .await?;
    Ok(())
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
    use crate::model::{Account, ApprovalOutcome, EdgeKind, LineRange, RangeEdge};
    use parking_lot::Mutex;
    use std::collections::HashMap;
    use std::sync::Arc;

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
            routes.get(&key).cloned().unwrap_or_else(|| {
                MockResponse::json(404, &json!({ "error": { "message": "Not found" } }))
            })
        });
        (http, log)
    }

    fn get(url: &str, body: Value) -> (String, MockResponse) {
        (format!("GET {url}"), MockResponse::json(200, &body))
    }

    fn get_text(url: &str, body: &str) -> (String, MockResponse) {
        (format!("GET {url}"), MockResponse::new(200, body))
    }

    fn with_status(method: &str, url: &str, status: u16, body: Value) -> (String, MockResponse) {
        (format!("{method} {url}"), MockResponse::json(status, &body))
    }

    /// A write the host accepts with an empty answer.
    fn ok_empty(method: &str, url: &str) -> (String, MockResponse) {
        with_status(method, url, 200, json!({}))
    }

    fn session() -> Session {
        Session {
            account: Account {
                id: "acc-bb".into(),
                kind: ProviderKind::Bitbucket,
                label: "Bitbucket".into(),
                base_url: API_ROOT.into(),
                web_url: WEB_ROOT.into(),
                username: "alice-dev".into(),
                display_name: "Alice".into(),
                avatar_url: String::new(),
                added_at: "2026-08-01T10:00:00.000Z".into(),
                agent_command: None,
            },
            token: "app-pass".into(),
        }
    }

    /// The Basic credential the tests expect: `alice-dev:app-pass`.
    const AUTH: &str = "Basic YWxpY2UtZGV2OmFwcC1wYXNz";

    fn draft(host: &str, username: Option<&str>, label: &str) -> AccountDraft {
        AccountDraft {
            kind: ProviderKind::Bitbucket,
            label: label.into(),
            host: host.into(),
            token: "app-pass".into(),
            username: username.map(str::to_owned),
            agent_command: None,
        }
    }

    fn tracked_item() -> ReviewItem {
        ReviewItem {
            id: "acc-bb:acme/web:42".into(),
            account_id: "acc-bb".into(),
            provider: ProviderKind::Bitbucket,
            repo_key: "acme/web".into(),
            repo: "acme/web".into(),
            number: 42,
            title: "Checkout".into(),
            url: "https://bitbucket.org/acme/web/pull-requests/42".into(),
            author: User {
                name: "Bob".into(),
                avatar_url: String::new(),
            },
            created_at: "2026-08-01T09:00:00Z".into(),
            updated_at: "2026-08-02T09:00:00Z".into(),
            draft: false,
            source_branch: "feature/checkout".into(),
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

    fn draft_comment(
        id: &str,
        body: &str,
        new_line: Option<u32>,
        old_line: Option<u32>,
    ) -> DraftComment {
        DraftComment {
            id: id.into(),
            item_id: "acc-bb:acme/web:42".into(),
            body: body.into(),
            path: "src/cart.ts".into(),
            new_line,
            old_line,
            range: None,
            created_at: "2026-08-02T11:00:00.000Z".into(),
            refs: DiffRefs::default(),
        }
    }

    const PR_URL: &str = "https://api.bitbucket.org/2.0/repositories/acme/web/pullrequests/42";
    const COMMENTS_URL: &str =
        "https://api.bitbucket.org/2.0/repositories/acme/web/pullrequests/42/comments";

    fn me() -> Value {
        json!({
            "uuid": "{me-uuid}",
            "display_name": "Alice Able",
            "nickname": "alice",
            "links": {"avatar": {"href": "https://bitbucket.org/account/alice/avatar/"}}
        })
    }

    /// The pull request `acme/web#42`, reviewed by the token, opened by someone else.
    fn checkout_pull() -> Value {
        json!({
            "id": 42,
            "title": "Checkout",
            "description": "Adds **checkout**.",
            "state": "OPEN",
            "created_on": "2026-08-01T09:00:00.000000+00:00",
            "updated_on": "2026-08-02T09:00:00.000000+00:00",
            "author": {"uuid": "{bob-uuid}", "display_name": "Bob", "links": {"avatar": {"href": "https://bitbucket.org/avatars/bob"}}},
            "reviewers": [{"uuid": "{me-uuid}", "display_name": "Alice Able"}],
            "participants": [
                {"user": {"uuid": "{bob-uuid}", "display_name": "Bob"}, "role": "PARTICIPANT", "approved": false, "state": null},
                {"user": {"uuid": "{carol-uuid}", "display_name": "Carol"}, "role": "PARTICIPANT", "approved": true, "state": "approved"},
                {"user": {"uuid": "{me-uuid}", "display_name": "Alice Able"}, "role": "REVIEWER", "approved": false, "state": "changes_requested"}
            ],
            "source": {"branch": {"name": "feature/checkout"}, "commit": {"hash": "aaa111"}},
            "destination": {"branch": {"name": "main"}, "commit": {"hash": "bbb222"}},
            "links": {"html": {"href": "https://bitbucket.org/acme/web/pull-requests/42"}}
        })
    }

    fn cart_diff() -> &'static str {
        "diff --git a/src/cart.ts b/src/cart.ts\n\
index 1111..2222 100644\n\
--- a/src/cart.ts\n\
+++ b/src/cart.ts\n\
@@ -1,2 +1,3 @@\n\
 keep\n\
-old\n\
+new\n\
+extra\n\
diff --git a/img/logo.png b/img/logo.png\n\
index 3333..4444 100644\n\
Binary files a/img/logo.png and b/img/logo.png differ\n"
    }

    // ----- connect ---------------------------------------------------------

    #[test]
    fn connect_checks_the_app_password_and_names_the_account() {
        let (http, log) = mock(vec![get(&format!("{API_ROOT}/user"), me())]);
        let account =
            futures::executor::block_on(connect(&http, &draft("", Some("alice-dev"), "")))
                .expect("connects");

        assert_eq!(account.kind, ProviderKind::Bitbucket);
        assert_eq!(account.label, "Bitbucket (alice)");
        assert_eq!(account.base_url, API_ROOT);
        assert_eq!(account.web_url, WEB_ROOT);
        // The username the app password belongs to, not the display name.
        assert_eq!(account.username, "alice-dev");
        assert_eq!(account.display_name, "Alice Able");
        assert_eq!(
            account.avatar_url,
            "https://bitbucket.org/account/alice/avatar/"
        );

        let requests = log.lock();
        assert_eq!(requests[0].url, format!("{API_ROOT}/user"));
        assert_eq!(requests[0].header("Authorization"), Some(AUTH));
        assert_eq!(requests[0].header("User-Agent"), Some("Reviewdeck"));
        assert_eq!(requests[0].header("Accept"), Some("application/json"));
    }

    #[test]
    fn connect_accepts_bitbucket_org_in_any_case_and_as_a_url() {
        let (http, _) = mock(vec![get(
            &format!("{API_ROOT}/user"),
            json!({"display_name": "Alice"}),
        )]);
        let account = futures::executor::block_on(connect(
            &http,
            &draft(
                "https://BitBucket.org/",
                Some("alice-dev"),
                "Client account",
            ),
        ))
        .expect("connects");
        assert_eq!(account.label, "Client account");
        // The display name stands in when there is no nickname, as `??` does.
        assert_eq!(account.display_name, "Alice");
    }

    #[test]
    fn connect_needs_the_username_the_app_password_belongs_to() {
        let (http, log) = mock(Vec::new());
        for username in [None, Some("")] {
            let error = futures::executor::block_on(connect(&http, &draft("", username, "")))
                .expect_err("no username");
            assert_eq!(
                error.to_string(),
                "Bitbucket needs the username the app password belongs to."
            );
        }
        assert!(log.lock().is_empty());
    }

    #[test]
    fn connect_refuses_a_server_host() {
        let (http, log) = mock(Vec::new());
        let error = futures::executor::block_on(connect(
            &http,
            &draft("bitbucket.example.com", Some("alice-dev"), ""),
        ))
        .expect_err("Bitbucket Server is a different API");
        assert_eq!(
            error.to_string(),
            "Only Bitbucket Cloud (bitbucket.org) is supported right now."
        );
        assert!(log.lock().is_empty());
    }

    #[test]
    fn connect_reports_a_rejected_app_password() {
        let (http, _) = mock(vec![with_status(
            "GET",
            &format!("{API_ROOT}/user"),
            401,
            json!({}),
        )]);
        let error = futures::executor::block_on(connect(&http, &draft("", Some("alice-dev"), "")))
            .expect_err("rejected");
        assert_eq!(
            error.to_string(),
            "Not authorised on api.bitbucket.org - the token is invalid or expired."
        );
    }

    // ----- list_review_requests --------------------------------------------

    /// A listing with two workspaces (the first paged), a repo with one review
    /// request, a repo with nothing for the token, and a repo the token authored.
    fn listing_routes() -> Vec<(String, MockResponse)> {
        let workspaces_first =
            format!("{API_ROOT}/workspaces?pagelen=50&fields=values.slug,values.name,next");
        let workspaces_second = format!("{API_ROOT}/workspaces?page=2");
        let repos_acme = format!(
            "{API_ROOT}/repositories/acme?role=member&sort=-updated_on&pagelen=50&fields=values.full_name,values.slug,values.workspace.slug,values.updated_on,next"
        );
        let repos_labs = format!(
            "{API_ROOT}/repositories/labs?role=member&sort=-updated_on&pagelen=50&fields=values.full_name,values.slug,values.workspace.slug,values.updated_on,next"
        );
        vec![
            get(&format!("{API_ROOT}/user"), me()),
            get(
                &workspaces_first,
                json!({
                    "values": [{"slug": "acme", "name": "Acme"}],
                    "next": workspaces_second
                }),
            ),
            get(
                &workspaces_second,
                json!({
                    "values": [{"slug": "labs", "name": "Labs"}]
                }),
            ),
            get(
                &repos_acme,
                json!({
                    "values": [
                        {"full_name": "acme/web", "slug": "web"},
                        {"full_name": "acme/idle", "slug": "idle"},
                        {"full_name": "acme/mine", "slug": "mine"}
                    ]
                }),
            ),
            get(
                &repos_labs,
                json!({"values": [{"full_name": "labs/tools"}]}),
            ),
            get(
                &format!("{API_ROOT}/repositories/acme/web/pullrequests?state=OPEN&pagelen=50"),
                json!({"values": [checkout_pull(), {
                    "id": 43,
                    "title": "Not mine",
                    "author": {"uuid": "{bob-uuid}", "display_name": "Bob"},
                    "reviewers": [{"uuid": "{carol-uuid}"}],
                    "participants": []
                }]}),
            ),
            get(
                &format!("{API_ROOT}/repositories/acme/idle/pullrequests?state=OPEN&pagelen=50"),
                json!({"values": []}),
            ),
            get(
                &format!("{API_ROOT}/repositories/acme/mine/pullrequests?state=OPEN&pagelen=50"),
                json!({"values": [{
                    "id": 7,
                    "title": "My own",
                    "author": {"uuid": "{me-uuid}", "display_name": "Alice Able"},
                    "reviewers": [{"uuid": "{me-uuid}"}],
                    "participants": []
                }]}),
            ),
            get(
                &format!("{API_ROOT}/repositories/labs/tools/pullrequests?state=OPEN&pagelen=50"),
                json!({"values": [{
                    "id": 5,
                    "title": "Tools",
                    "author": {"display_name": "Dan"},
                    "participants": [{"user": {"uuid": "{me-uuid}"}, "role": "REVIEWER", "approved": true}]
                }]}),
            ),
            get(
                &format!(
                    "{API_ROOT}/repositories/acme/web/branch-restrictions?kind=require_approvals_to_merge&pagelen=50"
                ),
                json!({"values": [
                    {"kind": "require_approvals_to_merge", "pattern": "main", "value": 2, "branch_match_kind": "glob"},
                    {"kind": "require_approvals_to_merge", "pattern": "ma*", "value": 3, "branch_match_kind": "glob"},
                    {"kind": "require_approvals_to_merge", "pattern": "release/*", "value": 9, "branch_match_kind": "glob"},
                    {"kind": "require_approvals_to_merge", "branch_match_kind": "branching_model", "value": 4}
                ]}),
            ),
            get(
                &format!(
                    "{API_ROOT}/repositories/labs/tools/branch-restrictions?kind=require_approvals_to_merge&pagelen=50"
                ),
                json!({"values": []}),
            ),
            get(
                &format!("{API_ROOT}/repositories/acme/web/pullrequests/42/statuses?pagelen=50"),
                json!({"values": [
                    {"key": "build", "name": "Build", "state": "SUCCESSFUL", "url": "https://ci/1", "description": "ok"},
                    {"key": "lint", "name": "", "state": "INPROGRESS", "url": "", "description": ""}
                ]}),
            ),
            get(
                &format!("{API_ROOT}/repositories/labs/tools/pullrequests/5/statuses?pagelen=50"),
                json!({"values": [{"key": "unit", "name": "", "state": "FAILED"}]}),
            ),
        ]
    }

    #[test]
    fn list_keeps_only_the_pull_requests_that_name_the_token() {
        let (http, log) = mock(listing_routes());
        let items =
            futures::executor::block_on(list_review_requests(&http, &session())).expect("lists");

        assert_eq!(
            items.len(),
            2,
            "idle, the authored PR and PR 43 are left out"
        );
        let web = items
            .iter()
            .find(|item| item.repo == "acme/web")
            .expect("acme/web is listed");
        assert_eq!(web.id, "acc-bb:acme/web:42");
        assert_eq!(web.account_id, "acc-bb");
        assert_eq!(web.provider, ProviderKind::Bitbucket);
        assert_eq!(web.repo_key, "acme/web");
        assert_eq!(web.number, 42);
        assert_eq!(web.title, "Checkout");
        assert_eq!(web.url, "https://bitbucket.org/acme/web/pull-requests/42");
        assert_eq!(web.author.name, "Bob");
        assert_eq!(web.author.avatar_url, "https://bitbucket.org/avatars/bob");
        assert_eq!(web.created_at, "2026-08-01T09:00:00.000000+00:00");
        assert_eq!(web.updated_at, "2026-08-02T09:00:00.000000+00:00");
        assert!(!web.draft);
        assert_eq!(web.source_branch, "feature/checkout");
        assert_eq!(web.target_branch, "main");
        assert!(web.labels.is_empty());
        // The token's participant record says changes were requested.
        assert_eq!(web.my_review_state, MyReviewState::ChangesRequested);
        // Carol approved; the glob "main" asks for 2 and the longer "ma*" for 3, so
        // the largest matching value counts, and the release rule does not apply.
        assert_eq!(web.approvals.given, 1);
        assert_eq!(web.approvals.required, Some(3));
        assert_eq!(web.approvals.outcome, ApprovalOutcome::Pending);
        assert_eq!(web.checks.status, CheckStatus::Running);
        assert_eq!(web.checks.passed, 1);
        assert_eq!(web.checks.running, 1);
        assert_eq!(
            web.checks.runs[1].name, "lint",
            "an unnamed status is named by its key"
        );
        assert_eq!(web.checks.runs[0].url.as_deref(), Some("https://ci/1"));
        assert_eq!(web.checks.runs[1].url, None);
        assert_eq!(web.additions, None);

        let tools = items
            .iter()
            .find(|item| item.repo == "labs/tools")
            .expect("labs/tools is listed");
        assert_eq!(tools.author.name, "Dan");
        assert_eq!(tools.author.avatar_url, "");
        assert_eq!(
            tools.url,
            "https://bitbucket.org/labs/tools/pull-requests/5"
        );
        // Approved through the participant record, with nothing required.
        assert_eq!(tools.my_review_state, MyReviewState::Approved);
        assert_eq!(tools.approvals.given, 1);
        assert_eq!(tools.approvals.required, None);
        assert_eq!(tools.approvals.outcome, ApprovalOutcome::NoneRequired);
        assert_eq!(tools.checks.status, CheckStatus::Failed);

        let requests = log.lock();
        assert!(
            requests
                .iter()
                .all(|request| request.header("Authorization") == Some(AUTH))
        );
        assert!(
            !requests
                .iter()
                .any(|request| request.url.contains("acme/mine/branch-restrictions")),
            "restrictions are read only for repos with a request to ask about"
        );
    }

    #[test]
    fn list_walks_the_workspace_pages_and_stops_at_the_repo_cap() {
        // The first workspace alone holds more repos than the cap; the second is
        // never asked for.
        let many: Vec<Value> = (0..125)
            .map(|n| json!({"full_name": format!("acme/r{n}")}))
            .collect();
        let repos_acme = format!(
            "{API_ROOT}/repositories/acme?role=member&sort=-updated_on&pagelen=50&fields=values.full_name,values.slug,values.workspace.slug,values.updated_on,next"
        );
        let routes = vec![
            get(&format!("{API_ROOT}/user"), me()),
            get(
                &format!("{API_ROOT}/workspaces?pagelen=50&fields=values.slug,values.name,next"),
                json!({"values": [{"slug": "acme"}, {"slug": "labs"}]}),
            ),
            get(&repos_acme, json!({"values": many})),
        ];
        let (http, log) = mock(routes);
        let items =
            futures::executor::block_on(list_review_requests(&http, &session())).expect("lists");
        assert!(items.is_empty());
        let requests = log.lock();
        assert!(
            !requests
                .iter()
                .any(|request| request.url.contains("repositories/labs"))
        );
        let pulls = requests
            .iter()
            .filter(|request| request.url.contains("/pullrequests?state=OPEN"))
            .count();
        assert_eq!(pulls, MAX_REPOS, "each repo up to the cap is asked once");
    }

    #[test]
    fn a_repo_that_will_not_list_its_pull_requests_is_skipped() {
        let mut routes = listing_routes();
        routes.retain(|(key, _)| !key.contains("acme/web/pullrequests?state"));
        let (http, _) = mock(routes);
        let items =
            futures::executor::block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].repo, "labs/tools");
    }

    #[test]
    fn a_failing_workspace_listing_fails_the_sync() {
        let (http, _) = mock(vec![
            get(&format!("{API_ROOT}/user"), me()),
            with_status(
                "GET",
                &format!("{API_ROOT}/workspaces?pagelen=50&fields=values.slug,values.name,next"),
                403,
                json!({"error": {"message": "Forbidden"}}),
            ),
        ]);
        let error = futures::executor::block_on(list_review_requests(&http, &session()))
            .expect_err("the workspaces are the sync");
        assert_eq!(
            error.to_string(),
            "Forbidden on api.bitbucket.org - the token is missing a required scope."
        );
    }

    #[test]
    fn a_missing_optional_field_never_fails_the_sync() {
        let repos = format!(
            "{API_ROOT}/repositories/acme?role=member&sort=-updated_on&pagelen=50&fields=values.full_name,values.slug,values.workspace.slug,values.updated_on,next"
        );
        let (http, _) = mock(vec![
            get(&format!("{API_ROOT}/user"), json!({"uuid": "{me-uuid}"})),
            get(
                &format!("{API_ROOT}/workspaces?pagelen=50&fields=values.slug,values.name,next"),
                json!({"values": [{"slug": "acme"}, {"name": "no slug"}, "junk"]}),
            ),
            get(
                &repos,
                json!({"values": [{"full_name": "acme/web"}, {"slug": "nameless"}]}),
            ),
            get(
                &format!("{API_ROOT}/repositories/acme/web/pullrequests?state=OPEN&pagelen=50"),
                json!({"values": [{
                    "id": 9,
                    "reviewers": [{"uuid": "{me-uuid}"}]
                }, {
                    "title": "no id"
                }]}),
            ),
            get(
                &format!(
                    "{API_ROOT}/repositories/acme/web/branch-restrictions?kind=require_approvals_to_merge&pagelen=50"
                ),
                json!({"values": [{"kind": "require_approvals_to_merge", "pattern": "*"}]}),
            ),
            get(
                &format!("{API_ROOT}/repositories/acme/web/pullrequests/9/statuses?pagelen=50"),
                json!({"values": null}),
            ),
        ]);
        let items = futures::executor::block_on(list_review_requests(&http, &session()))
            .expect("lists what it can");
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.title, "");
        // No author at all: the TypeScript's 'unknown' stands in.
        assert_eq!(item.author.name, "unknown");
        assert_eq!(item.author.avatar_url, "");
        assert_eq!(item.source_branch, "");
        assert_eq!(item.target_branch, "");
        assert_eq!(item.url, "https://bitbucket.org/acme/web/pull-requests/9");
        // A restriction with no value asks for nothing, as `?? 0` does.
        assert_eq!(item.approvals.required, Some(0));
        assert_eq!(item.approvals.outcome, ApprovalOutcome::NoneRequired);
        assert_eq!(item.checks.status, CheckStatus::Unknown);
    }

    #[test]
    fn branch_restrictions_read_by_glob_and_largest_value() {
        let restriction = |pattern: &str, value: Option<f64>, kind: &str| BbBranchRestriction {
            kind: Some(kind.into()),
            pattern: Some(pattern.into()),
            branch_match_kind: Some("glob".into()),
            value,
        };
        let rules = vec![
            restriction("main", Some(1.0), "require_approvals_to_merge"),
            restriction("*", Some(2.0), "require_approvals_to_merge"),
            restriction("release/*", Some(5.0), "require_approvals_to_merge"),
            restriction("*", Some(9.0), "require_approvals_to_build"),
            restriction("m**", Some(7.0), "require_approvals_to_merge"),
            restriction("feat*", Some(-4.0), "require_approvals_to_merge"),
        ];
        // "*", "m**" and "main" cover "main"; the largest value counts.
        assert_eq!(required_for(&rules, "main"), Some(7));
        assert_eq!(required_for(&rules, "release/1.2"), Some(5));
        assert_eq!(required_for(&rules, "mxx"), Some(7));
        assert_eq!(required_for(&rules[..2], "release/1.2"), Some(2));
        assert_eq!(required_for(&rules[..1], "dev"), None);
        // A negative value clamps to nothing.
        assert_eq!(required_for(&rules[5..], "feature"), Some(0));
    }

    #[test]
    fn review_state_follows_the_participant_record() {
        let pr = |state: Value, approved: bool| -> BbPullRequest {
            serde_json::from_value(json!({
                "participants": [{"user": {"uuid": "u1"}, "role": "REVIEWER", "approved": approved, "state": state}]
            }))
            .expect("parses")
        };
        assert_eq!(
            review_state_for(&pr(json!("approved"), false), "u1"),
            MyReviewState::Approved
        );
        assert_eq!(
            review_state_for(&pr(Value::Null, true), "u1"),
            MyReviewState::Approved
        );
        assert_eq!(
            review_state_for(&pr(json!("changes_requested"), false), "u1"),
            MyReviewState::ChangesRequested
        );
        assert_eq!(
            review_state_for(&pr(Value::Null, false), "u1"),
            MyReviewState::Pending
        );
        assert_eq!(
            review_state_for(&pr(json!("approved"), true), "someone-else"),
            MyReviewState::Pending
        );
    }

    // ----- load_detail -----------------------------------------------------

    #[test]
    fn load_detail_reads_the_pull_request_diff_threads_and_refs() {
        let (http, log) = mock(vec![
            get(PR_URL, checkout_pull()),
            get_text(&format!("{PR_URL}/diff"), cart_diff()),
            get(
                &format!("{COMMENTS_URL}?pagelen=50"),
                json!({
                    "values": [
                        {"id": 1, "user": {"display_name": "Carol"}, "content": {"raw": "Rename this"}, "created_on": "2026-08-02T10:00:00+00:00", "inline": {"path": "src/cart.ts", "to": 2}},
                        {"id": 2, "parent": {"id": 1}, "user": {"display_name": "Bob"}, "content": {"raw": "Done"}, "created_on": "2026-08-02T10:05:00+00:00"}
                    ],
                    "next": format!("{COMMENTS_URL}?page=2")
                }),
            ),
            get(
                &format!("{COMMENTS_URL}?page=2"),
                json!({"values": [
                    {"id": 3, "user": {"display_name": "Dan"}, "content": {"raw": "Overall fine"}, "created_on": "2026-08-02T11:00:00+00:00"}
                ]}),
            ),
        ]);
        let detail = futures::executor::block_on(load_detail(&http, &session(), &tracked_item()))
            .expect("loads");

        assert_eq!(detail.description, "Adds **checkout**.");
        assert_eq!(detail.refs.head_sha.as_deref(), Some("aaa111"));
        assert_eq!(detail.refs.base_sha.as_deref(), Some("bbb222"));
        // Bitbucket's detail totals come from the diff, not the listing.
        assert_eq!(detail.item.additions, Some(2));
        assert_eq!(detail.item.deletions, Some(1));
        assert_eq!(detail.item.changed_files, Some(2));

        assert_eq!(detail.files.len(), 2);
        assert_eq!(detail.files[0].path, "src/cart.ts");
        assert_eq!(
            (detail.files[0].additions, detail.files[0].deletions),
            (2, 1)
        );
        assert!(detail.files[1].binary);
        assert_eq!(detail.files[1].patch, None);

        // The inline thread (with its reply) and the general comment on page two.
        assert_eq!(detail.threads.len(), 2);
        let inline = detail
            .threads
            .iter()
            .find(|thread| thread.path.as_deref() == Some("src/cart.ts"))
            .expect("inline thread");
        assert_eq!(inline.comments.len(), 2);
        assert_eq!(inline.comments[1].body, "Done");

        let requests = log.lock();
        assert!(
            requests
                .iter()
                .any(|request| request.url == format!("{COMMENTS_URL}?page=2"))
        );
    }

    #[test]
    fn a_diff_or_conversation_that_will_not_load_costs_only_itself() {
        let (http, _) = mock(vec![
            get(PR_URL, checkout_pull()),
            with_status("GET", &format!("{PR_URL}/diff"), 500, json!({})),
        ]);
        let detail = futures::executor::block_on(load_detail(&http, &session(), &tracked_item()))
            .expect("loads");
        assert!(detail.files.is_empty());
        assert!(detail.threads.is_empty());
        assert_eq!(detail.item.changed_files, Some(0));
    }

    #[test]
    fn a_failing_pull_request_read_fails_the_detail() {
        let (http, _) = mock(vec![with_status(
            "GET",
            PR_URL,
            404,
            json!({"error": {"message": "No such pull request"}}),
        )]);
        let error = futures::executor::block_on(load_detail(&http, &session(), &tracked_item()))
            .expect_err("the pull request is the detail");
        // Bitbucket nests its message under `error`, which the shared describe does not read.
        assert_eq!(
            error.to_string(),
            "Not found on api.bitbucket.org (/2.0/repositories/acme/web/pullrequests/42)."
        );
    }

    #[test]
    fn refresh_checks_maps_the_build_states() {
        let (http, _) = mock(vec![get(
            &format!("{PR_URL}/statuses?pagelen=50"),
            json!({"values": [
                {"key": "a", "name": "A", "state": "SUCCESSFUL"},
                {"key": "b", "name": "B", "state": "FAILED"},
                {"key": "c", "name": "C", "state": "STOPPED"},
                {"key": "d", "name": "D", "state": "INPROGRESS"},
                {"key": "e", "name": "E", "state": "NEVER_HEARD_OF_IT"}
            ]}),
        )]);
        let checks =
            futures::executor::block_on(refresh_checks(&http, &session(), &tracked_item()))
                .expect("refreshes");
        assert_eq!(checks.total, 5);
        assert_eq!(checks.passed, 1);
        assert_eq!(checks.failed, 2);
        assert_eq!(checks.running, 1);
        assert_eq!(checks.status, CheckStatus::Failed);
        assert_eq!(checks.runs[0].id, "status-a");
        assert_eq!(checks.runs[4].status, CheckStatus::Unknown);
    }

    #[test]
    fn load_threads_reads_only_the_conversation() {
        let (http, log) = mock(vec![get(
            &format!("{COMMENTS_URL}?pagelen=50"),
            json!({"values": [{"id": 9, "content": {"raw": "Hi"}, "created_on": "2026-08-02T10:00:00+00:00", "deleted": false}]}),
        )]);
        let threads = futures::executor::block_on(load_threads(&http, &session(), &tracked_item()))
            .expect("threads");
        assert_eq!(threads.len(), 1);
        assert_eq!(log.lock().len(), 1);
    }

    // ----- submit_review ---------------------------------------------------

    #[test]
    fn approve_clears_the_request_for_changes_then_posts_each_comment_and_the_text() {
        let (http, log) = mock(vec![
            ok_empty("POST", COMMENTS_URL),
            ok_empty("POST", &format!("{PR_URL}/approve")),
            with_status(
                "DELETE",
                &format!("{PR_URL}/request-changes"),
                404,
                json!({}),
            ),
        ]);
        let comments = [
            draft_comment("d1", "First", Some(3), None),
            draft_comment("d2", "Second", None, Some(8)),
        ];
        futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Approve,
            "Shipping it",
            &comments,
        ))
        .expect("submits even though nothing stood to clear");

        let requests = log.lock();
        let sequence: Vec<(String, String)> = requests
            .iter()
            .map(|request| (request.method.as_str().to_string(), request.url.clone()))
            .collect();
        assert_eq!(
            sequence,
            vec![
                ("POST".to_string(), COMMENTS_URL.to_string()),
                ("POST".to_string(), COMMENTS_URL.to_string()),
                ("DELETE".to_string(), format!("{PR_URL}/request-changes")),
                ("POST".to_string(), format!("{PR_URL}/approve")),
                ("POST".to_string(), COMMENTS_URL.to_string()),
            ]
        );
        assert_eq!(
            requests[0].json_body(),
            Some(json!({"content": {"raw": "First"}, "inline": {"path": "src/cart.ts", "to": 3}}))
        );
        assert_eq!(
            requests[1].json_body(),
            Some(
                json!({"content": {"raw": "Second"}, "inline": {"path": "src/cart.ts", "from": 8}})
            )
        );
        assert_eq!(
            requests[4].json_body(),
            Some(json!({"content": {"raw": "Shipping it"}}))
        );
        assert_eq!(
            requests[3].header("Content-Type"),
            None,
            "no body on the verdict"
        );
    }

    #[test]
    fn request_changes_clears_the_approval_then_asks_for_changes() {
        let (http, log) = mock(vec![
            with_status("DELETE", &format!("{PR_URL}/approve"), 404, json!({})),
            ok_empty("POST", &format!("{PR_URL}/request-changes")),
        ]);
        futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::RequestChanges,
            "",
            &[],
        ))
        .expect("submits");
        let sequence: Vec<(String, String)> = log
            .lock()
            .iter()
            .map(|request| (request.method.as_str().to_string(), request.url.clone()))
            .collect();
        assert_eq!(
            sequence,
            vec![
                ("DELETE".to_string(), format!("{PR_URL}/approve")),
                ("POST".to_string(), format!("{PR_URL}/request-changes")),
            ]
        );
    }

    #[test]
    fn a_plain_comment_posts_only_the_text() {
        let (http, log) = mock(vec![ok_empty("POST", COMMENTS_URL)]);
        futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Comment,
            "Just a note",
            &[],
        ))
        .expect("submits");
        let requests = log.lock();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].json_body(),
            Some(json!({"content": {"raw": "Just a note"}}))
        );
    }

    #[test]
    fn a_failed_first_comment_leaves_nothing_posted_and_reports_the_hosts_error() {
        let (http, log) = mock(vec![with_status(
            "POST",
            COMMENTS_URL,
            429,
            json!({"error": {"message": "Too many requests"}}),
        )]);
        let error = futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Approve,
            "",
            &[draft_comment("d1", "First", Some(3), None)],
        ))
        .expect_err("the first comment failed");
        assert!(matches!(error, Error::Api { status: 429, .. }), "{error:?}");
        assert_eq!(error.to_string(), "api.bitbucket.org returned 429.");
        // Nothing landed, so the verdict was never attempted.
        assert_eq!(log.lock().len(), 1);
    }

    #[test]
    fn a_comment_failing_midway_names_what_landed() {
        // The first comment lands; every later one is turned away.
        let log: Log = Arc::default();
        let recorder = Arc::clone(&log);
        let http = Http::mock(move |request| {
            recorder.lock().push(request.clone());
            if recorder.lock().len() == 1 {
                MockResponse::json(200, &json!({}))
            } else {
                MockResponse::json(429, &json!({}))
            }
        });
        let comments = [
            draft_comment("d1", "One", Some(1), None),
            draft_comment("d2", "Two", Some(2), None),
            draft_comment("d3", "Three", Some(3), None),
        ];
        let error = futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Approve,
            "",
            &comments,
        ))
        .expect_err("stops at the second");
        match &error {
            Error::PartialSubmit { posted, message } => {
                assert_eq!(posted, &vec!["d1".to_string()]);
                assert!(
                    message.starts_with(
                        "Posted 1 of 3 comments, then stopped: api.bitbucket.org returned 429."
                    ),
                    "{message}"
                );
                assert!(message.contains("Still drafted"), "{message}");
            }
            other => panic!("expected a partial submit, got {other:?}"),
        }
        assert_eq!(log.lock().len(), 2, "the verdict is not attempted");
    }

    #[test]
    fn a_failing_verdict_after_every_comment_landed_still_reports_them() {
        let (http, _) = mock(vec![
            ok_empty("POST", COMMENTS_URL),
            with_status(
                "DELETE",
                &format!("{PR_URL}/request-changes"),
                404,
                json!({}),
            ),
            with_status(
                "POST",
                &format!("{PR_URL}/approve"),
                422,
                json!({"message": "You cannot approve"}),
            ),
        ]);
        let error = futures::executor::block_on(submit_review(
            &http,
            &session(),
            &tracked_item(),
            ReviewVerdict::Approve,
            "",
            &[draft_comment("d1", "Only", Some(1), None)],
        ))
        .expect_err("the verdict failed");
        match error {
            Error::PartialSubmit { posted, message } => {
                assert_eq!(posted, vec!["d1".to_string()]);
                assert!(
                    message.starts_with("Posted 1 of 1 comment, then stopped: You cannot approve."),
                    "{message}"
                );
                assert!(
                    message.ends_with("Every comment landed; only the verdict did not."),
                    "{message}"
                );
            }
            other => panic!("expected a partial submit, got {other:?}"),
        }
    }

    // ----- comments and replies --------------------------------------------

    #[test]
    fn add_comment_posts_the_raw_text() {
        let (http, log) = mock(vec![ok_empty("POST", COMMENTS_URL)]);
        futures::executor::block_on(add_comment(&http, &session(), &tracked_item(), "LGTM"))
            .expect("posts");
        assert_eq!(
            log.lock()[0].json_body(),
            Some(json!({"content": {"raw": "LGTM"}}))
        );
    }

    #[test]
    fn add_line_comment_addresses_the_new_file_or_the_old_one() {
        let (http, log) = mock(vec![ok_empty("POST", COMMENTS_URL)]);
        let refs = DiffRefs::default();
        let new_side = LineCommentDraft {
            item_id: "acc-bb:acme/web:42".into(),
            body: "New".into(),
            path: "src/cart.ts".into(),
            new_line: Some(12),
            old_line: None,
            range: None,
        };
        futures::executor::block_on(add_line_comment(
            &http,
            &session(),
            &tracked_item(),
            &new_side,
            &refs,
        ))
        .expect("posts");
        let old_side = LineCommentDraft {
            new_line: None,
            old_line: Some(4),
            body: "Old".into(),
            ..new_side.clone()
        };
        futures::executor::block_on(add_line_comment(
            &http,
            &session(),
            &tracked_item(),
            &old_side,
            &refs,
        ))
        .expect("posts");

        let requests = log.lock();
        assert_eq!(
            requests[0].json_body(),
            Some(json!({"content": {"raw": "New"}, "inline": {"path": "src/cart.ts", "to": 12}}))
        );
        assert_eq!(
            requests[1].json_body(),
            Some(json!({"content": {"raw": "Old"}, "inline": {"path": "src/cart.ts", "from": 4}}))
        );
    }

    #[test]
    fn a_range_comment_names_where_it_starts_on_its_side() {
        let (http, log) = mock(vec![ok_empty("POST", COMMENTS_URL)]);
        let draft = LineCommentDraft {
            item_id: "acc-bb:acme/web:42".into(),
            body: "Range".into(),
            path: "src/cart.ts".into(),
            new_line: Some(12),
            old_line: None,
            range: Some(LineRange {
                start_line: 10,
                start: RangeEdge {
                    kind: EdgeKind::Add,
                    old_pos: 0,
                    new_pos: 10,
                },
                end: RangeEdge {
                    kind: EdgeKind::Add,
                    old_pos: 0,
                    new_pos: 12,
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
        assert_eq!(
            log.lock()[0].json_body(),
            Some(
                json!({"content": {"raw": "Range"}, "inline": {"path": "src/cart.ts", "to": 12, "start_to": 10}})
            )
        );
    }

    #[test]
    fn reply_names_the_comment_that_opened_the_thread() {
        let (http, log) = mock(vec![ok_empty("POST", COMMENTS_URL)]);
        futures::executor::block_on(reply_to_thread(
            &http,
            &session(),
            &tracked_item(),
            "1234",
            "Agreed",
        ))
        .expect("replies");
        assert_eq!(
            log.lock()[0].json_body(),
            Some(json!({"content": {"raw": "Agreed"}, "parent": {"id": 1234}}))
        );
    }

    #[test]
    fn a_reply_to_a_non_numeric_thread_sends_a_null_parent_as_javascript_would() {
        let (http, log) = mock(vec![ok_empty("POST", COMMENTS_URL)]);
        futures::executor::block_on(reply_to_thread(
            &http,
            &session(),
            &tracked_item(),
            "abc",
            "Hmm",
        ))
        .expect("replies");
        assert_eq!(
            log.lock()[0].json_body(),
            Some(json!({"content": {"raw": "Hmm"}, "parent": {"id": null}}))
        );
    }

    #[test]
    fn a_reply_to_an_empty_thread_id_names_parent_zero_as_number_does() {
        let (http, log) = mock(vec![ok_empty("POST", COMMENTS_URL)]);
        futures::executor::block_on(reply_to_thread(
            &http,
            &session(),
            &tracked_item(),
            " ",
            "Hm",
        ))
        .expect("replies");
        assert_eq!(
            log.lock()[0].json_body(),
            Some(json!({"content": {"raw": "Hm"}, "parent": {"id": 0}}))
        );
    }

    #[test]
    fn a_restriction_value_sent_as_a_float_still_counts() {
        let restrictions: Vec<BbBranchRestriction> = lenient(vec![
            json!({"kind": "require_approvals_to_merge", "pattern": "main", "value": 2.0}),
            json!({"kind": "require_approvals_to_merge", "pattern": "main", "value": -1}),
            json!({"kind": "require_approvals_to_merge", "pattern": "main", "value": null}),
        ]);
        assert_eq!(required_for(&restrictions, "main"), Some(2));
    }

    #[test]
    fn resolving_posts_to_the_opening_comment_and_reopening_deletes_it() {
        let resolve = format!("{COMMENTS_URL}/1234/resolve");
        let (http, log) = mock(vec![
            ok_empty("POST", &resolve),
            ok_empty("DELETE", &resolve),
        ]);
        futures::executor::block_on(set_thread_resolved(
            &http,
            &session(),
            &tracked_item(),
            "1234",
            true,
        ))
        .expect("resolves");
        futures::executor::block_on(set_thread_resolved(
            &http,
            &session(),
            &tracked_item(),
            "1234",
            false,
        ))
        .expect("reopens");
        let requests = log.lock();
        assert_eq!(
            (requests[0].method, requests[0].url.as_str()),
            (Method::Post, resolve.as_str())
        );
        assert_eq!(
            (requests[1].method, requests[1].url.as_str()),
            (Method::Delete, resolve.as_str())
        );
        assert_eq!(requests[0].body, None);
    }

    #[test]
    fn a_thread_id_is_percent_encoded_in_the_resolve_url() {
        let (http, log) = mock(vec![ok_empty(
            "POST",
            &format!("{COMMENTS_URL}/a%20b%2Fc/resolve"),
        )]);
        futures::executor::block_on(set_thread_resolved(
            &http,
            &session(),
            &tracked_item(),
            "a b/c",
            true,
        ))
        .expect("resolves");
        assert_eq!(
            log.lock()[0].url,
            format!("{COMMENTS_URL}/a%20b%2Fc/resolve")
        );
    }

    #[test]
    fn encode_uri_component_matches_the_javascript_function() {
        assert_eq!(encode_uri_component("1234"), "1234");
        assert_eq!(encode_uri_component("a b/c?"), "a%20b%2Fc%3F");
        assert_eq!(encode_uri_component("ü"), "%C3%BC");
    }
}
