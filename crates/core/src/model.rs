//! The data model: a port of src/shared/types.ts, plus the pure helpers of
//! src/main/providers/types.ts that every provider adapter shares.
//!
//! Everything here serialises to exactly the JSON the Electron app wrote - camelCase
//! fields, the TypeScript string literals for enums, optional fields left out when
//! absent - because the vault on disk is shared between the two.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Github,
    Gitlab,
    Forgejo,
    Bitbucket,
}

impl ProviderKind {
    /// Every provider, in the order the TypeScript registry lists them.
    pub const ALL: [ProviderKind; 4] = [
        ProviderKind::Github,
        ProviderKind::Gitlab,
        ProviderKind::Forgejo,
        ProviderKind::Bitbucket,
    ];

    /// The user-facing name: `PROVIDER_LABELS`.
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Github => "GitHub",
            ProviderKind::Gitlab => "GitLab",
            ProviderKind::Forgejo => "Forgejo / Gitea",
            ProviderKind::Bitbucket => "Bitbucket",
        }
    }

    /// The identifier as it is written to disk: `"github"` and so on.
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Github => "github",
            ProviderKind::Gitlab => "gitlab",
            ProviderKind::Forgejo => "forgejo",
            ProviderKind::Bitbucket => "bitbucket",
        }
    }
}

/// A signed-in account. The token itself never leaves the app state and the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub kind: ProviderKind,
    /// User-facing name, e.g. "Work GitHub".
    pub label: String,
    /// API root without a trailing slash, e.g. https://api.github.com
    pub base_url: String,
    /// Web root used to build browser links, e.g. https://github.com
    pub web_url: String,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
    pub added_at: String,
    /// Overrides the agent command for handoffs from this account, so a client's Git
    /// account can route through that client's Claude configuration. Blank means the
    /// setting applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_command: Option<String>,
}

/// What a provider's `connect` resolves: an [`Account`] before the vault gives it an
/// id and a date (`Omit<Account, 'id' | 'addedAt'>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    pub kind: ProviderKind,
    pub label: String,
    pub base_url: String,
    pub web_url: String,
    pub username: String,
    pub display_name: String,
    pub avatar_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_command: Option<String>,
}

