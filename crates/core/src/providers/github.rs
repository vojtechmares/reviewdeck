//! Port of src/main/providers/github.ts.
//!
//! GitHub.com and GitHub Enterprise Server. GHES puts the API under `/api/v3` on the
//! same host as the web UI, so the account keeps both roots and every call goes
//! through [`api`].
//!
//! Host JSON is read into private structs that are as lenient as the TypeScript:
//! every optional field is an `Option`, and list entries that do not parse are
//! dropped one by one rather than failing the whole list, so one odd entry never
//! sinks a sync.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use url::Url;

use crate::error::{Error, Result, msg};
use crate::http::{Http, Method, RequestOptions, to_origin};
use crate::model::{
    AccountDraft, ApprovalOutcome, ApprovalSummary, CheckRun, CheckStatus, CheckSummary,
    CommentThread, DiffFile, DiffRefs, DraftComment, FileStatus, LineCommentDraft, MyReviewState,
    NewAccount, ProviderKind, PullDetail, ReviewItem, ReviewVerdict, User, count_approvers,
    make_item_id, summarise_checks,
};
use crate::providers::Session;
use crate::providers::limit_concurrency;
use crate::providers::submit::{github_comment_position, github_review_comments};
use crate::providers::threads::{
    GithubComment, GithubGqlComment, GithubReviewThread, github_flat_threads, github_threads,
};

// --- Requests ------------------------------------------------------------------

fn headers(token: &str) -> Vec<(String, String)> {
    vec![
        ("Authorization".into(), format!("Bearer {token}")),
        ("Accept".into(), "application/vnd.github+json".into()),
        ("X-GitHub-Api-Version".into(), "2022-11-28".into()),
        ("User-Agent".into(), "Reviewdeck".into()),
    ]
}

fn get(token: &str) -> RequestOptions {
    RequestOptions::new(Method::Get).headers(headers(token))
}

fn post(token: &str) -> RequestOptions {
    RequestOptions::new(Method::Post).headers(headers(token))
}