impl NewAccount {
    /// The stored account, once the vault has picked its id and date.
    pub fn into_account(self, id: String, added_at: String) -> Account {
        Account {
            id,
            kind: self.kind,
            label: self.label,
            base_url: self.base_url,
            web_url: self.web_url,
            username: self.username,
            display_name: self.display_name,
            avatar_url: self.avatar_url,
            added_at,
            agent_command: self.agent_command,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDraft {
    pub kind: ProviderKind,
    pub label: String,
    /// Host as typed by the user, e.g. "gitlab.example.com" or a full URL.
    pub host: String,
    pub token: String,
    /// Bitbucket app passwords are tied to a username.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Per-account agent command; blank clears the override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_command: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Passed,
    Failed,
    Running,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckRun {
    pub id: String,
    pub name: String,
    pub status: CheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckSummary {
    pub status: CheckStatus,
    pub passed: u32,
    pub failed: u32,
    pub running: u32,
    pub total: u32,
    pub runs: Vec<CheckRun>,
}

/// How the signed-in user has reviewed a PR so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MyReviewState {
    Pending,
    Approved,
    ChangesRequested,
    Commented,
}

/// Where a pull request stands against the approvals its host wants before it will
/// merge.
///
/// `NoneRequired` is also the answer when the host will not say: a GitHub branch
/// with no protection and a Bitbucket repository the token cannot administer look
/// the same from here, and neither is a reason to hide anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalOutcome {
    NoneRequired,
    Pending,
    Satisfied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalSummary {
    /// Reviewers whose standing verdict is an approval, whoever they are.
    pub given: u32,
    /// How many the host asks for; absent when it does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<u32>,
    pub outcome: ApprovalOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub name: String,
    pub avatar_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    /// Stable across refreshes: `{accountId}:{repoKey}:{number}`.
    pub id: String,
    pub account_id: String,
    pub provider: ProviderKind,
    /// Provider-native handle for follow-up API calls (owner/repo, project id, ws/slug).
    pub repo_key: String,
    /// Human readable repository name.
    pub repo: String,
    pub number: u64,
    pub title: String,
    pub url: String,
    pub author: User,
    pub created_at: String,
    pub updated_at: String,
    pub draft: bool,
    pub source_branch: String,
    pub target_branch: String,
    pub labels: Vec<String>,
    pub my_review_state: MyReviewState,
    pub approvals: ApprovalSummary,
    pub checks: CheckSummary,
    /// Set when the provider reports it cheaply; otherwise filled in on detail load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_files: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileStatus {
    Added,
    Removed,
    Modified,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub old_path: String,
    pub status: FileStatus,
    pub additions: u32,
    pub deletions: u32,
    /// Raw unified diff body for this file; `None` (`null`) when the provider omits it
    /// (binary, too large).
    #[serde(default)]
    pub patch: Option<String>,
    pub binary: bool,
}

/// Provider-specific handles a line comment needs (GitLab wants diff refs, GitHub a
/// commit sha).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffRefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullComment {
    pub id: String,
    pub author: User,
    pub body: String,
    pub created_at: String,
}

/// Which side of a diff an inline thread sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}

/// One conversation: an opening comment and the replies under it, or a lone note on
/// a host that gives comments no thread structure at all.
///
/// The two capability flags are per thread, not per provider - GitHub can reply to a
/// review thread and cannot reply to an ordinary issue comment - and they are the
/// only thing the UI goes by. It never branches on which host it is talking to;
/// every difference is absorbed in the adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentThread {
    pub id: String,
    /// Oldest first. The first is the comment a reply would land under.
    pub comments: Vec<PullComment>,
    pub resolved: bool,
    /// The thread is anchored to a version of the diff that has since moved on.
    pub outdated: bool,
    /// Present for inline threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The line the thread is shown on - the last one, when it covers several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The first line the thread covers, on the same side, when it covers several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<Side>,
    pub can_reply: bool,
    pub can_resolve: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullDetail {
    pub item: ReviewItem,
    pub description: String,
    pub files: Vec<DiffFile>,
    pub threads: Vec<CommentThread>,
    pub refs: DiffRefs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSubmission {
    pub item_id: String,
    pub verdict: ReviewVerdict,
    pub body: String,
}

/// The kind of diff line a [`RangeEdge`] sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    Add,
    Del,
    Context,
}

/// One end of the range a multi-line comment covers, placed the way a unified diff
/// counts: where the line sits in the old file and in the new one at once. An added
/// line takes the old-file number it was inserted ahead of, a removed line the
/// new-file number that follows it - so both counters exist for every line.
///
/// Only GitLab addresses a range this way, through the line codes it keys multi-line
/// notes by. The counters fall straight out of the hunk headers and cost nothing to
/// carry, and carrying them is what lets a draft be submitted without the diff it
/// was written against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeEdge {
    pub kind: EdgeKind,
    pub old_pos: u32,
    pub new_pos: u32,
}

/// The lines a comment covers when it covers more than one.
///
/// A range stays on the side of the diff its comment is on and inside one hunk,
/// which is the shape every host accepts; the comment's own line is the last line of
/// it, which is where every host shows the comment. Forgejo alone anchors at the
/// first line, and its adapter converts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    /// The first line, on the same side as the comment's own line.
    pub start_line: u32,
    pub start: RangeEdge,
    pub end: RangeEdge,
}

/// A line comment written but not yet sent.
///
/// Drafts are owned by the app state and carry the diff references they were
/// written against, so they can be submitted against the code that was actually
/// read rather than against whatever the branch looks like by then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftComment {
    pub id: String,
    pub item_id: String,
    pub body: String,
    pub path: String,
    /// Line in the file after the change, when the comment is on an added or context line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
    /// Line in the file before the change, when the comment is on a removed line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    /// Present when the comment covers several lines, ending on the one above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
    pub created_at: String,
    pub refs: DiffRefs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineCommentDraft {
    pub item_id: String,
    pub body: String,
    pub path: String,
    /// Line number in the file *after* the change, when commenting on an added/context line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
    /// Line number in the file *before* the change, when commenting on a removed line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    /// Present when the comment covers several lines, ending on the one above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
}

/// A recurring stretch of the day the app is allowed to interrupt in.
///
/// The list of them is the whole feature: empty means the app notifies whenever a
/// poll finds something, exactly as it always has, and adding one is what buys the
/// quiet. The rules that read this live in `review_window`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewWindow {
    pub id: String,
    pub enabled: bool,
    /// The days it covers, numbered as `Date#getDay` does - 0 is Sunday.
    pub days: Vec<u32>,
    /// Local wall clock, `HH:MM`. The span is [start, end) and may not cross midnight.
    pub start: String,
    pub end: String,
    /// Stays quiet until at least this many reviews are waiting in its own scope.
    /// Signed, because the schedule editor holds whatever was typed until
    /// `window_problem` has had its say.
    pub minimum: i64,
    /// The accounts it covers. Empty means every account, now and in future - which
    /// is why an account connected next March is picked up by an existing
    /// all-accounts window rather than quietly going dark.
    ///
    /// A window written before one could name accounts has none stored, and covers
    /// all of them, which is exactly what an empty list means - so the migration is
    /// the default itself.
    #[serde(default, deserialize_with = "null_as_default")]
    pub accounts: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffViewMode {
    Split,
    Unified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

/// Plain Claude Code, unless settings or an account say otherwise.
pub const DEFAULT_AGENT_COMMAND: &str = "claude";

/// The user's preferences.
///
/// `Default` is `DEFAULT_SETTINGS`. Deserialising goes through [`merge_settings`],
/// so a vault written by an older build loads with every missing setting filled in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Seconds between background refreshes.
    pub poll_interval: u32,
    /// Seconds between check-status polls while any check is running.
    pub check_poll_interval: u32,
    pub notifications_enabled: bool,
    pub play_sound: bool,
    pub diff_view: DiffViewMode,
    pub theme: ThemeMode,
    /// Hide PRs the user has already approved.
    pub hide_approved: bool,
    /// Hide PRs that already carry every approval their host asks for.
    pub hide_fully_approved: bool,
    /// Hide PRs still being written, by the host's flag or by the title convention.
    pub hide_drafts: bool,
    /// Draw the waiting count beside the menu bar icon. Off leaves the icon alone.
    pub show_menu_bar_count: bool,
    pub launch_at_login: bool,
    /// When the app may interrupt. Empty means whenever it finds something.
    pub review_windows: Vec<ReviewWindow>,
    /// The command a copied agent handoff is built around. Any command or shell alias
    /// will do - it is the user's own shell that runs it, not the app.
    pub agent_command: String,
}

impl Default for Settings {
    /// `DEFAULT_SETTINGS`.
    fn default() -> Self {
        Settings {
            poll_interval: 180,
            check_poll_interval: 45,
            notifications_enabled: true,
            play_sound: true,
            diff_view: DiffViewMode::Split,
            theme: ThemeMode::System,
            hide_approved: false,
            hide_fully_approved: true,
            hide_drafts: true,
            show_menu_bar_count: true,
            launch_at_login: false,
            review_windows: Vec::new(),
            agent_command: DEFAULT_AGENT_COMMAND.to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = Value::deserialize(deserializer)?;
        Ok(merge_settings(Some(&stored)))
    }
}

/// Stored settings laid over the defaults, so a vault written by an older build
/// still loads: a setting added since is filled in from the defaults, and one
/// removed since is ignored harmlessly rather than failing the load.
///
/// A stored value of the wrong type falls back to its default too, and a review
/// window too malformed to read is dropped: one bad field must not cost the user
/// the whole vault.
pub fn merge_settings(stored: Option<&Value>) -> Settings {
    let mut settings = Settings::default();
    let Some(Value::Object(stored)) = stored else {
        return settings;
    };

    fn lay<T: DeserializeOwned>(stored: &serde_json::Map<String, Value>, key: &str, slot: &mut T) {
        if let Some(value) = stored.get(key)
            && let Ok(value) = T::deserialize(value)
        {
            *slot = value;
        }
    }

    lay(stored, "pollInterval", &mut settings.poll_interval);
    lay(
        stored,
        "checkPollInterval",
        &mut settings.check_poll_interval,
    );
    lay(
        stored,
        "notificationsEnabled",
        &mut settings.notifications_enabled,
    );
    lay(stored, "playSound", &mut settings.play_sound);
    lay(stored, "diffView", &mut settings.diff_view);
    lay(stored, "theme", &mut settings.theme);
    lay(stored, "hideApproved", &mut settings.hide_approved);
    lay(
        stored,
        "hideFullyApproved",
        &mut settings.hide_fully_approved,
    );
    lay(stored, "hideDrafts", &mut settings.hide_drafts);
    lay(
        stored,
        "showMenuBarCount",
        &mut settings.show_menu_bar_count,
    );
    lay(stored, "launchAtLogin", &mut settings.launch_at_login);
    lay(stored, "agentCommand", &mut settings.agent_command);
    if let Some(Value::Array(windows)) = stored.get("reviewWindows") {
        settings.review_windows = windows
            .iter()
            .filter_map(|window| ReviewWindow::deserialize(window).ok())
            .collect();
    }
    settings
}

/// `null` read as the type's default, the way `value ?? default` reads it.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Whether a review is one the user still wants in front of them.
///
/// Shared rather than kept with the UI's other filters, because the menu bar counts
/// the same deck the window lists, and the two disagreeing is something the user
/// sees. Only standing preferences belong here - the query box, the account picker
/// and the check picker stay in the UI, because those are a view somebody is
/// driving by hand rather than a rule the whole app should honour.
pub fn is_visible_review(item: &ReviewItem, settings: &Settings) -> bool {
    if settings.hide_approved && item.my_review_state == MyReviewState::Approved {
        return false;
    }
    if settings.hide_fully_approved && item.approvals.outcome == ApprovalOutcome::Satisfied {
        return false;
    }
    if settings.hide_drafts && is_draft_review(item) {
        return false;
    }
    true
}

/// What no host has said anything about: nothing given, nothing asked for.
pub fn no_approvals() -> ApprovalSummary {
    ApprovalSummary {
        given: 0,
        required: None,
        outcome: ApprovalOutcome::NoneRequired,
    }
}

/// The approval count once this user's own approval has gone out.
///
/// The card is patched straight away rather than waiting for the next sync, so the
/// count moves the moment the button does. Only the required count can settle the
/// outcome, and only when the host gave one; a host that says nothing keeps saying
/// nothing until it is asked again.
pub fn approvals_after_approving(
    approvals: &ApprovalSummary,
    already_approved: bool,
) -> ApprovalSummary {
    if already_approved {
        return approvals.clone();
    }
    let given = approvals.given + 1;
    let outcome = match approvals.outcome {
        ApprovalOutcome::NoneRequired => ApprovalOutcome::NoneRequired,
        outcome => match approvals.required {
            Some(required) if given >= required => ApprovalOutcome::Satisfied,
            _ => outcome,
        },
    };
    ApprovalSummary {
        given,
        required: approvals.required,
        outcome,
    }
}

/// A title that says draft before it says anything else. `\u{FEFF}` joins `\s`
/// because JavaScript counts it as white space and Unicode does not.
static DRAFT_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[\s\u{FEFF}]*(?:draft:|wip:|\[draft\]|\[wip\])").expect("valid pattern")
});

/// Whether a pull request is still being written rather than waiting on a review.
///
/// Two signals, because no single one covers every host. GitHub has a real draft
/// flag. GitLab derives its flag from a `Draft:` title prefix and Forgejo derives its
/// own from `WIP:`, so on those the flag already carries the convention. Bitbucket
/// Cloud has no draft concept at all, which leaves its flag always false and the
/// title the only thing there is to go on. The title rule also catches somebody
/// typing the habit on a host that would not have set the flag for them.
///
/// It has to be a prefix: a pull request about the draft feature is not a draft.
/// One false positive is accepted knowingly - a pull request genuinely called
/// "Draft: release notes for 2.0" is hidden with the rest - because the deck says
/// how many it is holding back, which is what makes that recoverable.
pub fn is_draft_review(item: &ReviewItem) -> bool {
    item.draft || DRAFT_TITLE.is_match(&item.title)
}

/// The reviews a deck actually shows, which is the number the menu bar carries.
pub fn visible_reviews<'a>(items: &'a [ReviewItem], settings: &Settings) -> Vec<&'a ReviewItem> {
    items
        .iter()
        .filter(|item| is_visible_review(item, settings))
        .collect()
}

/// The reviews a completed sync should raise a notification about: the ones that
/// arrived since the last sync and that the deck is going to show.
///
/// The third reading of the same rule, alongside the list and the menu bar count,
/// and deliberately not a fourth rule of its own. A banner for a review a standing
/// preference hides opens onto a deck that does not contain it - the app arguing
/// with itself in front of the user.
pub fn reviews_to_announce<'a>(
    items: &'a [ReviewItem],
    fresh: &[String],
    settings: &Settings,
) -> Vec<&'a ReviewItem> {
    let arrived: HashSet<&str> = fresh.iter().map(String::as_str).collect();
    visible_reviews(items, settings)
        .into_iter()
        .filter(|item| arrived.contains(item.id.as_str()))
        .collect()
}

/// The reviews a completed sync records as having been seen: every one it found,
/// including the ones no notification will mention.
///
/// Not the same set as what gets announced, and the difference is the point. A
/// review hidden by a standing preference is one the user has decided they never
/// want, so it is seen the moment it arrives. Recording only what was announced
/// would leave months of hidden reviews waiting to fire the day the preference is
/// turned off.
pub fn ids_to_record(items: &[ReviewItem]) -> Vec<String> {
    items.iter().map(|item| item.id.clone()).collect()
}

/// Per-account outcome of a refresh, so the UI can show which host is unhappy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub account_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeckState {
    pub items: Vec<ReviewItem>,
    pub statuses: Vec<AccountStatus>,
    pub syncing: bool,
    /// Whether a sync has completed for the accounts currently connected. Distinct
    /// from `last_synced_at`, which survives an account being added or removed: a
    /// deck that has never looked at the current set of accounts has nothing to say
    /// about whether they are quiet.
    pub synced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<String>,
}

/// What the deck pane says in place of review cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeckEmptyState {
    NoAccounts,
    Syncing,
    NoMatches,
    InboxZero,
    OnlyDrafts,
}

/// What [`deck_empty_state`] decides from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeckCounts {
    pub account_count: usize,
    pub synced: bool,
    pub visible_count: usize,
    /// Drafts the preference is holding back, already narrowed to the current view.
    pub hidden_draft_count: usize,
    pub filters_active: bool,
}

/// Why the deck pane is showing nothing, or `None` when it has cards to show.
///
/// Emptiness is not `items.len()` - before the first sync lands the deck is empty
/// because it has not looked yet, which is a very different thing to tell the user
/// than that nobody is waiting on them.
pub fn deck_empty_state(deck: DeckCounts) -> Option<DeckEmptyState> {
    // Nobody with no accounts is waiting on a sync, so this one never waits.
    if deck.account_count == 0 {
        return Some(DeckEmptyState::NoAccounts);
    }
    if deck.visible_count > 0 {
        return None;
    }
    if !deck.synced {
        return Some(DeckEmptyState::Syncing);
    }
    // Ahead of both of the below, because a deck holding nothing but drafts is
    // neither at inbox zero nor short of anything matching - it is holding drafts,
    // and saying so is the only version the user can act on.
    if deck.hidden_draft_count > 0 {
        return Some(DeckEmptyState::OnlyDrafts);
    }
    Some(if deck.filters_active {
        DeckEmptyState::NoMatches
    } else {
        DeckEmptyState::InboxZero
    })
}