fn api(session: &Session, path: &str) -> String {
    format!("{}{path}", session.account.base_url)
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

/// A 404 is an answer ("this commit has no checks"), not a failure.
fn ignore_not_found(error: Error) -> Result<()> {
    match error {
        Error::Api { status: 404, .. } => Ok(()),
        other => Err(other),
    }
}

/// `acme/widgets` -> `("acme", "widgets")`
fn split_repo(repo_key: &str) -> (&str, &str) {
    repo_key.split_once('/').unwrap_or((repo_key, repo_key))
}

/// `https://api.github.com/repos/acme/widgets` -> `acme/widgets`
fn repo_from_url(repository_url: &str) -> &str {
    let parts: Vec<&str> = repository_url.split("/repos/").collect();
    parts.get(1).copied().unwrap_or(repository_url)
}

/// `GitHub.com` and Enterprise Server: the REST root and the web root for a host.
fn roots_for(host: &str) -> Result<(String, String)> {
    let origin = to_origin(host)?;
    let hostname = Url::parse(&origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    if matches!(
        hostname.as_str(),
        "github.com" | "www.github.com" | "api.github.com"
    ) {
        return Ok(("https://api.github.com".into(), "https://github.com".into()));
    }
    Ok((format!("{origin}/api/v3"), origin))
}

/// Where the instance serves GraphQL.
///
/// GitHub.com puts it on the API host; Enterprise Server puts it beside the REST
/// root on the instance's own host, one path segment up from `/api/v3`.
pub fn graphql_root(base_url: &str) -> String {
    let root = base_url.trim_end_matches('/');
    match root.strip_suffix("/v3") {
        Some(prefix) if root.ends_with("/api/v3") => format!("{prefix}/graphql"),
        _ => format!("{root}/graphql"),
    }
}

/// A thin layer over the request wrapper, for GitHub only and only where REST cannot
/// answer: review threads are absent from the REST API entirely, and their resolution
/// exists nowhere but as a mutation here.
///
/// GraphQL answers 200 with an `errors` array, so a failure has to be read out of the
/// body rather than the status. Returns the `data` object.
async fn graphql(http: &Http, s: &Session, query: &str, variables: Value) -> Result<Value> {
    let root = graphql_root(&s.account.base_url);
    let envelope: Value = http
        .json(
            &root,
            post(&s.token).json(json!({ "query": query, "variables": variables })),
        )
        .await?;
    let errors = envelope
        .get("errors")
        .and_then(Value::as_array)
        .filter(|errors| !errors.is_empty());
    if let Some(errors) = errors {
        let message = errors
            .first()
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("GitHub rejected the query.");
        return Err(graphql_error(&root, message));
    }
    match envelope.get("data") {
        Some(data) if !data.is_null() => Ok(data.clone()),
        _ => Err(graphql_error(&root, "GitHub returned no data.")),
    }
}

fn graphql_error(root: &str, message: &str) -> Error {
    Error::Api {
        status: 200,
        url: root.to_string(),
        message: message.to_string(),
        body: None,
    }
}

fn parse_data<T: DeserializeOwned>(data: Value) -> Result<T> {
    serde_json::from_value(data)
        .map_err(|error| msg(format!("Unexpected response from GitHub: {error}")))
}

// --- Host shapes ------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhUser {
    login: Option<String>,
    name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhSearchResponse {
    items: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhSearchItem {
    number: Option<u64>,
    repository_url: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhRef {
    #[serde(rename = "ref")]
    git_ref: Option<String>,
    sha: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhLabel {
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhPull {
    title: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    draft: Option<bool>,
    created_at: Option<String>,
    updated_at: Option<String>,
    user: Option<GhUser>,
    head: Option<GhRef>,
    base: Option<GhRef>,
    additions: Option<u32>,
    deletions: Option<u32>,
    changed_files: Option<u32>,
    labels: Option<Vec<GhLabel>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhFile {
    filename: Option<String>,
    previous_filename: Option<String>,
    status: Option<String>,
    additions: Option<u32>,
    deletions: Option<u32>,
    patch: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhReview {
    user: Option<GhUser>,
    state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhCheckRunsResponse {
    check_runs: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhCheckOutput {
    title: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhCheckRun {
    id: u64,
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    html_url: Option<String>,
    output: Option<GhCheckOutput>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhStatusesResponse {
    statuses: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct GhStatus {
    id: u64,
    context: Option<String>,
    state: Option<String>,
    target_url: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct DecisionData {
    repository: Option<DecisionRepository>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DecisionRepository {
    pull_request: Option<DecisionPull>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DecisionPull {
    review_decision: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ThreadsData {
    repository: Option<ThreadsRepository>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ThreadsRepository {
    pull_request: Option<ThreadsPull>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ThreadsPull {
    review_threads: Option<NodeList>,
    comments: Option<NodeList>,
}

/// A GraphQL connection. Its nodes stay raw so one unreadable node is dropped alone.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct NodeList {
    nodes: Option<Vec<Value>>,
}

impl GhPull {
    fn head_sha(&self) -> String {
        self.head
            .as_ref()
            .and_then(|head| head.sha.clone())
            .unwrap_or_default()
    }
}

// --- Mapping ---------------------------------------------------------------------

fn check_status(run: &GhCheckRun) -> CheckStatus {
    if run.status.as_deref() != Some("completed") {
        return CheckStatus::Running;
    }
    match run.conclusion.as_deref() {
        Some("success") => CheckStatus::Passed,
        Some("failure" | "timed_out" | "startup_failure") => CheckStatus::Failed,
        Some("cancelled" | "action_required") => CheckStatus::Failed,
        Some("neutral" | "skipped" | "success_but_ignored") => CheckStatus::Passed,
        _ => CheckStatus::Unknown,
    }
}

fn status_state(state: Option<&str>) -> CheckStatus {
    match state {
        Some("success") => CheckStatus::Passed,
        Some("failure" | "error") => CheckStatus::Failed,
        Some("pending") => CheckStatus::Running,
        _ => CheckStatus::Unknown,
    }
}

fn file_status(status: Option<&str>) -> FileStatus {
    match status {
        Some("added") => FileStatus::Added,
        Some("removed") => FileStatus::Removed,
        Some("renamed") => FileStatus::Renamed,
        _ => FileStatus::Modified,
    }
}

fn diff_file(file: GhFile) -> DiffFile {
    let filename = file.filename.unwrap_or_default();
    let binary = file.patch.as_deref().unwrap_or("").is_empty()
        && file.additions == Some(0)
        && file.deletions == Some(0);
    DiffFile {
        old_path: file.previous_filename.unwrap_or_else(|| filename.clone()),
        path: filename,
        status: file_status(file.status.as_deref()),
        additions: file.additions.unwrap_or(0),
        deletions: file.deletions.unwrap_or(0),
        patch: file.patch,
        binary,
    }
}

fn empty_checks() -> CheckSummary {
    summarise_checks(Vec::new())
}

/// The checks on one commit: check runs (Actions and most apps) and legacy commit
/// statuses are separate APIs, and each may be absent (404) without failing the rest.
async fn load_checks(http: &Http, s: &Session, repo: &str, sha: &str) -> Result<CheckSummary> {
    let mut runs = Vec::new();

    let check_runs = api(
        s,
        &format!("/repos/{repo}/commits/{sha}/check-runs?per_page=100"),
    );
    match http
        .json::<GhCheckRunsResponse>(&check_runs, get(&s.token))
        .await
    {
        Ok(result) => {
            for run in lenient::<GhCheckRun>(result.check_runs.unwrap_or_default()) {
                runs.push(CheckRun {
                    id: format!("check-{}", run.id),
                    name: run.name.clone().unwrap_or_default(),
                    status: check_status(&run),
                    url: run.html_url.clone(),
                    description: run.output.and_then(|output| output.title),
                });
            }
        }
        Err(error) => ignore_not_found(error)?,
    }

    let statuses = api(s, &format!("/repos/{repo}/commits/{sha}/status"));
    match http
        .json::<GhStatusesResponse>(&statuses, get(&s.token))
        .await
    {
        Ok(result) => {
            for status in lenient::<GhStatus>(result.statuses.unwrap_or_default()) {
                runs.push(CheckRun {
                    id: format!("status-{}", status.id),
                    name: status.context.clone().unwrap_or_default(),
                    status: status_state(status.state.as_deref()),
                    url: status.target_url,
                    description: status.description,
                });
            }
        }
        Err(error) => ignore_not_found(error)?,
    }

    Ok(summarise_checks(runs))
}

/// The user's own verdict and the pull request's standing.
///
/// The count comes from the review list. The verdict on it does not: whether the
/// approvals given are the ones the branch wants - how many, and from a code owner or
/// not - is settled by branch protection, which the REST API only shows to a
/// repository admin. GraphQL's `reviewDecision` is that verdict without the admin, and
/// null there is a branch with no protection asking for reviews at all.
async fn load_approvals(
    http: &Http,
    s: &Session,
    repo: &str,
    number: u64,
) -> (MyReviewState, ApprovalSummary) {
    let mut review_state = MyReviewState::Pending;
    let mut given = 0;

    let reviews_url = api(
        s,
        &format!("/repos/{repo}/pulls/{number}/reviews?per_page=100"),
    );
    // A missing review list should not sink the whole sync.
    if let Ok(entries) = http.json::<Vec<Value>>(&reviews_url, get(&s.token)).await {
        let reviews: Vec<GhReview> = lenient(entries);
        given = count_approvers(reviews.iter().map(|review| {
            (
                review.user.as_ref().and_then(|user| user.login.as_deref()),
                review.state.as_deref().unwrap_or(""),
            )
        }));
        let mine: Vec<&GhReview> = reviews
            .iter()
            .filter(|review| {
                review.user.as_ref().and_then(|user| user.login.as_deref())
                    == Some(s.account.username.as_str())
            })
            .collect();
        // The last non-comment verdict is the one that counts.
        for review in mine.iter().rev() {
            match review.state.as_deref() {
                Some("APPROVED") => {
                    review_state = MyReviewState::Approved;
                    break;
                }
                Some("CHANGES_REQUESTED") => {
                    review_state = MyReviewState::ChangesRequested;
                    break;
                }
                _ => {}
            }
        }
        if review_state == MyReviewState::Pending && !mine.is_empty() {
            review_state = MyReviewState::Commented;
        }
    }

    // A token that cannot ask is a branch that says nothing.
    let (owner, name) = split_repo(repo);
    let decision = graphql(
        http,
        s,
        DECISION_QUERY,
        json!({ "owner": owner, "name": name, "number": number }),
    )
    .await
    .and_then(parse_data::<DecisionData>)
    .ok()
    .and_then(|data| data.repository)
    .and_then(|repository| repository.pull_request)
    .and_then(|pull| pull.review_decision);
    let outcome = match decision.as_deref() {
        Some("APPROVED") => ApprovalOutcome::Satisfied,
        Some("REVIEW_REQUIRED" | "CHANGES_REQUESTED") => ApprovalOutcome::Pending,
        _ => ApprovalOutcome::NoneRequired,
    };

    (
        review_state,
        ApprovalSummary {
            given,
            required: None,
            outcome,
        },
    )
}

/// One pull request as it appears in the deck: the pull, its checks and its approvals.
async fn review_item(
    http: &Http,
    s: &Session,
    number: u64,
    repository_url: &str,
) -> Result<ReviewItem> {
    let repo = repo_from_url(repository_url).to_string();
    let pull: GhPull = http
        .json(
            &api(s, &format!("/repos/{repo}/pulls/{number}")),
            get(&s.token),
        )
        .await?;
    let head_sha = pull.head_sha();
    let (checks, (review_state, approvals)) = futures::join!(
        load_checks(http, s, &repo, &head_sha),
        load_approvals(http, s, &repo, number),
    );
    // A checks failure is an empty checks list, not a failed item.
    let checks = checks.unwrap_or_else(|_| empty_checks());

    let author = pull.user.unwrap_or_default();
    Ok(ReviewItem {
        id: make_item_id(&s.account.id, &repo, number),
        account_id: s.account.id.clone(),
        provider: ProviderKind::Github,
        repo_key: repo.clone(),
        repo,
        number,
        title: pull.title.unwrap_or_default(),
        url: pull.html_url.unwrap_or_default(),
        author: User {
            name: author.login.unwrap_or_default(),
            avatar_url: author.avatar_url.unwrap_or_default(),
        },
        created_at: pull.created_at.unwrap_or_default(),
        updated_at: pull.updated_at.unwrap_or_default(),
        draft: pull.draft.unwrap_or(false),
        source_branch: pull
            .head
            .as_ref()
            .and_then(|head| head.git_ref.clone())
            .unwrap_or_default(),
        target_branch: pull
            .base
            .as_ref()
            .and_then(|base| base.git_ref.clone())
            .unwrap_or_default(),
        labels: pull
            .labels
            .unwrap_or_default()
            .into_iter()
            .filter_map(|label| label.name)
            .collect(),
        my_review_state: review_state,
        approvals,
        checks,
        additions: pull.additions,
        deletions: pull.deletions,
        changed_files: pull.changed_files,
    })
}

// --- Threads ---------------------------------------------------------------------

const THREADS_QUERY: &str = r#"
query Threads($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewThreads(first: 100) {
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          startLine
          diffSide
          startDiffSide
          viewerCanReply
          viewerCanResolve
          viewerCanUnresolve
          comments(first: 50) {
            nodes { id body createdAt author { login avatarUrl } }
          }
        }
      }
      comments(first: 100) {
        nodes { id body createdAt author { login avatarUrl } }
      }
    }
  }
}"#;

const DECISION_QUERY: &str = r#"
query Decision($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      reviewDecision
    }
  }
}"#;

/// Threads for one pull request, over GraphQL because REST has no notion of them.
///
/// An instance that will not answer - an older Enterprise Server, a token GraphQL
/// turns away - falls back to the flat REST shape, which is what the app showed
/// before. Losing the thread structure is much better than losing the comments.
async fn fetch_threads(http: &Http, s: &Session, item: &ReviewItem) -> Vec<CommentThread> {
    match graphql_threads(http, s, item).await {
        Ok(threads) => threads,
        Err(_) => {
            let issue_url = api(
                s,
                &format!(
                    "/repos/{}/issues/{}/comments?per_page=100",
                    item.repo_key, item.number
                ),
            );
            let review_url = api(
                s,
                &format!(
                    "/repos/{}/pulls/{}/comments?per_page=100",
                    item.repo_key, item.number
                ),
            );
            let (issue_comments, review_comments) = futures::join!(
                http.json::<Vec<Value>>(&issue_url, get(&s.token)),
                http.json::<Vec<Value>>(&review_url, get(&s.token)),
            );
            let issue_comments: Vec<GithubComment> = lenient(issue_comments.unwrap_or_default());
            let review_comments: Vec<GithubComment> = lenient(review_comments.unwrap_or_default());
            github_flat_threads(&issue_comments, &review_comments)
        }
    }
}

async fn graphql_threads(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
) -> Result<Vec<CommentThread>> {
    let (owner, name) = split_repo(&item.repo_key);
    let data = graphql(
        http,
        s,
        THREADS_QUERY,
        json!({ "owner": owner, "name": name, "number": item.number }),
    )
    .await?;
    let data: ThreadsData = parse_data(data)?;
    let pull = data
        .repository
        .and_then(|repository| repository.pull_request);
    let (threads, comments) = match pull {
        Some(pull) => (
            pull.review_threads
                .and_then(|list| list.nodes)
                .unwrap_or_default(),
            pull.comments
                .and_then(|list| list.nodes)
                .unwrap_or_default(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let threads: Vec<GithubReviewThread> = lenient(threads);
    let comments: Vec<GithubGqlComment> = lenient(comments);
    Ok(github_threads(&threads, &comments))
}

// --- The provider ----------------------------------------------------------------

pub async fn connect(http: &Http, draft: &AccountDraft) -> Result<NewAccount> {
    let host = if draft.host.is_empty() {
        "github.com"
    } else {
        draft.host.as_str()
    };
    let (base_url, web_url) = roots_for(host)?;
    let user: GhUser = http
        .json(&format!("{base_url}/user"), get(&draft.token))
        .await?;
    let login = user.login.unwrap_or_default();
    let display_name = user
        .name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| login.clone());
    Ok(NewAccount {
        kind: ProviderKind::Github,
        label: if draft.label.is_empty() {
            format!("GitHub ({login})")
        } else {
            draft.label.clone()
        },
        base_url,
        web_url,
        username: login,
        display_name,
        avatar_url: user.avatar_url.unwrap_or_default(),
        agent_command: None,
    })
}

pub async fn list_review_requests(http: &Http, s: &Session) -> Result<Vec<ReviewItem>> {
    // The search API is the only cheap way to ask "across every org I belong to".
    let query = encode_uri_component("is:open is:pr archived:false review-requested:@me");
    let search: GhSearchResponse = http
        .json(
            &api(
                s,
                &format!("/search/issues?q={query}&per_page=50&sort=updated&order=desc"),
            ),
            get(&s.token),
        )
        .await?;

    // An entry without its number or repository cannot be addressed, so it is skipped.
    let found: Vec<GhSearchItem> = lenient(search.items.unwrap_or_default());
    let found: Vec<(u64, String)> = found
        .into_iter()
        .filter_map(|found| Some((found.number?, found.repository_url?)))
        .collect();

    limit_concurrency(found, 6, move |(number, repository_url)| async move {
        review_item(http, s, number, &repository_url).await
    })
    .await
    .into_iter()
    .collect()
}

pub async fn load_detail(http: &Http, s: &Session, item: &ReviewItem) -> Result<PullDetail> {
    let pull_path = format!("/repos/{}/pulls/{}", item.repo_key, item.number);
    let pull_url = api(s, &pull_path);
    let files_url = api(s, &format!("{pull_path}/files?per_page=100"));
    let (pull, raw_files, threads) = futures::join!(
        http.json::<GhPull>(&pull_url, get(&s.token)),
        http.paginate::<Value>(&files_url, get(&s.token), 4),
        fetch_threads(http, s, item),
    );
    let pull = pull?;
    let files: Vec<GhFile> = lenient(raw_files?);

    Ok(PullDetail {
        item: ReviewItem {
            additions: pull.additions,
            deletions: pull.deletions,
            changed_files: pull.changed_files,
            ..item.clone()
        },
        description: pull.body.clone().unwrap_or_default(),
        files: files.into_iter().map(diff_file).collect(),
        threads,
        refs: DiffRefs {
            base_sha: pull.base.as_ref().and_then(|base| base.sha.clone()),
            start_sha: None,
            head_sha: pull.head.as_ref().and_then(|head| head.sha.clone()),
        },
    })
}

/// The thread's global node id is the whole address, so the item is not needed.
pub async fn reply_to_thread(
    http: &Http,
    s: &Session,
    _item: &ReviewItem,
    thread_id: &str,
    body: &str,
) -> Result<()> {
    graphql(
        http,
        s,
        r#"mutation Reply($threadId: ID!, $body: String!) {
        addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $threadId, body: $body }) {
          clientMutationId
        }
      }"#,
        json!({ "threadId": thread_id, "body": body }),
    )
    .await
    .map(|_| ())
}

pub async fn set_thread_resolved(
    http: &Http,
    s: &Session,
    _item: &ReviewItem,
    thread_id: &str,
    resolved: bool,
) -> Result<()> {
    let mutation = if resolved {
        r#"mutation Resolve($threadId: ID!) {
          resolveReviewThread(input: { threadId: $threadId }) { clientMutationId }
        }"#
    } else {
        r#"mutation Unresolve($threadId: ID!) {
          unresolveReviewThread(input: { threadId: $threadId }) { clientMutationId }
        }"#
    };
    graphql(http, s, mutation, json!({ "threadId": thread_id }))
        .await
        .map(|_| ())
}

pub async fn refresh_checks(http: &Http, s: &Session, item: &ReviewItem) -> Result<CheckSummary> {
    let pull: GhPull = http
        .json(
            &api(
                s,
                &format!("/repos/{}/pulls/{}", item.repo_key, item.number),
            ),
            get(&s.token),
        )
        .await?;
    load_checks(http, s, &item.repo_key, &pull.head_sha()).await
}

pub async fn load_threads(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
) -> Result<Vec<CommentThread>> {
    Ok(fetch_threads(http, s, item).await)
}

pub async fn submit_review(
    http: &Http,
    s: &Session,
    item: &ReviewItem,
    verdict: ReviewVerdict,
    body: &str,
    comments: &[DraftComment],
) -> Result<()> {
    let event = match verdict {
        ReviewVerdict::Approve => "APPROVE",
        ReviewVerdict::RequestChanges => "REQUEST_CHANGES",
        ReviewVerdict::Comment => "COMMENT",
    };
    // `undefined` fields are left out of the JSON, as JSON.stringify does.
    let mut payload = Map::new();
    payload.insert("event".into(), json!(event));
    if !body.is_empty() {
        payload.insert("body".into(), json!(body));
    }
    // Against the commit the drafts were written on, not whatever the branch has
    // become, so each remark lands on the code its author actually read.
    if let Some(head_sha) = comments
        .first()
        .and_then(|first| first.refs.head_sha.as_ref())
    {
        payload.insert("commit_id".into(), json!(head_sha));
    }
    if !comments.is_empty() {
        payload.insert(
            "comments".into(),
            Value::Array(github_review_comments(comments)),
        );
    }
    http.request(
        &api(
            s,
            &format!("/repos/{}/pulls/{}/reviews", item.repo_key, item.number),
        ),
        post(&s.token).json(Value::Object(payload)),
    )
    .await
    .map(|_| ())
}

pub async fn add_comment(http: &Http, s: &Session, item: &ReviewItem, body: &str) -> Result<()> {
    http.request(
        &api(
            s,
            &format!("/repos/{}/issues/{}/comments", item.repo_key, item.number),
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
    let Some(head_sha) = refs.head_sha.as_deref().filter(|sha| !sha.is_empty()) else {
        return Err(msg("Missing the head commit for this pull request."));
    };
    let mut payload = Map::new();
    payload.insert("commit_id".into(), json!(head_sha));
    if let Value::Object(position) = github_comment_position(draft.into()) {
        payload.extend(position);
    }
    http.request(
        &api(
            s,
            &format!("/repos/{}/pulls/{}/comments", item.repo_key, item.number),
        ),
        post(&s.token).json(Value::Object(payload)),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{MockRequest, MockResponse};
    use crate::model::{Account, EdgeKind, LineRange, RangeEdge, Side};
    use futures::executor::block_on;
    use parking_lot::Mutex;
    use std::sync::Arc;

    const API: &str = "https://api.github.com";
    const GRAPHQL: &str = "https://api.github.com/graphql";

    /// One request the mock saw: `GET https://...`, and its raw body.
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

    /// A mock host: each `"VERB url"` answers with its response, and anything else
    /// answers 599 so a test that strays fails loudly.
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

    fn ok(value: Value) -> MockResponse {
        MockResponse::json(200, &value)
    }

    fn session() -> Session {
        Session {
            account: Account {
                id: "acct-1".into(),
                kind: ProviderKind::Github,
                label: "Work GitHub".into(),
                base_url: API.into(),
                web_url: "https://github.com".into(),
                username: "octocat".into(),
                display_name: "Octo Cat".into(),
                avatar_url: "https://avatars.example/octocat".into(),
                added_at: "2026-08-01T10:00:00.000Z".into(),
                agent_command: None,
            },
            token: "ghp_test".into(),
        }
    }

    fn item() -> ReviewItem {
        ReviewItem {
            id: "acct-1:acme/widgets:7".into(),
            account_id: "acct-1".into(),
            provider: ProviderKind::Github,
            repo_key: "acme/widgets".into(),
            repo: "acme/widgets".into(),
            number: 7,
            title: "Add retry to the sync loop".into(),
            url: "https://github.com/acme/widgets/pull/7".into(),
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
            approvals: crate::model::no_approvals(),
            checks: empty_checks(),
            additions: None,
            deletions: None,
            changed_files: None,
        }
    }

    fn pull_json() -> Value {
        json!({
            "number": 7,
            "title": "Add retry to the sync loop",
            "body": "Retries three times before giving up.",
            "html_url": "https://github.com/acme/widgets/pull/7",
            "draft": false,
            "created_at": "2026-08-01T09:00:00Z",
            "updated_at": "2026-08-03T12:00:00Z",
            "user": {"login": "bob", "avatar_url": "https://avatars.example/bob", "html_url": "https://github.com/bob"},
            "head": {"ref": "retry", "sha": "aaa111"},
            "base": {"ref": "main", "sha": "bbb222"},
            "additions": 42,
            "deletions": 7,
            "changed_files": 3,
            "labels": [{"name": "backend"}],
            "state": "open"
        })
    }

    fn check_runs_json() -> Value {
        json!({
            "total_count": 2,
            "check_runs": [
                {"id": 101, "name": "build", "status": "completed", "conclusion": "success",
                 "html_url": "https://github.com/acme/widgets/runs/101", "output": {"title": "Build passed", "summary": null}},
                {"id": 102, "name": "lint", "status": "in_progress", "conclusion": null,
                 "html_url": null, "output": {"title": null}}
            ]
        })
    }

    fn statuses_json() -> Value {
        json!({
            "state": "failure",
            "statuses": [
                {"id": 5, "context": "ci/legacy", "state": "failure",
                 "target_url": "https://ci.example/5", "description": "Failed"}
            ]
        })
    }

    fn search_json() -> Value {
        json!({
            "total_count": 1,
            "incomplete_results": false,
            "items": [{
                "id": 9001,
                "number": 7,
                "title": "Add retry to the sync loop",
                "html_url": "https://github.com/acme/widgets/pull/7",
                "state": "open",
                "repository_url": "https://api.github.com/repos/acme/widgets",
                "user": {"login": "bob"},
                "labels": [],
                "pull_request": {"url": "https://api.github.com/repos/acme/widgets/pulls/7"}
            }]
        })
    }

    const SEARCH_URL: &str = "https://api.github.com/search/issues?q=is%3Aopen%20is%3Apr%20archived%3Afalse%20review-requested%3A%40me&per_page=50&sort=updated&order=desc";
    const PULL_URL: &str = "https://api.github.com/repos/acme/widgets/pulls/7";
    const CHECK_RUNS_URL: &str =
        "https://api.github.com/repos/acme/widgets/commits/aaa111/check-runs?per_page=100";
    const STATUS_URL: &str = "https://api.github.com/repos/acme/widgets/commits/aaa111/status";
    const REVIEWS_URL: &str =
        "https://api.github.com/repos/acme/widgets/pulls/7/reviews?per_page=100";
    const FILES_URL: &str = "https://api.github.com/repos/acme/widgets/pulls/7/files?per_page=100";
    const REVIEW_URL: &str = "https://api.github.com/repos/acme/widgets/pulls/7/reviews";

    /// Routes for a full list: the search, the pull, checks, reviews and the decision.
    fn list_routes(reviews: Value, decision: Value) -> Vec<(String, MockResponse)> {
        vec![
            (format!("GET {SEARCH_URL}"), ok(search_json())),
            (format!("GET {PULL_URL}"), ok(pull_json())),
            (format!("GET {CHECK_RUNS_URL}"), ok(check_runs_json())),
            (format!("GET {STATUS_URL}"), ok(statuses_json())),
            (format!("GET {REVIEWS_URL}"), ok(reviews)),
            (format!("POST {GRAPHQL}"), ok(decision)),
        ]
    }

    fn decision(value: &str) -> Value {
        json!({"data": {"repository": {"pullRequest": {"reviewDecision": value}}}})
    }

    fn draft_comment(path: &str, body: &str, new_line: u32, head: &str) -> DraftComment {
        DraftComment {
            id: format!("draft-{new_line}"),
            item_id: "acct-1:acme/widgets:7".into(),
            body: body.into(),
            path: path.into(),
            new_line: Some(new_line),
            old_line: None,
            range: None,
            created_at: "2026-08-02T10:00:00.000Z".into(),
            refs: DiffRefs {
                base_sha: Some("bbb222".into()),
                start_sha: None,
                head_sha: Some(head.into()),
            },
        }
    }

    fn refs_at(head: &str) -> DiffRefs {
        DiffRefs {
            base_sha: Some("bbb222".into()),
            start_sha: None,
            head_sha: Some(head.into()),
        }
    }

    // --- Connect -----------------------------------------------------------------

    #[test]
    fn graphql_root_follows_github_com_and_enterprise_server_apart() {
        // GitHub.com serves GraphQL on the API host it already uses.
        assert_eq!(
            graphql_root("https://api.github.com"),
            "https://api.github.com/graphql"
        );

        // Enterprise Server serves it beside the REST root, one segment up from /api/v3.
        assert_eq!(
            graphql_root("https://github.acme.com/api/v3"),
            "https://github.acme.com/api/graphql"
        );
        assert_eq!(
            graphql_root("https://github.acme.com/api/v3/"),
            "https://github.acme.com/api/graphql"
        );
        assert_eq!(
            graphql_root("https://acme.dev/github/api/v3"),
            "https://acme.dev/github/api/graphql"
        );
    }

    #[test]
    fn connect_resolves_github_com_identity_with_a_default_label() {
        let (http, calls) = serve(vec![(
            format!("GET {API}/user"),
            ok(
                json!({"login": "octocat", "name": "The Octocat", "avatar_url": "https://avatars.example/octocat"}),
            ),
        )]);
        let draft = AccountDraft {
            kind: ProviderKind::Github,
            label: String::new(),
            host: String::new(),
            token: "ghp_test".into(),
            username: None,
            agent_command: None,
        };
        let account = block_on(connect(&http, &draft)).expect("connects");

        assert_eq!(account.kind, ProviderKind::Github);
        assert_eq!(account.label, "GitHub (octocat)");
        assert_eq!(account.base_url, API);
        assert_eq!(account.web_url, "https://github.com");
        assert_eq!(account.username, "octocat");
        assert_eq!(account.display_name, "The Octocat");
        assert_eq!(account.avatar_url, "https://avatars.example/octocat");
        assert_eq!(account.agent_command, None);

        let lock = calls.lock();
        let call = &lock[0];
        assert_eq!(call.key, "GET https://api.github.com/user");
    }

    #[test]
    fn connect_sends_the_bearer_token_and_the_github_headers() {
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let log = Arc::clone(&seen);
        let http = Http::mock(move |request: &MockRequest| {
            log.lock().push((
                request
                    .header("authorization")
                    .unwrap_or_default()
                    .to_string(),
                request.header("user-agent").unwrap_or_default().to_string(),
            ));
            ok(json!({"login": "octocat"}))
        });
        let draft = AccountDraft {
            kind: ProviderKind::Github,
            label: String::new(),
            host: "github.com".into(),
            token: "ghp_test".into(),
            username: None,
            agent_command: None,
        };
        block_on(connect(&http, &draft)).expect("connects");
        assert_eq!(
            seen.lock().first().cloned(),
            Some(("Bearer ghp_test".to_string(), "Reviewdeck".to_string()))
        );
    }

    #[test]
    fn connect_derives_enterprise_roots_and_keeps_the_given_label() {
        let (http, calls) = serve(vec![(
            "GET https://github.acme.com/api/v3/user".into(),
            ok(
                json!({"login": "carol", "name": null, "avatar_url": "https://github.acme.com/avatars/carol"}),
            ),
        )]);
        let draft = AccountDraft {
            kind: ProviderKind::Github,
            label: "Work".into(),
            host: "https://github.acme.com/".into(),
            token: "t".into(),
            username: None,
            agent_command: None,
        };
        let account = block_on(connect(&http, &draft)).expect("connects");

        assert_eq!(account.label, "Work");
        assert_eq!(account.base_url, "https://github.acme.com/api/v3");
        assert_eq!(account.web_url, "https://github.acme.com");
        // A null name falls back to the login.
        assert_eq!(account.display_name, "carol");
        assert_eq!(
            keys(&calls),
            vec!["GET https://github.acme.com/api/v3/user"]
        );
    }

    #[test]
    fn connect_reports_a_rejected_token_in_the_hosts_words() {
        let (http, _) = serve(vec![(
            format!("GET {API}/user"),
            MockResponse::json(401, &json!({"message": "Bad credentials"})),
        )]);
        let draft = AccountDraft {
            kind: ProviderKind::Github,
            label: String::new(),
            host: String::new(),
            token: "bad".into(),
            username: None,
            agent_command: None,
        };
        let error = block_on(connect(&http, &draft)).expect_err("rejected");
        assert_eq!(
            error.to_string(),
            "Not authorised on api.github.com - the token is invalid or expired."
        );
    }

    #[test]
    fn connect_refuses_a_blank_host_and_an_unreachable_one() {
        let (http, calls) = serve(Vec::new());
        let blank = AccountDraft {
            kind: ProviderKind::Github,
            label: String::new(),
            host: "   ".into(),
            token: "t".into(),
            username: None,
            agent_command: None,
        };
        assert_eq!(
            block_on(connect(&http, &blank))
                .expect_err("blank")
                .to_string(),
            "A host is required."
        );
        assert!(calls.lock().is_empty(), "no request for a blank host");

        let unreachable = Http::mock(|_| MockResponse::unreachable("connection refused"));
        let draft = AccountDraft {
            kind: ProviderKind::Github,
            label: String::new(),
            host: String::new(),
            token: "t".into(),
            username: None,
            agent_command: None,
        };
        // A transport failure is an error, and the user reads it as it is.
        let error = block_on(connect(&unreachable, &draft)).expect_err("unreachable");
        assert!(error.to_string().contains("api.github.com"), "{error}");
    }

    // --- Listing -----------------------------------------------------------------

    #[test]
    fn list_review_requests_builds_every_field_of_an_item() {
        let (http, calls) = serve(list_routes(
            json!([
                {"id": 1, "user": {"login": "alice"}, "state": "APPROVED"},
                {"id": 2, "user": {"login": "octocat"}, "state": "COMMENTED"}
            ]),
            decision("REVIEW_REQUIRED"),
        ));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items.len(), 1);
        let item = &items[0];

        assert_eq!(item.id, "acct-1:acme/widgets:7");
        assert_eq!(item.account_id, "acct-1");
        assert_eq!(item.provider, ProviderKind::Github);
        assert_eq!(item.repo_key, "acme/widgets");
        assert_eq!(item.repo, "acme/widgets");
        assert_eq!(item.number, 7);
        assert_eq!(item.title, "Add retry to the sync loop");
        assert_eq!(item.url, "https://github.com/acme/widgets/pull/7");
        assert_eq!(
            item.author,
            User {
                name: "bob".into(),
                avatar_url: "https://avatars.example/bob".into()
            }
        );
        assert_eq!(item.created_at, "2026-08-01T09:00:00Z");
        assert_eq!(item.updated_at, "2026-08-03T12:00:00Z");
        assert!(!item.draft);
        assert_eq!(item.source_branch, "retry");
        assert_eq!(item.target_branch, "main");
        assert_eq!(item.labels, vec!["backend".to_string()]);
        // Octocat only commented, so the verdict is "commented"; alice's approval counts.
        assert_eq!(item.my_review_state, MyReviewState::Commented);
        assert_eq!(
            item.approvals,
            ApprovalSummary {
                given: 1,
                required: None,
                outcome: ApprovalOutcome::Pending
            }
        );
        assert_eq!(item.checks.status, CheckStatus::Failed);
        assert_eq!(item.checks.passed, 1);
        assert_eq!(item.checks.failed, 1);
        assert_eq!(item.checks.running, 1);
        assert_eq!(item.checks.total, 3);
        let runs: Vec<(&str, &str, CheckStatus)> = item
            .checks
            .runs
            .iter()
            .map(|run| (run.id.as_str(), run.name.as_str(), run.status))
            .collect();
        assert_eq!(
            runs,
            vec![
                ("check-101", "build", CheckStatus::Passed),
                ("check-102", "lint", CheckStatus::Running),
                ("status-5", "ci/legacy", CheckStatus::Failed),
            ]
        );
        assert_eq!(
            item.checks.runs[0].url.as_deref(),
            Some("https://github.com/acme/widgets/runs/101")
        );
        assert_eq!(
            item.checks.runs[0].description.as_deref(),
            Some("Build passed")
        );
        assert_eq!(item.checks.runs[1].url, None);
        assert_eq!(
            item.checks.runs[2].url.as_deref(),
            Some("https://ci.example/5")
        );
        assert_eq!(item.checks.runs[2].description.as_deref(), Some("Failed"));
        assert_eq!(item.additions, Some(42));
        assert_eq!(item.deletions, Some(7));
        assert_eq!(item.changed_files, Some(3));

        let sent = keys(&calls);
        for expected in [
            format!("GET {SEARCH_URL}"),
            format!("GET {PULL_URL}"),
            format!("GET {CHECK_RUNS_URL}"),
            format!("GET {STATUS_URL}"),
            format!("GET {REVIEWS_URL}"),
            format!("POST {GRAPHQL}"),
        ] {
            assert!(sent.contains(&expected), "missing {expected} in {sent:?}");
        }
    }

    #[test]
    fn list_review_requests_sends_the_decision_query_with_its_variables() {
        let (http, calls) = serve(list_routes(json!([]), decision("APPROVED")));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].approvals.outcome, ApprovalOutcome::Satisfied);
        let body = body_of(&calls, &format!("POST {GRAPHQL}"));
        assert_eq!(
            body["variables"],
            json!({"owner": "acme", "name": "widgets", "number": 7})
        );
        assert!(
            body["query"]
                .as_str()
                .unwrap_or_default()
                .contains("reviewDecision")
        );
    }

    #[test]
    fn my_review_state_follows_the_last_non_comment_verdict() {
        let cases = [
            (
                json!([{"user": {"login": "octocat"}, "state": "APPROVED"}, {"user": {"login": "octocat"}, "state": "COMMENTED"}]),
                MyReviewState::Approved,
            ),
            (
                json!([{"user": {"login": "octocat"}, "state": "CHANGES_REQUESTED"}, {"user": {"login": "octocat"}, "state": "COMMENTED"}]),
                MyReviewState::ChangesRequested,
            ),
            (
                json!([{"user": {"login": "octocat"}, "state": "COMMENTED"}]),
                MyReviewState::Commented,
            ),
            (
                json!([{"user": {"login": "someone"}, "state": "APPROVED"}]),
                MyReviewState::Pending,
            ),
            (json!([]), MyReviewState::Pending),
        ];
        for (reviews, expected) in cases {
            let (http, _) = serve(list_routes(reviews.clone(), decision("REVIEW_REQUIRED")));
            let items = block_on(list_review_requests(&http, &session())).expect("lists");
            assert_eq!(items[0].my_review_state, expected, "{reviews}");
        }
    }

    #[test]
    fn approvals_count_only_latest_standing_approvals() {
        // Alice approved and then withdrew; bob approved; carol only commented.
        let reviews = json!([
            {"user": {"login": "alice"}, "state": "APPROVED"},
            {"user": {"login": "alice"}, "state": "DISMISSED"},
            {"user": {"login": "bob"}, "state": "APPROVED"},
            {"user": {"login": "carol"}, "state": "COMMENTED"}
        ]);
        let (http, _) = serve(list_routes(reviews, decision("APPROVED")));
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].approvals.given, 1);
    }

    #[test]
    fn a_failing_checks_read_is_an_empty_checks_list_not_a_failed_item() {
        let mut routes = list_routes(json!([]), decision("REVIEW_REQUIRED"));
        routes.retain(|(key, _)| !key.contains("/commits/"));
        routes.push((
            format!("GET {CHECK_RUNS_URL}"),
            MockResponse::json(500, &json!({"message": "boom"})),
        ));
        let (http, _) = serve(routes);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].checks, empty_checks());
    }

    #[test]
    fn checks_that_are_absent_on_the_commit_are_not_an_error() {
        let mut routes = list_routes(json!([]), decision("REVIEW_REQUIRED"));
        routes.retain(|(key, _)| !key.contains("/commits/"));
        routes.push((
            format!("GET {CHECK_RUNS_URL}"),
            MockResponse::json(404, &json!({"message": "Not Found"})),
        ));
        routes.push((
            format!("GET {STATUS_URL}"),
            MockResponse::json(404, &json!({"message": "Not Found"})),
        ));
        let (http, _) = serve(routes);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].checks, empty_checks());
    }

    #[test]
    fn a_failing_reviews_read_and_a_refused_decision_leave_the_item_pending() {
        let mut routes = list_routes(json!([]), decision("APPROVED"));
        routes.retain(|(key, _)| !key.contains("/reviews"));
        routes.push((
            format!("GET {REVIEWS_URL}"),
            MockResponse::json(403, &json!({"message": "Forbidden"})),
        ));
        routes.retain(|(key, _)| !key.starts_with("POST"));
        routes.push((
            format!("POST {GRAPHQL}"),
            ok(json!({"data": null, "errors": [{"message": "Resource not accessible"}]})),
        ));
        let (http, _) = serve(routes);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items[0].my_review_state, MyReviewState::Pending);
        assert_eq!(
            items[0].approvals,
            ApprovalSummary {
                given: 0,
                required: None,
                outcome: ApprovalOutcome::NoneRequired
            }
        );
    }

    #[test]
    fn a_missing_optional_field_never_fails_the_sync() {
        let sparse_pull = json!({
            "title": "Bare",
            "html_url": "https://github.com/acme/widgets/pull/7",
            "user": {"login": "bob"},
            "head": {"ref": "retry", "sha": "aaa111"},
            "base": {"ref": "main"}
        });
        let mut routes = list_routes(json!([]), decision("REVIEW_REQUIRED"));
        routes.retain(|(key, _)| !key.ends_with(PULL_URL));
        routes.push((format!("GET {PULL_URL}"), ok(sparse_pull)));
        let (http, _) = serve(routes);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        let item = &items[0];
        assert_eq!(item.title, "Bare");
        assert!(!item.draft);
        assert!(item.labels.is_empty());
        assert_eq!(item.additions, None);
        assert_eq!(item.changed_files, None);
        assert_eq!(item.author.avatar_url, "");
        assert_eq!(item.target_branch, "main");
    }

    #[test]
    fn a_null_search_list_is_an_empty_deck_and_a_failed_search_is_an_error() {
        let (http, _) = serve(vec![(
            format!("GET {SEARCH_URL}"),
            ok(json!({"items": null})),
        )]);
        assert!(
            block_on(list_review_requests(&http, &session()))
                .expect("lists")
                .is_empty()
        );

        let (http, _) = serve(vec![(
            format!("GET {SEARCH_URL}"),
            MockResponse::json(500, &json!({"message": "down"})),
        )]);
        assert_eq!(
            block_on(list_review_requests(&http, &session()))
                .expect_err("search failed")
                .to_string(),
            "api.github.com returned a server error (500)."
        );
    }

    #[test]
    fn search_entries_without_a_number_or_repository_are_skipped() {
        let search = json!({"items": [
            {"title": "no number"},
            {"number": 7, "repository_url": "https://api.github.com/repos/acme/widgets"}
        ]});
        let routes = vec![
            (format!("GET {SEARCH_URL}"), ok(search)),
            (format!("GET {PULL_URL}"), ok(pull_json())),
            (format!("GET {CHECK_RUNS_URL}"), ok(check_runs_json())),
            (format!("GET {STATUS_URL}"), ok(statuses_json())),
            (format!("GET {REVIEWS_URL}"), ok(json!([]))),
            (format!("POST {GRAPHQL}"), ok(decision("REVIEW_REQUIRED"))),
        ];
        let (http, _) = serve(routes);
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 7);
    }

    // --- Detail ------------------------------------------------------------------

    fn detail_routes(files: Vec<(String, MockResponse)>) -> Vec<(String, MockResponse)> {
        let mut routes = vec![
            (format!("GET {PULL_URL}"), ok(pull_json())),
            (
                format!("POST {GRAPHQL}"),
                ok(json!({"data": {"repository": {"pullRequest": {
                    "reviewThreads": {"nodes": []},
                    "comments": {"nodes": []}
                }}}})),
            ),
        ];
        routes.extend(files);
        routes
    }

    #[test]
    fn load_detail_maps_renames_binaries_and_too_large_files() {
        let files = json!([
            {"filename": "src/sync.rs", "status": "modified", "additions": 10, "deletions": 2, "patch": "@@ -1 +1 @@\n-a\n+b"},
            {"filename": "src/new/name.rs", "previous_filename": "src/old/name.rs", "status": "renamed", "additions": 0, "deletions": 0, "patch": "@@ -0,0 +0,0 @@"},
            {"filename": "assets/logo.png", "status": "added", "additions": 0, "deletions": 0},
            {"filename": "docs/gone.md", "status": "removed", "additions": 0, "deletions": 9, "patch": "@@ -1,9 +0,0 @@"},
            {"filename": "generated/huge.json", "status": "modified", "additions": 4000, "deletions": 3}
        ]);
        let (http, _) = serve(detail_routes(vec![(format!("GET {FILES_URL}"), ok(files))]));
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");

        let files = &detail.files;
        assert_eq!(files.len(), 5);

        assert_eq!(files[0].path, "src/sync.rs");
        assert_eq!(files[0].old_path, "src/sync.rs");
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!((files[0].additions, files[0].deletions), (10, 2));
        assert_eq!(files[0].patch.as_deref(), Some("@@ -1 +1 @@\n-a\n+b"));
        assert!(!files[0].binary);

        assert_eq!(files[1].path, "src/new/name.rs");
        assert_eq!(files[1].old_path, "src/old/name.rs");
        assert_eq!(files[1].status, FileStatus::Renamed);

        // No patch and no changes: a binary.
        assert_eq!(files[2].status, FileStatus::Added);
        assert_eq!(files[2].patch, None);
        assert!(files[2].binary);

        assert_eq!(files[3].status, FileStatus::Removed);
        assert_eq!(files[3].deletions, 9);

        // No patch, but changes: too large for the host to send, not binary.
        assert_eq!(files[4].patch, None);
        assert_eq!((files[4].additions, files[4].deletions), (4000, 3));
        assert!(!files[4].binary);
    }

    #[test]
    fn load_detail_fills_the_item_refs_and_description() {
        let (http, _) = serve(detail_routes(vec![(
            format!("GET {FILES_URL}"),
            ok(json!([])),
        )]));
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert_eq!(detail.description, "Retries three times before giving up.");
        assert_eq!(detail.item.additions, Some(42));
        assert_eq!(detail.item.deletions, Some(7));
        assert_eq!(detail.item.changed_files, Some(3));
        assert_eq!(
            detail.refs,
            DiffRefs {
                base_sha: Some("bbb222".into()),
                start_sha: None,
                head_sha: Some("aaa111".into()),
            }
        );
        assert!(detail.threads.is_empty());
    }

    #[test]
    fn a_null_pull_body_is_an_empty_description() {
        let mut pull = pull_json();
        pull["body"] = Value::Null;
        let mut routes = detail_routes(vec![(format!("GET {FILES_URL}"), ok(json!([])))]);
        routes.retain(|(key, _)| key != &format!("GET {PULL_URL}"));
        routes.push((format!("GET {PULL_URL}"), ok(pull)));
        let (http, _) = serve(routes);
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert_eq!(detail.description, "");
    }

    #[test]
    fn load_detail_follows_file_pages_up_to_four() {
        let page = |n: u32| {
            let next = format!(
                "<https://api.github.com/repos/acme/widgets/pulls/7/files?per_page=100&page={}>; rel=\"next\"",
                n + 1
            );
            let body = json!([{"filename": format!("f{n}.rs"), "status": "modified", "additions": 1, "deletions": 0, "patch": "@@ -0,0 +1 @@\n+x"}]);
            MockResponse::json(200, &body).header("Link", next)
        };
        let mut files = vec![(FILES_URL.to_string(), page(1))];
        for n in 2..=5 {
            files.push((
                format!(
                    "https://api.github.com/repos/acme/widgets/pulls/7/files?per_page=100&page={n}"
                ),
                page(n),
            ));
        }
        let routes = detail_routes(Vec::new());
        let mut all = routes;
        all.extend(
            files
                .into_iter()
                .map(|(url, response)| (format!("GET {url}"), response)),
        );
        let (http, calls) = serve(all);
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        assert_eq!(detail.files.len(), 4, "four pages, then the cap");
        let file_calls = keys(&calls)
            .iter()
            .filter(|key| key.contains("/files"))
            .count();
        assert_eq!(file_calls, 4);
    }

    #[test]
    fn a_files_failure_fails_the_detail_load() {
        let (http, _) = serve(detail_routes(vec![(
            format!("GET {FILES_URL}"),
            MockResponse::json(500, &json!({"message": "Server Error"})),
        )]));
        assert_eq!(
            block_on(load_detail(&http, &session(), &item()))
                .expect_err("files failed")
                .to_string(),
            "api.github.com returned a server error (500)."
        );
    }

    #[test]
    fn load_detail_reads_review_threads_over_graphql() {
        let threads = json!({"data": {"repository": {"pullRequest": {
            "reviewThreads": {"nodes": [{
                "id": "PRRT_1",
                "isResolved": false,
                "isOutdated": false,
                "path": "src/sync.rs",
                "line": 12,
                "startLine": null,
                "diffSide": "RIGHT",
                "startDiffSide": null,
                "viewerCanReply": true,
                "viewerCanResolve": true,
                "viewerCanUnresolve": false,
                "comments": {"nodes": [{
                    "id": "PRRC_1",
                    "body": "Why three?",
                    "createdAt": "2026-08-02T10:00:00Z",
                    "author": {"login": "alice", "avatarUrl": "https://avatars.example/alice"}
                }]}
            }]},
            "comments": {"nodes": [null]}
        }}}});
        let mut routes = detail_routes(vec![(format!("GET {FILES_URL}"), ok(json!([])))]);
        routes.retain(|(key, _)| !key.starts_with("POST"));
        routes.push((format!("POST {GRAPHQL}"), ok(threads)));
        let (http, _) = serve(routes);
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");

        assert_eq!(detail.threads.len(), 1);
        let thread = &detail.threads[0];
        assert_eq!(thread.id, "PRRT_1");
        assert_eq!(thread.path.as_deref(), Some("src/sync.rs"));
        assert_eq!(thread.line, Some(12));
        assert_eq!(thread.side, Some(Side::New));
        assert!(thread.can_reply);
        assert!(thread.can_resolve);
        assert!(!thread.resolved);
        assert_eq!(thread.comments.len(), 1);
        assert_eq!(thread.comments[0].author.name, "alice");
        assert_eq!(thread.comments[0].body, "Why three?");
    }

    #[test]
    fn threads_fall_back_to_the_flat_rest_shape_when_graphql_refuses() {
        let mut routes = detail_routes(vec![(format!("GET {FILES_URL}"), ok(json!([])))]);
        routes.retain(|(key, _)| !key.starts_with("POST"));
        routes.push((
            format!("POST {GRAPHQL}"),
            ok(json!({"data": null, "errors": [{"message": "Something went wrong"}]})),
        ));
        routes.push((
            format!("GET {API}/repos/acme/widgets/issues/7/comments?per_page=100"),
            ok(json!([{
                "id": 900,
                "user": {"login": "carol", "avatar_url": "https://avatars.example/carol"},
                "body": "General note",
                "created_at": "2026-08-01T11:00:00Z"
            }])),
        ));
        routes.push((
            format!("GET {API}/repos/acme/widgets/pulls/7/comments?per_page=100"),
            ok(json!([{
                "id": 901,
                "user": {"login": "dave", "avatar_url": "https://avatars.example/dave"},
                "body": "Inline remark",
                "created_at": "2026-08-01T12:00:00Z",
                "path": "src/sync.rs",
                "line": 12,
                "side": "RIGHT"
            }])),
        ));
        let (http, _) = serve(routes);
        let detail = block_on(load_detail(&http, &session(), &item())).expect("loads");
        let bodies: Vec<&str> = detail
            .threads
            .iter()
            .flat_map(|thread| thread.comments.iter().map(|comment| comment.body.as_str()))
            .collect();
        assert!(bodies.contains(&"General note"), "{bodies:?}");
        assert!(bodies.contains(&"Inline remark"), "{bodies:?}");
    }

    #[test]
    fn threads_fall_back_when_graphql_answers_with_a_broken_shape() {
        let mut routes = detail_routes(vec![(format!("GET {FILES_URL}"), ok(json!([])))]);
        routes.retain(|(key, _)| !key.starts_with("POST"));
        routes.push((
            format!("POST {GRAPHQL}"),
            ok(json!({"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": "nope"}}}}})),
        ));
        let (http, calls) = serve(routes);
        block_on(load_detail(&http, &session(), &item())).expect("loads");
        let sent = keys(&calls);
        assert!(sent.contains(&format!(
            "GET {API}/repos/acme/widgets/issues/7/comments?per_page=100"
        )));
    }

    // --- Refresh and threads ---------------------------------------------------

    #[test]
    fn refresh_checks_reads_the_head_commit_of_the_pull() {
        let (http, calls) = serve(vec![
            (format!("GET {PULL_URL}"), ok(pull_json())),
            (format!("GET {CHECK_RUNS_URL}"), ok(check_runs_json())),
            (format!("GET {STATUS_URL}"), ok(json!({"statuses": []}))),
        ]);
        let checks = block_on(refresh_checks(&http, &session(), &item())).expect("refreshes");
        assert_eq!(checks.total, 2);
        assert_eq!(checks.status, CheckStatus::Running);
        assert_eq!(keys(&calls)[0], format!("GET {PULL_URL}"));
    }

    #[test]
    fn refresh_checks_propagates_a_failure_other_than_not_found() {
        let (http, _) = serve(vec![
            (format!("GET {PULL_URL}"), ok(pull_json())),
            (
                format!("GET {CHECK_RUNS_URL}"),
                MockResponse::json(502, &json!({"message": "Bad Gateway"})),
            ),
        ]);
        assert!(block_on(refresh_checks(&http, &session(), &item())).is_err());
    }

    #[test]
    fn load_threads_is_the_threads_half_of_the_detail() {
        let mut routes = detail_routes(Vec::new());
        routes.retain(|(key, _)| !key.starts_with("POST"));
        routes.push((
            format!("POST {GRAPHQL}"),
            ok(json!({"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": []}, "comments": {"nodes": [{
                "id": "IC_1", "body": "Thanks!", "createdAt": "2026-08-02T11:00:00Z",
                "author": {"login": "bob", "avatarUrl": "https://avatars.example/bob"}
            }]}}}}})),
        ));
        let (http, _) = serve(routes);
        let threads = block_on(load_threads(&http, &session(), &item())).expect("loads");
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].comments[0].body, "Thanks!");
    }

    #[test]
    fn check_status_maps_each_conclusion() {
        let run = |status: &str, conclusion: Option<&str>| GhCheckRun {
            id: 1,
            status: Some(status.into()),
            conclusion: conclusion.map(str::to_string),
            ..GhCheckRun::default()
        };
        assert_eq!(check_status(&run("queued", None)), CheckStatus::Running);
        assert_eq!(
            check_status(&run("completed", Some("success"))),
            CheckStatus::Passed
        );
        for conclusion in [
            "failure",
            "timed_out",
            "startup_failure",
            "cancelled",
            "action_required",
        ] {
            assert_eq!(
                check_status(&run("completed", Some(conclusion))),
                CheckStatus::Failed,
                "{conclusion}"
            );
        }
        for conclusion in ["neutral", "skipped", "success_but_ignored"] {
            assert_eq!(
                check_status(&run("completed", Some(conclusion))),
                CheckStatus::Passed,
                "{conclusion}"
            );
        }
        assert_eq!(
            check_status(&run("completed", Some("stale"))),
            CheckStatus::Unknown
        );
        assert_eq!(check_status(&run("completed", None)), CheckStatus::Unknown);
        assert_eq!(status_state(Some("error")), CheckStatus::Failed);
        assert_eq!(status_state(Some("pending")), CheckStatus::Running);
        assert_eq!(status_state(Some("weird")), CheckStatus::Unknown);
    }

    #[test]
    fn repo_helpers_split_keys_and_urls() {
        assert_eq!(split_repo("acme/widgets"), ("acme", "widgets"));
        assert_eq!(split_repo("lonely"), ("lonely", "lonely"));
        assert_eq!(
            repo_from_url("https://api.github.com/repos/acme/widgets"),
            "acme/widgets"
        );
        assert_eq!(repo_from_url("not a repo url"), "not a repo url");
    }

    // --- Submitting --------------------------------------------------------------

    fn review_call() -> String {
        format!("POST {REVIEW_URL}")
    }

    #[test]
    fn submit_review_without_drafts_sends_only_the_verdict_and_a_body() {
        for (verdict, event) in [
            (ReviewVerdict::Approve, "APPROVE"),
            (ReviewVerdict::RequestChanges, "REQUEST_CHANGES"),
            (ReviewVerdict::Comment, "COMMENT"),
        ] {
            let (http, calls) = serve(vec![(review_call(), ok(json!({"id": 1})))]);
            block_on(submit_review(
                &http,
                &session(),
                &item(),
                verdict,
                "Looks good",
                &[],
            ))
            .expect("submits");
            assert_eq!(keys(&calls), vec![review_call()]);
            assert_eq!(
                body_of(&calls, &review_call()),
                json!({"event": event, "body": "Looks good"}),
                "{verdict:?}"
            );
        }
    }

    #[test]
    fn submit_review_leaves_out_an_empty_body_and_the_comment_list() {
        let (http, calls) = serve(vec![(review_call(), ok(json!({"id": 1})))]);
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Comment,
            "",
            &[],
        ))
        .expect("submits");
        assert_eq!(body_of(&calls, &review_call()), json!({"event": "COMMENT"}));
    }

    #[test]
    fn submit_review_with_drafts_posts_them_against_the_commit_they_were_written_on() {
        let (http, calls) = serve(vec![(review_call(), ok(json!({"id": 1})))]);
        let drafts = vec![
            draft_comment("src/sync.rs", "Nit", 12, "aaa111"),
            draft_comment("src/other.rs", "Shadowed name", 3, "aaa111"),
        ];
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::RequestChanges,
            "Two things",
            &drafts,
        ))
        .expect("submits");
        assert_eq!(
            keys(&calls),
            vec![review_call()],
            "one batch call, not one per draft"
        );
        assert_eq!(
            body_of(&calls, &review_call()),
            json!({
                "event": "REQUEST_CHANGES",
                "body": "Two things",
                "commit_id": "aaa111",
                "comments": [
                    {"path": "src/sync.rs", "body": "Nit", "side": "RIGHT", "line": 12},
                    {"path": "src/other.rs", "body": "Shadowed name", "side": "RIGHT", "line": 3}
                ]
            })
        );
    }

    #[test]
    fn submit_review_places_a_range_by_its_start_line() {
        let (http, calls) = serve(vec![(review_call(), ok(json!({"id": 1})))]);
        let mut draft = draft_comment("src/sync.rs", "Extract this", 14, "aaa111");
        draft.range = Some(LineRange {
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
        });
        block_on(submit_review(
            &http,
            &session(),
            &item(),
            ReviewVerdict::Comment,
            "",
            &[draft],
        ))
        .expect("submits");
        assert_eq!(
            body_of(&calls, &review_call())["comments"][0],
            json!({"path": "src/sync.rs", "body": "Extract this", "side": "RIGHT", "line": 14, "start_side": "RIGHT", "start_line": 10})
        );
    }

    #[test]
    fn submit_review_reports_a_rejected_verdict_in_the_hosts_words() {
        let (http, _) = serve(vec![(
            review_call(),
            MockResponse::json(
                422,
                &json!({"message": "Can not approve your own pull request"}),
            ),
        )]);
        assert_eq!(
            block_on(submit_review(
                &http,
                &session(),
                &item(),
                ReviewVerdict::Approve,
                "",
                &[]
            ))
            .expect_err("rejected")
            .to_string(),
            "Can not approve your own pull request"
        );
    }

    #[test]
    fn add_comment_posts_an_issue_comment() {
        let url = format!("POST {API}/repos/acme/widgets/issues/7/comments");
        let (http, calls) = serve(vec![(url.clone(), ok(json!({"id": 2})))]);
        block_on(add_comment(&http, &session(), &item(), "Ping")).expect("posts");
        assert_eq!(keys(&calls), vec![url.clone()]);
        assert_eq!(body_of(&calls, &url), json!({"body": "Ping"}));
    }

    #[test]
    fn add_line_comment_posts_a_review_comment_on_the_head_commit() {
        let url = format!("POST {API}/repos/acme/widgets/pulls/7/comments");
        let (http, calls) = serve(vec![(url.clone(), ok(json!({"id": 3})))]);
        let draft = LineCommentDraft {
            item_id: "acct-1:acme/widgets:7".into(),
            body: "Off by one".into(),
            path: "src/sync.rs".into(),
            new_line: Some(4),
            old_line: None,
            range: None,
        };
        block_on(add_line_comment(
            &http,
            &session(),
            &item(),
            &draft,
            &refs_at("aaa111"),
        ))
        .expect("posts");
        assert_eq!(
            body_of(&calls, &url),
            json!({"commit_id": "aaa111", "path": "src/sync.rs", "body": "Off by one", "side": "RIGHT", "line": 4})
        );
    }

    #[test]
    fn add_line_comment_needs_the_head_commit_and_sends_nothing_without_it() {
        let (http, calls) = serve(Vec::new());
        let draft = LineCommentDraft {
            item_id: "acct-1:acme/widgets:7".into(),
            body: "x".into(),
            path: "src/sync.rs".into(),
            new_line: Some(4),
            old_line: None,
            range: None,
        };
        let refs = DiffRefs::default();
        assert_eq!(
            block_on(add_line_comment(&http, &session(), &item(), &draft, &refs))
                .expect_err("no head")
                .to_string(),
            "Missing the head commit for this pull request."
        );
        assert!(calls.lock().is_empty());
    }

    #[test]
    fn reply_to_thread_sends_the_mutation_with_the_thread_id() {
        let (http, calls) = serve(vec![(
            format!("POST {GRAPHQL}"),
            ok(json!({"data": {"addPullRequestReviewThreadReply": {"clientMutationId": null}}})),
        )]);
        block_on(reply_to_thread(
            &http,
            &session(),
            &item(),
            "PRRT_1",
            "Done",
        ))
        .expect("replies");
        let body = body_of(&calls, &format!("POST {GRAPHQL}"));
        assert!(
            body["query"]
                .as_str()
                .unwrap_or_default()
                .contains("addPullRequestReviewThreadReply")
        );
        assert_eq!(
            body["variables"],
            json!({"threadId": "PRRT_1", "body": "Done"})
        );
    }

    #[test]
    fn reply_to_thread_reports_a_graphql_error_in_its_message() {
        let (http, _) = serve(vec![(
            format!("POST {GRAPHQL}"),
            ok(json!({"errors": [{"message": "Could not resolve to a node"}]})),
        )]);
        assert_eq!(
            block_on(reply_to_thread(
                &http,
                &session(),
                &item(),
                "PRRT_x",
                "Done"
            ))
            .expect_err("rejected")
            .to_string(),
            "Could not resolve to a node"
        );
    }

    #[test]
    fn graphql_without_data_or_a_message_uses_the_fallback_sentences() {
        let (http, _) = serve(vec![(format!("POST {GRAPHQL}"), ok(json!({"errors": []})))]);
        assert_eq!(
            block_on(set_thread_resolved(
                &http,
                &session(),
                &item(),
                "PRRT_1",
                true
            ))
            .expect_err("no data")
            .to_string(),
            "GitHub returned no data."
        );

        let (http, _) = serve(vec![(
            format!("POST {GRAPHQL}"),
            ok(json!({"data": null, "errors": [{"type": "FORBIDDEN"}]})),
        )]);
        assert_eq!(
            block_on(set_thread_resolved(
                &http,
                &session(),
                &item(),
                "PRRT_1",
                true
            ))
            .expect_err("rejected")
            .to_string(),
            "GitHub rejected the query."
        );
    }

    #[test]
    fn set_thread_resolved_picks_the_mutation_for_each_direction() {
        let (http, calls) = serve(vec![(
            format!("POST {GRAPHQL}"),
            ok(json!({"data": {"resolveReviewThread": {"clientMutationId": null}}})),
        )]);
        block_on(set_thread_resolved(
            &http,
            &session(),
            &item(),
            "PRRT_1",
            true,
        ))
        .expect("resolves");
        let body = body_of(&calls, &format!("POST {GRAPHQL}"));
        assert!(
            body["query"]
                .as_str()
                .unwrap_or_default()
                .contains("resolveReviewThread(")
        );
        assert_eq!(body["variables"], json!({"threadId": "PRRT_1"}));

        let (http, calls) = serve(vec![(
            format!("POST {GRAPHQL}"),
            ok(json!({"data": {"unresolveReviewThread": {"clientMutationId": null}}})),
        )]);
        block_on(set_thread_resolved(
            &http,
            &session(),
            &item(),
            "PRRT_1",
            false,
        ))
        .expect("reopens");
        let body = body_of(&calls, &format!("POST {GRAPHQL}"));
        assert!(
            body["query"]
                .as_str()
                .unwrap_or_default()
                .contains("unresolveReviewThread(")
        );
    }

    #[test]
    fn encode_uri_component_escapes_like_the_browser_does() {
        assert_eq!(
            encode_uri_component("is:open is:pr review-requested:@me"),
            "is%3Aopen%20is%3Apr%20review-requested%3A%40me"
        );
        assert_eq!(encode_uri_component("a.b_c!~*'()"), "a.b_c!~*'()");
    }

    #[test]
    fn a_session_authorises_with_the_bearer_scheme() {
        let session = session();
        assert_eq!(
            headers(&session.token)[0],
            ("Authorization".into(), "Bearer ghp_test".into())
        );
    }

    /// The query texts are what GitHub is sent, and a changed character is a changed
    /// request, so they are pinned against the TypeScript they were ported from.
    #[test]
    fn graphql_texts_are_byte_for_byte_the_typescript_ones() {
        let ts = include_str!("../../../../src/main/providers/github.ts");
        for text in [
            THREADS_QUERY,
            DECISION_QUERY,
            "mutation Reply($threadId: ID!, $body: String!) {\n        addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $threadId, body: $body }) {\n          clientMutationId\n        }\n      }",
            "mutation Resolve($threadId: ID!) {\n          resolveReviewThread(input: { threadId: $threadId }) { clientMutationId }\n        }",
            "mutation Unresolve($threadId: ID!) {\n          unresolveReviewThread(input: { threadId: $threadId }) { clientMutationId }\n        }",
        ] {
            assert!(ts.contains(text), "not in github.ts: {text}");
        }
    }

    #[test]
    fn the_mutations_sent_are_the_ones_pinned_above() {
        let (http, calls) = serve(vec![(
            format!("POST {GRAPHQL}"),
            ok(json!({"data": {"x": {}}})),
        )]);
        block_on(set_thread_resolved(&http, &session(), &item(), "T", false)).expect("ok");
        block_on(set_thread_resolved(&http, &session(), &item(), "T", true)).expect("ok");
        let calls = calls.lock();
        let query = |i: usize| -> String {
            let body: Value = serde_json::from_str(calls[i].body.as_deref().unwrap_or("{}"))
                .unwrap_or(Value::Null);
            body["query"].as_str().unwrap_or_default().to_string()
        };
        assert!(query(0).contains("unresolveReviewThread"));
        assert!(query(1).contains("mutation Resolve"));
    }

    /// More pull requests than the six workers, answered in input order.
    #[test]
    fn a_long_deck_keeps_the_search_order() {
        let count = 14u64;
        let http = Http::mock(move |request: &MockRequest| {
            let url = request.url.as_str();
            if url.starts_with("https://api.github.com/search/issues") {
                let items: Vec<Value> = (1..=count)
                    .map(|n| {
                        json!({"number": n, "repository_url": format!("https://api.github.com/repos/acme/r{n}")})
                    })
                    .collect();
                return ok(json!({"items": items}));
            }
            if url.ends_with("/graphql") {
                return ok(
                    json!({"data": {"repository": {"pullRequest": {"reviewDecision": null}}}}),
                );
            }
            if let Some(rest) = url.strip_prefix("https://api.github.com/repos/acme/r") {
                let n: u64 = rest
                    .split('/')
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                if rest.contains("/pulls/") && rest.ends_with("/reviews?per_page=100") {
                    return ok(json!([]));
                }
                if rest.contains("/pulls/") {
                    return ok(
                        json!({"title": format!("pr {n}"), "head": {"ref": "h", "sha": format!("s{n}")}, "base": {"ref": "main", "sha": "b"}}),
                    );
                }
                if rest.contains("/check-runs") {
                    return ok(json!({"check_runs": []}));
                }
                if rest.ends_with("/status") {
                    return ok(json!({"statuses": []}));
                }
            }
            MockResponse::new(599, format!("unexpected {url}"))
        });
        let items = block_on(list_review_requests(&http, &session())).expect("lists");
        let titles: Vec<String> = items.iter().map(|item| item.title.clone()).collect();
        let expected: Vec<String> = (1..=count).map(|n| format!("pr {n}")).collect();
        assert_eq!(titles, expected);
        assert_eq!(items[3].id, "acct-1:acme/r4:4");
    }
}