// ---------------------------------------------------------------------------
// The pure helpers of src/main/providers/types.ts.
// ---------------------------------------------------------------------------

/// Collapse a set of individual check states into the one badge the deck shows.
pub fn roll_up(states: &[CheckStatus]) -> CheckStatus {
    if states.is_empty() {
        return CheckStatus::Unknown;
    }
    if states.contains(&CheckStatus::Failed) {
        return CheckStatus::Failed;
    }
    if states.contains(&CheckStatus::Running) {
        return CheckStatus::Running;
    }
    if states.iter().all(|state| *state == CheckStatus::Unknown) {
        return CheckStatus::Unknown;
    }
    CheckStatus::Passed
}

/// Count each bucket of `runs` and roll them up into the summary the deck shows,
/// keeping the runs themselves (`{ ...summariseChecks(runs), runs }` in every
/// adapter).
pub fn summarise_checks(runs: Vec<CheckRun>) -> CheckSummary {
    let count = |status: CheckStatus| runs.iter().filter(|run| run.status == status).count() as u32;
    let statuses: Vec<CheckStatus> = runs.iter().map(|run| run.status).collect();
    CheckSummary {
        status: roll_up(&statuses),
        passed: count(CheckStatus::Passed),
        failed: count(CheckStatus::Failed),
        running: count(CheckStatus::Running),
        total: runs.len() as u32,
        runs,
    }
}

/// Fold an approval count against what the host asks for.
///
/// For hosts that only publish a plain number: a rule that names who has to approve
/// (GitLab's rules, GitHub's code owners) is settled by the host itself, and those
/// adapters build the summary from its verdict rather than from here. `required`
/// unknown or zero both read as nothing required, which is also what the card shows
/// when the token was not allowed to ask.
pub fn summarise_approvals(given: u32, required: Option<u32>) -> ApprovalSummary {
    let outcome = match required {
        None | Some(0) => ApprovalOutcome::NoneRequired,
        Some(required) if given >= required => ApprovalOutcome::Satisfied,
        Some(_) => ApprovalOutcome::Pending,
    };
    ApprovalSummary {
        given,
        required,
        outcome,
    }
}

/// Who stands approving once every reviewer's latest verdict is taken. A review that
/// only comments does not move a verdict; a dismissed one withdraws it.
///
/// Each review is `(login, state)`; a review without a login is skipped.
pub fn count_approvers<'a>(reviews: impl IntoIterator<Item = (Option<&'a str>, &'a str)>) -> u32 {
    let mut standing: HashMap<&str, String> = HashMap::new();
    for (login, state) in reviews {
        let Some(login) = login.filter(|login| !login.is_empty()) else {
            continue;
        };
        let state = state.to_uppercase();
        if matches!(state.as_str(), "COMMENTED" | "COMMENT" | "PENDING") {
            continue;
        }
        standing.insert(login, state);
    }
    standing
        .values()
        .filter(|state| *state == "APPROVED")
        .count() as u32
}

/// Whether a branch restriction's pattern covers a branch. Bitbucket's patterns are
/// globs where `*` stands for any run of characters, and one per pattern is what the
/// app needs - `**` and character classes are read literally.
pub fn restriction_covers(pattern: &str, branch: &str) -> bool {
    // `*` becomes what JavaScript's `.*` matches: anything but a line terminator.
    let body = pattern
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(r"[^\n\r\u{2028}\u{2029}]*");
    Regex::new(&format!("^{body}$")).is_ok_and(|pattern| pattern.is_match(branch))
}

pub fn make_item_id(account_id: &str, repo_key: &str, number: u64) -> String {
    format!("{account_id}:{repo_key}:{number}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn review(id: &str, my_review_state: MyReviewState) -> ReviewItem {
        ReviewItem {
            id: id.into(),
            account_id: "acc".into(),
            provider: ProviderKind::Github,
            repo_key: "acme/design-tokens".into(),
            repo: "acme/design-tokens".into(),
            number: id.parse().unwrap_or(0),
            title: format!("Pull request {id}"),
            url: format!("https://example.test/pull/{id}"),
            author: User {
                name: "lpeters".into(),
                avatar_url: String::new(),
            },
            created_at: "2026-08-01T10:00:00Z".into(),
            updated_at: "2026-08-01T10:00:00Z".into(),
            draft: false,
            source_branch: "topic".into(),
            target_branch: "main".into(),
            labels: vec![],
            my_review_state,
            approvals: no_approvals(),
            checks: CheckSummary {
                status: CheckStatus::Unknown,
                passed: 0,
                failed: 0,
                running: 0,
                total: 0,
                runs: vec![],
            },
            additions: None,
            deletions: None,
            changed_files: None,
        }
    }

    fn ids(items: &[&ReviewItem]) -> Vec<String> {
        items.iter().map(|item| item.id.clone()).collect()
    }

    fn settings_with(patch: impl FnOnce(&mut Settings)) -> Settings {
        let mut settings = Settings::default();
        patch(&mut settings);
        settings
    }

    // --- test/announce.test.ts ---

    mod announce {
        use super::*;

        fn hiding_approved() -> Settings {
            settings_with(|s| s.hide_approved = true)
        }

        fn deck() -> Vec<ReviewItem> {
            vec![
                review("1", MyReviewState::Pending),
                review("2", MyReviewState::Approved),
            ]
        }

        #[test]
        fn a_new_review_the_settings_hide_raises_nothing_and_one_they_show_is_announced() {
            let deck = deck();
            let fresh = ids_to_record(&deck);
            assert_eq!(
                ids(&reviews_to_announce(&deck, &fresh, &hiding_approved())),
                ["1"]
            );
        }

        #[test]
        fn a_hidden_review_is_recorded_as_seen_all_the_same() {
            // Both halves of one sync: everything it found is seen, only what it will
            // show is announced. Recording just the announced set would leave the
            // approved one waiting to fire the day the preference is turned off.
            let deck = deck();
            assert_eq!(ids_to_record(&deck), ["1", "2"]);
            assert_eq!(
                ids(&reviews_to_announce(
                    &deck,
                    &ids_to_record(&deck),
                    &hiding_approved()
                )),
                ["1"]
            );
        }

        #[test]
        fn announcing_follows_the_rule_the_menu_bar_counts_by_not_one_of_its_own() {
            let deck = deck();
            let fresh = ids_to_record(&deck);
            for settings in [Settings::default(), hiding_approved()] {
                assert_eq!(
                    reviews_to_announce(&deck, &fresh, &settings),
                    visible_reviews(&deck, &settings),
                    "everything visible is new this sync, so the two sets have to match"
                );
            }
        }

        #[test]
        fn a_review_that_is_not_new_this_sync_is_not_announced_again() {
            let deck = deck();
            assert!(reviews_to_announce(&deck, &[], &Settings::default()).is_empty());
            assert_eq!(
                ids(&reviews_to_announce(
                    &deck,
                    &["2".to_string()],
                    &Settings::default()
                )),
                ["2"]
            );
            assert!(reviews_to_announce(&deck, &["2".to_string()], &hiding_approved()).is_empty());
        }

        #[test]
        fn announcing_keeps_the_deck_order_so_a_grouped_banner_lists_what_the_deck_lists() {
            let many: Vec<ReviewItem> = ["5", "4", "3"]
                .iter()
                .map(|id| review(id, MyReviewState::Pending))
                .collect();
            let mut fresh = ids_to_record(&many);
            fresh.reverse();
            assert_eq!(
                ids(&reviews_to_announce(&many, &fresh, &Settings::default())),
                ["5", "4", "3"]
            );
        }
    }

    // --- test/deck-empty-state.test.ts ---

    mod deck_empty {
        use super::*;

        const COLD: DeckCounts = DeckCounts {
            account_count: 1,
            synced: false,
            visible_count: 0,
            hidden_draft_count: 0,
            filters_active: false,
        };

        #[test]
        fn a_deck_with_no_accounts_says_so_straight_away_synced_or_not() {
            let none = DeckCounts {
                account_count: 0,
                ..COLD
            };
            assert_eq!(deck_empty_state(none), Some(DeckEmptyState::NoAccounts));
            assert_eq!(
                deck_empty_state(DeckCounts {
                    synced: true,
                    ..none
                }),
                Some(DeckEmptyState::NoAccounts)
            );
        }

        #[test]
        fn an_unsynced_deck_is_checking_never_inbox_zero() {
            assert_eq!(deck_empty_state(COLD), Some(DeckEmptyState::Syncing));
            assert_eq!(
                deck_empty_state(DeckCounts {
                    filters_active: true,
                    ..COLD
                }),
                Some(DeckEmptyState::Syncing)
            );
        }

        #[test]
        fn inbox_zero_waits_for_a_sync_that_genuinely_returned_nothing() {
            assert_eq!(
                deck_empty_state(DeckCounts {
                    synced: true,
                    ..COLD
                }),
                Some(DeckEmptyState::InboxZero)
            );
        }

        #[test]
        fn filters_take_precedence_over_inbox_zero_once_synced() {
            assert_eq!(
                deck_empty_state(DeckCounts {
                    synced: true,
                    filters_active: true,
                    ..COLD
                }),
                Some(DeckEmptyState::NoMatches)
            );
        }

        #[test]
        fn a_deck_holding_nothing_but_hidden_drafts_says_so_instead_of_inbox_zero() {
            let synced = DeckCounts {
                synced: true,
                hidden_draft_count: 2,
                ..COLD
            };
            assert_eq!(deck_empty_state(synced), Some(DeckEmptyState::OnlyDrafts));
            // Ahead of the filters too: with the drafts already narrowed to the
            // current view, "nothing matches" would be as wrong as "inbox zero".
            assert_eq!(
                deck_empty_state(DeckCounts {
                    filters_active: true,
                    ..synced
                }),
                Some(DeckEmptyState::OnlyDrafts)
            );
            // Still waiting on the first sync, though, so it does not know that yet.
            assert_eq!(
                deck_empty_state(DeckCounts {
                    synced: false,
                    ..synced
                }),
                Some(DeckEmptyState::Syncing)
            );
        }

        #[test]
        fn drafts_on_screen_are_not_drafts_held_back_so_the_deck_reads_normally() {
            // What revealing them looks like: the count stays, but there is a list again.
            assert_eq!(
                deck_empty_state(DeckCounts {
                    synced: true,
                    hidden_draft_count: 2,
                    visible_count: 2,
                    ..COLD
                }),
                None
            );
        }

        #[test]
        fn reviews_to_show_beat_every_empty_state_except_a_missing_account() {
            assert_eq!(
                deck_empty_state(DeckCounts {
                    visible_count: 3,
                    ..COLD
                }),
                None
            );
            assert_eq!(
                deck_empty_state(DeckCounts {
                    synced: true,
                    visible_count: 3,
                    ..COLD
                }),
                None
            );
            assert_eq!(
                deck_empty_state(DeckCounts {
                    synced: true,
                    visible_count: 3,
                    filters_active: true,
                    ..COLD
                }),
                None
            );
        }

        #[test]
        fn empty_states_serialise_to_the_typescript_literals() {
            assert_eq!(
                serde_json::to_value([
                    DeckEmptyState::NoAccounts,
                    DeckEmptyState::Syncing,
                    DeckEmptyState::NoMatches,
                    DeckEmptyState::InboxZero,
                    DeckEmptyState::OnlyDrafts,
                ])
                .unwrap(),
                json!([
                    "no-accounts",
                    "syncing",
                    "no-matches",
                    "inbox-zero",
                    "only-drafts"
                ])
            );
        }
    }

    // --- test/settings.test.ts ---

    mod settings {
        use super::*;

        fn hiding(hide_approved: bool) -> Settings {
            settings_with(|s| s.hide_approved = hide_approved)
        }

        #[test]
        fn merge_settings_fills_in_everything_the_stored_vault_does_not_carry() {
            assert_eq!(merge_settings(Some(&json!({}))), Settings::default());
            assert_eq!(merge_settings(None), Settings::default());
            assert_eq!(merge_settings(Some(&Value::Null)), Settings::default());

            let merged = merge_settings(Some(&json!({ "diffView": "unified" })));
            assert_eq!(merged.diff_view, DiffViewMode::Unified);
            assert_eq!(merged.poll_interval, Settings::default().poll_interval);
        }

        #[test]
        fn merge_settings_keeps_a_stored_value_even_when_it_matches_no_default() {
            let merged = merge_settings(Some(&json!({
                "notificationsEnabled": false,
                "playSound": false,
            })));
            assert!(!merged.notifications_enabled);
            assert!(!merged.play_sound);
        }

        #[test]
        fn merge_settings_loads_a_vault_written_before_a_setting_was_removed() {
            // `showWhitespace` was defined and defaulted but read nowhere, so it was
            // removed. A vault written by a build that still had it must keep loading.
            let older = json!({ "diffView": "unified", "showWhitespace": true, "theme": "dark" });

            let merged = merge_settings(Some(&older));

            assert_eq!(merged.diff_view, DiffViewMode::Unified);
            assert_eq!(merged.theme, ThemeMode::Dark);
            assert_eq!(merged.hide_approved, Settings::default().hide_approved);
        }

        #[test]
        fn a_review_window_stored_before_it_could_name_accounts_covers_all_of_them() {
            // Written by the build that shipped windows without scoping. Coming back
            // with no `accounts` at all would fail the moment anything asked what it
            // covers.
            let older = json!({
                "reviewWindows": [
                    { "id": "morning", "enabled": true, "days": [1, 2, 3, 4, 5], "start": "09:00", "end": "09:30", "minimum": 1 },
                ],
            });

            let merged = merge_settings(Some(&older));

            assert!(merged.review_windows[0].accounts.is_empty());
            assert_eq!(merged.review_windows[0].start, "09:00");
        }

        #[test]
        fn the_settings_type_no_longer_carries_the_whitespace_display_key() {
            let defaults = serde_json::to_value(Settings::default()).unwrap();
            assert!(defaults.get("showWhitespace").is_none());
        }

        #[test]
        fn the_menu_bar_shows_its_count_unless_somebody_turns_it_off() {
            assert!(Settings::default().show_menu_bar_count);
            assert!(
                !merge_settings(Some(&json!({ "showMenuBarCount": false }))).show_menu_bar_count
            );
        }

        #[test]
        fn a_vault_written_before_the_quiet_menu_bar_existed_keeps_its_count() {
            let merged = merge_settings(Some(&json!({ "theme": "dark", "hideApproved": true })));

            assert!(merged.show_menu_bar_count);
            assert!(merged.hide_approved);
        }

        #[test]
        fn the_approved_filter_is_off_by_default_so_nothing_is_hidden() {
            let item = review("1", MyReviewState::Approved);
            assert!(is_visible_review(&item, &Settings::default()));
        }

        #[test]
        fn a_review_the_user_approved_is_hidden_only_while_the_setting_is_on() {
            let item = review("1", MyReviewState::Approved);

            assert!(!is_visible_review(&item, &hiding(true)));
            assert!(is_visible_review(&item, &hiding(false)));
        }

        #[test]
        fn every_other_review_state_survives_the_approved_filter() {
            for state in [
                MyReviewState::Pending,
                MyReviewState::ChangesRequested,
                MyReviewState::Commented,
            ] {
                assert!(
                    is_visible_review(&review("1", state), &hiding(true)),
                    "{state:?}"
                );
            }
        }

        #[test]
        fn the_count_the_menu_bar_carries_is_the_count_the_window_lists() {
            let items = vec![
                review("1", MyReviewState::Pending),
                review("2", MyReviewState::Approved),
                review("3", MyReviewState::Commented),
                review("4", MyReviewState::Approved),
            ];

            assert_eq!(visible_reviews(&items, &hiding(false)).len(), 4);
            assert_eq!(ids(&visible_reviews(&items, &hiding(true))), ["1", "3"]);
        }

        #[test]
        fn a_deck_of_nothing_but_approved_reviews_counts_zero_so_the_menu_bar_shows_no_number() {
            let items = vec![
                review("1", MyReviewState::Approved),
                review("2", MyReviewState::Approved),
            ];

            assert_eq!(visible_reviews(&items, &hiding(true)).len(), 0);
            assert_eq!(visible_reviews(&[], &hiding(false)).len(), 0);
        }

        #[test]
        fn defaults_serialise_to_the_typescript_shape() {
            assert_eq!(
                serde_json::to_value(Settings::default()).unwrap(),
                json!({
                    "pollInterval": 180,
                    "checkPollInterval": 45,
                    "notificationsEnabled": true,
                    "playSound": true,
                    "diffView": "split",
                    "theme": "system",
                    "hideApproved": false,
                    "hideFullyApproved": true,
                    "hideDrafts": true,
                    "showMenuBarCount": true,
                    "launchAtLogin": false,
                    "reviewWindows": [],
                    "agentCommand": "claude",
                })
            );
        }

        #[test]
        fn settings_deserialise_through_merge_settings() {
            let parsed: Settings = serde_json::from_value(json!({
                "pollInterval": 300,
                "theme": "light",
                "playSound": "not a boolean",
                "reviewWindows": [
                    { "id": "a", "enabled": true, "days": [1], "start": "09:00", "end": "10:00", "minimum": 2, "accounts": null },
                    { "id": "broken" },
                ],
            }))
            .unwrap();
            assert_eq!(parsed.poll_interval, 300);
            assert_eq!(parsed.theme, ThemeMode::Light);
            // A value of the wrong type falls back to its default.
            assert!(parsed.play_sound);
            // A window too malformed to read is dropped; `accounts: null` is all of them.
            assert_eq!(parsed.review_windows.len(), 1);
            assert!(parsed.review_windows[0].accounts.is_empty());
            assert_eq!(parsed.agent_command, DEFAULT_AGENT_COMMAND);
        }
    }

    // --- test/drafts-visibility.test.ts ---

    mod drafts_visibility {
        use super::*;

        fn titled(title: &str) -> ReviewItem {
            ReviewItem {
                title: title.into(),
                ..review("1", MyReviewState::Pending)
            }
        }

        fn drafted() -> ReviewItem {
            ReviewItem {
                draft: true,
                ..review("1", MyReviewState::Pending)
            }
        }

        #[test]
        fn the_host_flag_alone_marks_a_draft_whatever_the_title_says() {
            assert!(is_draft_review(&drafted()));
            assert!(is_draft_review(&ReviewItem {
                title: "Ready to go".into(),
                ..drafted()
            }));
        }

        #[test]
        fn a_title_that_opens_with_the_convention_marks_a_draft_on_a_host_with_no_flag() {
            // Bitbucket Cloud has no draft concept at all, so this is the only signal there.
            for title in [
                "draft: cache the tokens",
                "Draft: cache the tokens",
                "DRAFT: cache the tokens",
                "wip: cache the tokens",
                "WIP: cache the tokens",
                "[draft] cache the tokens",
                "[DRAFT] cache the tokens",
                "[wip] cache the tokens",
                "[WIP] cache the tokens",
                "   WIP: leading whitespace is still a prefix",
                "\u{FEFF}WIP: a byte order mark is white space to JavaScript",
            ] {
                assert!(is_draft_review(&titled(title)), "{title}");
            }
        }

        #[test]
        fn a_title_that_merely_mentions_the_words_is_not_a_draft() {
            for title in [
                "Fix the wip: handler",
                "Rewrite the draft: parser",
                "Drafts of the release notes",
                "Add draft mode to the editor",
                "Explain [WIP] in the contributing guide",
                "draft",
                "wip",
            ] {
                assert!(!is_draft_review(&titled(title)), "{title}");
            }
        }

        #[test]
        fn drafts_are_hidden_by_default_and_only_while_the_setting_is_on() {
            assert!(Settings::default().hide_drafts);

            assert!(!is_visible_review(&drafted(), &Settings::default()));
            assert!(is_visible_review(
                &drafted(),
                &settings_with(|s| s.hide_drafts = false)
            ));
        }

        #[test]
        fn a_vault_written_before_the_drafts_preference_existed_picks_up_the_default() {
            let merged = merge_settings(Some(&json!({ "theme": "dark", "hideApproved": true })));

            assert!(merged.hide_drafts);
            assert!(merged.hide_approved);
            assert_eq!(merged.theme, ThemeMode::Dark);
        }

        #[test]
        fn the_two_standing_preferences_hide_independently_and_together() {
            let items = vec![
                review("1", MyReviewState::Pending),
                review("2", MyReviewState::Approved),
                ReviewItem {
                    id: "3".into(),
                    ..drafted()
                },
                ReviewItem {
                    id: "4".into(),
                    title: "WIP: still writing it".into(),
                    ..review("4", MyReviewState::Approved)
                },
            ];
            let shown = |hide_approved: bool, hide_drafts: bool| {
                ids(&visible_reviews(
                    &items,
                    &settings_with(|s| {
                        s.hide_approved = hide_approved;
                        s.hide_drafts = hide_drafts;
                    }),
                ))
            };

            assert_eq!(shown(false, false), ["1", "2", "3", "4"]);
            assert_eq!(shown(false, true), ["1", "2"]);
            assert_eq!(shown(true, false), ["1", "3"]);
            assert_eq!(shown(true, true), ["1"]);
        }
    }

    // --- test/approvals.test.ts ---

    mod approvals {
        use super::*;

        fn with(id: &str, approvals: ApprovalSummary) -> ReviewItem {
            ReviewItem {
                provider: ProviderKind::Gitlab,
                repo_key: "2841".into(),
                repo: "acme/platform/edge-router".into(),
                title: format!("Merge request {id}"),
                url: format!("https://example.test/merge_requests/{id}"),
                author: User {
                    name: "dnovak".into(),
                    avatar_url: String::new(),
                },
                approvals,
                ..review(id, MyReviewState::Pending)
            }
        }

        fn summary(given: u32, required: Option<u32>, outcome: ApprovalOutcome) -> ApprovalSummary {
            ApprovalSummary {
                given,
                required,
                outcome,
            }
        }

        fn none_required() -> ReviewItem {
            with("1", summary(1, None, ApprovalOutcome::NoneRequired))
        }

        fn short() -> ReviewItem {
            with("2", summary(1, Some(2), ApprovalOutcome::Pending))
        }

        fn satisfied() -> ReviewItem {
            with("3", summary(2, Some(2), ApprovalOutcome::Satisfied))
        }

        #[test]
        fn a_fully_approved_review_is_hidden_by_default() {
            assert!(Settings::default().hide_fully_approved);
            assert!(!is_visible_review(&satisfied(), &Settings::default()));
        }

        #[test]
        fn a_review_still_short_of_approvals_or_on_a_host_asking_for_none_stays() {
            assert!(is_visible_review(&short(), &Settings::default()));
            assert!(is_visible_review(&none_required(), &Settings::default()));
        }

        #[test]
        fn turning_the_preference_off_shows_the_fully_approved_ones_again() {
            let showing = settings_with(|s| s.hide_fully_approved = false);
            let items = [none_required(), short(), satisfied()];
            assert_eq!(ids(&visible_reviews(&items, &showing)), ["1", "2", "3"]);
        }

        #[test]
        fn the_preference_is_on_for_a_vault_written_before_it_existed() {
            let merged = merge_settings(Some(&json!({ "theme": "dark", "hideApproved": true })));
            assert!(merged.hide_fully_approved);
            assert!(merged.hide_approved);
        }

        #[test]
        fn approving_moves_the_count_at_once_and_settles_the_outcome_only_with_a_figure() {
            use ApprovalOutcome::*;
            assert_eq!(
                approvals_after_approving(&summary(1, Some(2), Pending), false),
                summary(2, Some(2), Satisfied)
            );
            assert_eq!(
                approvals_after_approving(&summary(1, Some(3), Pending), false),
                summary(2, Some(3), Pending)
            );
            // A host that gave a verdict but no figure keeps its verdict until asked again.
            assert_eq!(
                approvals_after_approving(&summary(0, None, Pending), false),
                summary(1, None, Pending)
            );
            // Nothing required stays nothing required, however many approve.
            assert_eq!(
                approvals_after_approving(&summary(0, None, NoneRequired), false),
                summary(1, None, NoneRequired)
            );
            // Approving again is not a second approval.
            let already = summary(1, Some(2), Pending);
            assert_eq!(approvals_after_approving(&already, true), already);
        }

        #[test]
        fn summarise_approvals_reads_a_missing_or_zero_requirement_as_none() {
            use ApprovalOutcome::*;
            assert_eq!(summarise_approvals(2, None), summary(2, None, NoneRequired));
            assert_eq!(
                summarise_approvals(2, Some(0)),
                summary(2, Some(0), NoneRequired)
            );
            assert_eq!(
                summarise_approvals(1, Some(2)),
                summary(1, Some(2), Pending)
            );
            assert_eq!(
                summarise_approvals(3, Some(2)),
                summary(3, Some(2), Satisfied)
            );
        }

        #[test]
        fn count_approvers_takes_each_reviewer_at_their_latest_verdict() {
            assert_eq!(
                count_approvers([
                    (Some("ana"), "APPROVED"),
                    (Some("ben"), "CHANGES_REQUESTED"),
                    (Some("ben"), "COMMENTED"),
                    (Some("ben"), "APPROVED"),
                    (Some("cyd"), "APPROVED"),
                    (Some("cyd"), "DISMISSED"),
                    (Some("dee"), "COMMENTED"),
                    (None, "APPROVED"),
                ]),
                2
            );
        }

        #[test]
        fn a_bitbucket_restriction_pattern_is_a_glob_with_one_wildcard() {
            assert!(restriction_covers("main", "main"));
            assert!(restriction_covers("release/*", "release/2.4"));
            assert!(!restriction_covers("release/*", "main"));
            assert!(restriction_covers("*", "anything/at/all"));
            assert!(!restriction_covers("v1.0", "v1x0"));
        }
    }

    // --- test/providers.test.ts (rollUp, summariseChecks) ---

    mod checks {
        use super::*;
        use CheckStatus::*;

        #[test]
        fn roll_up_lets_the_worst_status_win() {
            assert_eq!(roll_up(&[]), Unknown);
            assert_eq!(roll_up(&[Passed, Passed]), Passed);
            assert_eq!(roll_up(&[Passed, Running]), Running);
            // A failure outranks anything still in flight.
            assert_eq!(roll_up(&[Running, Failed]), Failed);
            assert_eq!(roll_up(&[Unknown, Unknown]), Unknown);
            // A single real pass alongside unknowns still counts as passing.
            assert_eq!(roll_up(&[Unknown, Passed]), Passed);
        }

        #[test]
        fn summarise_checks_counts_each_bucket() {
            let runs: Vec<CheckRun> = [Passed, Passed, Failed, Running, Unknown]
                .into_iter()
                .enumerate()
                .map(|(i, status)| CheckRun {
                    id: i.to_string(),
                    name: format!("check {i}"),
                    status,
                    url: None,
                    description: None,
                })
                .collect();
            let summary = summarise_checks(runs.clone());
            assert_eq!(
                (
                    summary.status,
                    summary.passed,
                    summary.failed,
                    summary.running,
                    summary.total
                ),
                (Failed, 2, 1, 1, 5)
            );
            assert_eq!(summary.runs, runs);
        }
    }

    // --- serde shapes the Electron vault holds ---

    mod shapes {
        use super::*;

        #[test]
        fn a_review_item_round_trips_the_electron_json() {
            let stored = json!({
                "id": "acc:acme/app:7",
                "accountId": "acc",
                "provider": "github",
                "repoKey": "acme/app",
                "repo": "acme/app",
                "number": 7,
                "title": "Cache the tokens",
                "url": "https://github.com/acme/app/pull/7",
                "author": { "name": "lpeters", "avatarUrl": "https://example.test/a.png" },
                "createdAt": "2026-08-01T10:00:00Z",
                "updatedAt": "2026-08-02T10:00:00Z",
                "draft": false,
                "sourceBranch": "topic",
                "targetBranch": "main",
                "labels": ["infra"],
                "myReviewState": "changes_requested",
                "approvals": { "given": 1, "required": 2, "outcome": "pending" },
                "checks": {
                    "status": "running",
                    "passed": 1,
                    "failed": 0,
                    "running": 1,
                    "total": 2,
                    "runs": [
                        { "id": "1", "name": "build", "status": "passed", "url": "https://ci.test/1" },
                        { "id": "2", "name": "test", "status": "running" },
                    ],
                },
                "additions": 10,
                "deletions": 2,
            });
            let item: ReviewItem = serde_json::from_value(stored.clone()).unwrap();
            assert_eq!(item.my_review_state, MyReviewState::ChangesRequested);
            assert_eq!(item.approvals.required, Some(2));
            assert_eq!(item.changed_files, None);
            assert_eq!(serde_json::to_value(&item).unwrap(), stored);
        }

        #[test]
        fn an_account_round_trips_with_and_without_its_agent_command() {
            let stored = json!({
                "id": "0f1d",
                "kind": "forgejo",
                "label": "Codeberg",
                "baseUrl": "https://codeberg.org/api/v1",
                "webUrl": "https://codeberg.org",
                "username": "vojta",
                "displayName": "Vojta",
                "avatarUrl": "",
                "addedAt": "2026-08-01T10:00:00.000Z",
            });
            let account: Account = serde_json::from_value(stored.clone()).unwrap();
            assert_eq!(account.kind, ProviderKind::Forgejo);
            assert_eq!(serde_json::to_value(&account).unwrap(), stored);

            let mut with_command = stored;
            with_command["agentCommand"] = json!("claude-work");
            let account: Account = serde_json::from_value(with_command.clone()).unwrap();
            assert_eq!(account.agent_command.as_deref(), Some("claude-work"));
            assert_eq!(serde_json::to_value(&account).unwrap(), with_command);
        }

        #[test]
        fn a_draft_comment_round_trips_with_a_range() {
            let stored = json!({
                "id": "d1",
                "itemId": "acc:2841:5",
                "body": "Why?",
                "path": "src/main.rs",
                "newLine": 12,
                "range": {
                    "startLine": 10,
                    "start": { "kind": "context", "oldPos": 9, "newPos": 10 },
                    "end": { "kind": "add", "oldPos": 11, "newPos": 12 },
                },
                "createdAt": "2026-08-01T10:00:00.000Z",
                "refs": { "baseSha": "a", "startSha": "b", "headSha": "c" },
            });
            let draft: DraftComment = serde_json::from_value(stored.clone()).unwrap();
            assert_eq!(draft.range.map(|range| range.end.kind), Some(EdgeKind::Add));
            assert_eq!(serde_json::to_value(&draft).unwrap(), stored);
        }

        #[test]
        fn a_diff_file_keeps_a_null_patch() {
            let stored = json!({
                "path": "logo.png",
                "oldPath": "logo.png",
                "status": "added",
                "additions": 0,
                "deletions": 0,
                "patch": null,
                "binary": true,
            });
            let file: DiffFile = serde_json::from_value(stored.clone()).unwrap();
            assert_eq!(file.patch, None);
            assert_eq!(serde_json::to_value(&file).unwrap(), stored);
        }

        #[test]
        fn enums_serialise_to_the_typescript_literals() {
            assert_eq!(
                serde_json::to_value((
                    ApprovalOutcome::NoneRequired,
                    ReviewVerdict::RequestChanges,
                    Side::Old,
                    FileStatus::Renamed,
                    ProviderKind::Bitbucket,
                ))
                .unwrap(),
                json!([
                    "none_required",
                    "request_changes",
                    "old",
                    "renamed",
                    "bitbucket"
                ])
            );
            for kind in ProviderKind::ALL {
                assert_eq!(serde_json::to_value(kind).unwrap(), json!(kind.as_str()));
            }
            assert_eq!(ProviderKind::Forgejo.label(), "Forgejo / Gitea");
        }

        #[test]
        fn make_item_id_joins_account_repo_and_number() {
            assert_eq!(make_item_id("acc", "acme/app", 7), "acc:acme/app:7");
        }
    }
}
