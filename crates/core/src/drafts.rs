//! Line comments written but not yet sent. A port of src/main/drafts.ts and
//! src/shared/drafts.ts.
//!
//! The app state owns them, not a view: they outlive the window, they have to be
//! there when the app is reopened, and they are what a review submission is built
//! from.
//!
//! They are held in memory and written on a debounce, because the vault they land in
//! rewrites its whole file on every change - fine once a review is submitted,
//! ruinous once per keystroke. The store itself keeps no timer: every change marks
//! it dirty, and the app debounces ([`SAVE_DEBOUNCE_MS`] after the last change) by
//! taking the flag with [`DraftStore::take_dirty`] and writing
//! [`DraftStore::snapshot`], and flushes the same way on quit.
//!
//! Nothing here touches the disk, and the clock and the identifiers are injectable,
//! so the keying, the recorded references and what happens when a submission fails
//! can be tested directly.

use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::model::{DiffRefs, DraftComment, LineRange, MyReviewState};
use crate::time::now_iso;

/// How long after the last change the app writes the drafts down.
pub const SAVE_DEBOUNCE_MS: u64 = 800;

/// What is known about one item's drafts beyond the text itself.
///
/// Local drafts are invisible to the host by design - that is what lets one
/// mechanism serve four of them - and the price is that a review submitted in a
/// browser leaves a full draft set here with nothing saying so. The baseline is what
/// makes that noticeable: the reviewer's own review state when the set began, to
/// compare against what the next sync reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSet {
    pub baseline: MyReviewState,
    /// The state changed without this app submitting, and the reviewer has not said
    /// what to do.
    pub diverged: bool,
}

/// Everything the store holds, as the vault keeps it: the comments under `drafts`
/// and the sets under `draftSets`, so the vault can flatten this straight into its
/// own shape (`#[serde(flatten)]`).
///
/// Reading is forgiving, because these sit in the same file as the accounts: a
/// missing or `null` field is empty, and a draft or a set too malformed to read is
/// dropped on its own rather than failing the whole vault.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftState {
    #[serde(rename = "drafts", default, deserialize_with = "lenient_list")]
    pub comments: Vec<DraftComment>,
    #[serde(rename = "draftSets", default, deserialize_with = "lenient_map")]
    pub sets: BTreeMap<String, DraftSet>,
}

/// A list where each entry that does not read is left out; anything but a list is
/// an empty one.
fn lenient_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Array(entries) => entries
            .into_iter()
            .filter_map(|entry| T::deserialize(entry).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// A map where each entry that does not read is left out; anything but an object is
/// an empty one.
fn lenient_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Object(entries) => entries
            .into_iter()
            .filter_map(|(key, entry)| T::deserialize(entry).ok().map(|value| (key, value)))
            .collect(),
        _ => BTreeMap::new(),
    })
}

/// What [`has_diverged`] decides from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DivergenceCheck {
    pub baseline: Option<MyReviewState>,
    pub current: MyReviewState,
    /// Submissions this app had made when the sync began.
    pub submissions_at_sync_start: u64,
    /// Submissions this app has made now.
    pub submissions_now: u64,
}

/// Whether an item's drafts have been left behind by a review submitted elsewhere.
///
/// Pure, and deliberately conservative: it reports divergence only when it can
/// account for the change. A set with no baseline has nothing to compare, and a
/// sync that was already in flight when this app submitted is holding a state from
/// before that submission - reading it would report the app's own review as
/// somebody else's.
pub fn has_diverged(check: DivergenceCheck) -> bool {
    let Some(baseline) = check.baseline else {
        return false;
    };
    if check.submissions_now != check.submissions_at_sync_start {
        return false;
    }
    check.current != baseline
}

/// A draft as the diff view hands it over, before the store gives it an id and a
/// date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewDraft {
    pub item_id: String,
    pub body: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
    pub refs: DiffRefs,
}

type Clock = Box<dyn Fn() -> String + Send>;
type IdSource = Box<dyn FnMut() -> String + Send>;

/// `draft-` and ten base-36 characters, the shape of the TypeScript's
/// `Math.random().toString(36).slice(2, 12)`. Randomly keyed hashing of a counter
/// and the clock is plenty for ids that only have to be unique on this machine.
fn random_draft_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    hasher.write_i64(crate::time::now_ms());
    let mut bits = hasher.finish();
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut id = String::from("draft-");
    for _ in 0..10 {
        id.push(char::from(DIGITS[(bits % 36) as usize]));
        bits /= 36;
    }
    id
}

/// The drafts of every pull request, in memory.
pub struct DraftStore {
    drafts: Vec<DraftComment>,
    sets: BTreeMap<String, DraftSet>,
    /// How many reviews this app has submitted, ever. Only differences matter: it is
    /// how a sync tells whether its own data predates a submission made since.
    submitted: u64,
    /// Changed since the app last took a snapshot to write.
    dirty: bool,
    now: Clock,
    id: IdSource,
}

impl std::fmt::Debug for DraftStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftStore")
            .field("drafts", &self.drafts)
            .field("sets", &self.sets)
            .field("submitted", &self.submitted)
            .field("dirty", &self.dirty)
            .finish_non_exhaustive()
    }
}

impl DraftStore {
    /// A store holding what the vault had, with the real clock and random ids.
    /// Loading is not a change, so it starts clean.
    pub fn new(loaded: DraftState) -> Self {
        DraftStore {
            drafts: loaded.comments,
            sets: loaded.sets,
            submitted: 0,
            dirty: false,
            now: Box::new(now_iso),
            id: Box::new(random_draft_id),
        }
    }

    /// Replaces the clock that dates new drafts (ISO-8601, as `toISOString` writes).
    pub fn with_clock(mut self, now: impl Fn() -> String + Send + 'static) -> Self {
        self.now = Box::new(now);
        self
    }

    /// Replaces the source of new drafts' ids.
    pub fn with_ids(mut self, id: impl FnMut() -> String + Send + 'static) -> Self {
        self.id = Box::new(id);
        self
    }

    /// Oldest first, so a review reads in the order it was written.
    pub fn list(&self, item_id: &str) -> Vec<DraftComment> {
        let mut drafts: Vec<DraftComment> = self
            .drafts
            .iter()
            .filter(|draft| draft.item_id == item_id)
            .cloned()
            .collect();
        // Stable, so drafts written in the same millisecond keep their order.
        drafts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        drafts
    }

    pub fn count(&self, item_id: &str) -> usize {
        self.drafts
            .iter()
            .filter(|draft| draft.item_id == item_id)
            .count()
    }

    /// Adds a draft and returns it as stored.
    ///
    /// `state` is the reviewer's own review state right now, which becomes the
    /// baseline if this is the first draft for the item. Recording it here rather
    /// than tracking every item in the deck is what makes a set created after an
    /// external change start from the current state instead of reporting divergence
    /// the moment it exists.
    pub fn add(&mut self, draft: NewDraft, state: Option<MyReviewState>) -> DraftComment {
        let id = (self.id)();
        let created_at = (self.now)();
        let created = DraftComment {
            id,
            item_id: draft.item_id,
            body: draft.body,
            path: draft.path,
            new_line: draft.new_line,
            old_line: draft.old_line,
            range: draft.range,
            created_at,
            refs: draft.refs,
        };
        self.drafts.push(created.clone());
        if let Some(state) = state
            && !self.sets.contains_key(&created.item_id)
        {
            self.sets.insert(
                created.item_id.clone(),
                DraftSet {
                    baseline: state,
                    diverged: false,
                },
            );
        }
        self.dirty = true;
        created
    }

    /// How many reviews this app has submitted, for the sync race guard.
    pub fn submissions(&self) -> u64 {
        self.submitted
    }

    /// The app has submitted for this item: the set starts again from where that left
    /// it, and any divergence it was showing is answered.
    pub fn record_submission(&mut self, item_id: &str, state: MyReviewState) {
        self.submitted += 1;
        if self.count(item_id) == 0 {
            return;
        }
        self.sets.insert(
            item_id.to_string(),
            DraftSet {
                baseline: state,
                diverged: false,
            },
        );
        self.dirty = true;
    }

    /// Compares a freshly synced state against the baseline and marks the set if it
    /// moved. Answers whether the set is diverged now.
    pub fn reconcile(
        &mut self,
        item_id: &str,
        current: MyReviewState,
        submissions_at_sync_start: u64,
    ) -> bool {
        let Some(set) = self.sets.get(item_id).copied() else {
            return false;
        };
        if self.count(item_id) == 0 {
            return false;
        }

        let diverged = has_diverged(DivergenceCheck {
            baseline: Some(set.baseline),
            current,
            submissions_at_sync_start,
            submissions_now: self.submitted,
        });
        if !diverged || set.diverged {
            return set.diverged;
        }

        self.sets.insert(
            item_id.to_string(),
            DraftSet {
                diverged: true,
                ..set
            },
        );
        self.dirty = true;
        true
    }

    pub fn diverged(&self, item_id: &str) -> bool {
        self.sets.get(item_id).is_some_and(|set| set.diverged) && self.count(item_id) > 0
    }

    /// The reviewer has said to keep the drafts. The mark goes, the text is not
    /// touched, and the baseline moves to what is there now so the same change is not
    /// reported twice.
    pub fn acknowledge(&mut self, item_id: &str, state: MyReviewState) {
        let Some(set) = self.sets.get_mut(item_id) else {
            return;
        };
        *set = DraftSet {
            baseline: state,
            diverged: false,
        };
        self.dirty = true;
    }

    /// Changes a draft's body and nothing else about where it belongs.
    pub fn update(&mut self, id: &str, body: &str) -> Option<DraftComment> {
        let found = self.drafts.iter_mut().find(|draft| draft.id == id)?;
        found.body = body.to_string();
        let updated = found.clone();
        self.dirty = true;
        Some(updated)
    }

    pub fn remove(&mut self, id: &str) -> Option<DraftComment> {
        let at = self.drafts.iter().position(|draft| draft.id == id)?;
        let removed = self.drafts.remove(at);
        self.dirty = true;
        Some(removed)
    }

    /// Drops an item's drafts. Only ever called once a submission has come back
    /// clean - a submission that fails must leave the set exactly as it was, which is
    /// why nothing is taken away in advance and put back afterwards.
    pub fn clear(&mut self, item_id: &str) {
        if self.count(item_id) == 0 {
            return;
        }
        self.drafts.retain(|draft| draft.item_id != item_id);
        self.sets.remove(item_id);
        self.dirty = true;
    }

    /// Whether anything changed since the app last took the flag.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Takes the flag: true when something changed since the last call, after which
    /// the store reads as clean until the next change. The app's debounce calls this
    /// when it fires and writes [`DraftStore::snapshot`] only if it answers true.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Everything the store holds, as it goes to the vault.
    pub fn snapshot(&self) -> DraftState {
        DraftState {
            comments: self.drafts.clone(),
            sets: self.sets.clone(),
        }
    }

    /// What to write now, whatever the debounce was waiting for: the snapshot, with
    /// the flag taken. Like the TypeScript `flush`, it hands back the state even when
    /// nothing changed.
    pub fn flush(&mut self) -> DraftState {
        self.dirty = false;
        self.snapshot()
    }
}

/// The heads these drafts were written against, in the order first seen.
///
/// On an active pull request the author pushing mid-review is ordinary rather than
/// exceptional, and each draft already records the references it was written
/// against, so this is a comparison rather than a guess.
pub fn drafted_heads(drafts: &[DraftComment]) -> Vec<String> {
    let mut heads: Vec<String> = Vec::new();
    for draft in drafts {
        if let Some(head) = draft.refs.head_sha.as_deref()
            && !head.is_empty()
            && !heads.iter().any(|seen| seen == head)
        {
            heads.push(head.to_string());
        }
    }
    heads
}

/// True when at least one draft was written against a head the pull request no
/// longer has.
///
/// A draft that recorded no head, or a pull request whose current head is unknown,
/// says nothing either way: without both sides there is nothing to compare, and
/// warning on a guess would train the reviewer to dismiss the warning.
///
/// What is deliberately absent is any attempt to re-anchor. Matching a draft's
/// content onto the new head puts a reviewer's words on the wrong line the moment
/// the match is ambiguous, and refusing to submit punishes the reviewer for the
/// author's timing. Submitting against the recorded references instead lands each
/// remark on the code that was actually read, and lets the host mark it outdated
/// itself.
pub fn head_moved(drafts: &[DraftComment], current: Option<&DiffRefs>) -> bool {
    let Some(head) = current
        .and_then(|refs| refs.head_sha.as_deref())
        .filter(|head| !head.is_empty())
    else {
        return false;
    };
    drafts.iter().any(|draft| {
        draft
            .refs
            .head_sha
            .as_deref()
            .is_some_and(|drafted| !drafted.is_empty() && drafted != head)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    const APPROVED: MyReviewState = MyReviewState::Approved;
    const PENDING: MyReviewState = MyReviewState::Pending;

    fn refs() -> DiffRefs {
        DiffRefs {
            base_sha: Some("base1".into()),
            start_sha: Some("start1".into()),
            head_sha: Some("head1".into()),
        }
    }

    /// A store with a ticking clock and ids that follow it, as the TypeScript test
    /// builds one. The id is drawn before the date, as the TypeScript spreads them.
    fn store(initial: Vec<DraftComment>) -> DraftStore {
        let tick = Arc::new(AtomicU64::new(0));
        let for_ids = Arc::clone(&tick);
        DraftStore::new(DraftState {
            comments: initial,
            sets: BTreeMap::new(),
        })
        .with_clock(move || {
            let at = tick.fetch_add(1, Ordering::SeqCst);
            format!("2026-08-05T10:00:{at:02}Z")
        })
        .with_ids(move || format!("d{}", for_ids.load(Ordering::SeqCst)))
    }

    fn draft(item_id: &str, body: &str) -> NewDraft {
        NewDraft {
            item_id: item_id.into(),
            body: body.into(),
            path: "src/a.ts".into(),
            new_line: Some(12),
            old_line: None,
            range: None,
            refs: refs(),
        }
    }

    fn bodies(drafts: &[DraftComment]) -> Vec<&str> {
        drafts.iter().map(|draft| draft.body.as_str()).collect()
    }

    #[test]
    fn drafts_are_keyed_per_item_and_never_leak_between_pull_requests() {
        let mut drafts = store(vec![]);

        drafts.add(draft("acct:repo:1", "on the first"), None);
        drafts.add(draft("acct:repo:2", "on the second"), None);
        drafts.add(draft("acct:repo:1", "also on the first"), None);

        assert_eq!(
            bodies(&drafts.list("acct:repo:1")),
            ["on the first", "also on the first"]
        );
        assert_eq!(bodies(&drafts.list("acct:repo:2")), ["on the second"]);
        assert!(drafts.list("acct:repo:3").is_empty());
        assert_eq!(drafts.count("acct:repo:1"), 2);
    }

    #[test]
    fn a_draft_records_the_diff_references_it_was_written_against() {
        let mut drafts = store(vec![]);

        let created = drafts.add(draft("item", "a remark"), None);

        assert_eq!(created.refs, refs());
        assert_eq!(created.path, "src/a.ts");
        assert_eq!(created.new_line, Some(12));
        assert_eq!(created.old_line, None);
        assert!(!created.id.is_empty());
        assert!(!created.created_at.is_empty());

        // A later push must not rewrite what an existing draft was anchored to.
        drafts.add(
            NewDraft {
                refs: DiffRefs {
                    head_sha: Some("head2".into()),
                    ..DiffRefs::default()
                },
                ..draft("item", "later")
            },
            None,
        );
        assert_eq!(drafts.list("item")[0].refs, refs());
    }

    #[test]
    fn ids_and_dates_come_from_the_injected_sources_in_order() {
        let mut drafts = store(vec![]);
        let first = drafts.add(draft("item", "one"), None);
        let second = drafts.add(draft("item", "two"), None);
        assert_eq!(
            (first.id.as_str(), first.created_at.as_str()),
            ("d0", "2026-08-05T10:00:00Z")
        );
        assert_eq!(
            (second.id.as_str(), second.created_at.as_str()),
            ("d1", "2026-08-05T10:00:01Z")
        );
    }

    #[test]
    fn the_default_sources_give_unique_random_ids_and_iso_dates() {
        let mut drafts = DraftStore::new(DraftState::default());
        let a = drafts.add(draft("item", "a"), None);
        let b = drafts.add(draft("item", "b"), None);
        assert_ne!(a.id, b.id);
        assert!(a.id.starts_with("draft-") && a.id.len() == 16, "{}", a.id);
        assert!(crate::time::parse_iso(&a.created_at).is_some());
    }

    #[test]
    fn a_draft_can_be_edited_and_deleted_before_it_is_submitted() {
        let mut drafts = store(vec![]);
        let created = drafts.add(draft("item", "first thought"), None);

        let updated = drafts.update(&created.id, "sharper second thought");
        assert_eq!(
            updated.map(|draft| draft.body).as_deref(),
            Some("sharper second thought")
        );
        assert_eq!(drafts.list("item")[0].body, "sharper second thought");
        // Editing changes the body and nothing else about where it belongs.
        assert_eq!(drafts.list("item")[0].refs, refs());

        assert_eq!(
            drafts.remove(&created.id).map(|draft| draft.id),
            Some(created.id.clone())
        );
        assert!(drafts.list("item").is_empty());

        assert_eq!(drafts.update("gone", "x"), None);
        assert_eq!(drafts.remove("gone"), None);
    }

    #[test]
    fn clearing_takes_one_item_drafts_and_leaves_every_other_item_alone() {
        let mut drafts = store(vec![]);
        drafts.add(draft("item-a", "one"), None);
        drafts.add(draft("item-a", "two"), None);
        drafts.add(draft("item-b", "elsewhere"), None);

        drafts.clear("item-a");

        assert!(drafts.list("item-a").is_empty());
        assert_eq!(drafts.count("item-b"), 1);
    }

    #[test]
    fn a_failed_submission_leaves_the_draft_set_exactly_as_it_was() {
        // Nothing is taken away in advance, so there is nothing to put back: the store is
        // only cleared once a submission has come back clean.
        let mut drafts = store(vec![]);
        drafts.add(draft("item", "one"), None);
        drafts.add(draft("item", "two"), None);

        let before = drafts.list("item");
        let submit =
            |_: &[DraftComment]| -> crate::Result<()> { Err(crate::msg("the host said no")) };

        match submit(&drafts.list("item")) {
            Ok(()) => panic!("the submission should have failed"),
            Err(_) => {
                assert_eq!(drafts.list("item"), before);
                assert_eq!(drafts.count("item"), 2);
            }
        }
    }

    #[test]
    fn writing_does_not_touch_the_store_per_keystroke() {
        // The app's debounce stands in for the timer: it writes only when it fires,
        // and then only if something changed.
        let mut saved: Vec<DraftState> = Vec::new();
        let mut drafts = store(vec![]);
        let created = drafts.add(draft("item", "a"), None);

        for body in ["ab", "abc", "abcd", "abcde"] {
            drafts.update(&created.id, body);
        }

        // Five changes so far, and the file has not been rewritten once: the store
        // only remembers that something is waiting.
        assert!(saved.is_empty());
        assert!(drafts.is_dirty());

        saved.push(drafts.flush());
        assert_eq!(saved.len(), 1, "one write for the lot");
        assert_eq!(saved[0].comments[0].body, "abcde");
        assert!(!drafts.is_dirty());
    }

    #[test]
    fn the_debounced_write_does_land_on_its_own() {
        let mut saved: Vec<DraftState> = Vec::new();
        let mut drafts = store(vec![]);
        drafts.add(draft("item", "a"), None);

        // The debounce firing: it takes the flag and writes what is there.
        if drafts.take_dirty() {
            saved.push(drafts.snapshot());
        }
        // A second firing with nothing new writes nothing.
        if drafts.take_dirty() {
            saved.push(drafts.snapshot());
        }

        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].comments[0].body, "a");
    }

    #[test]
    fn every_change_marks_the_store_dirty_and_a_no_op_does_not() {
        let mut drafts = store(vec![]);
        assert!(!drafts.is_dirty(), "loading is not a change");

        let created = drafts.add(draft("item", "a"), Some(PENDING));
        assert!(drafts.take_dirty());

        drafts.update("gone", "x");
        drafts.remove("gone");
        drafts.clear("nothing-here");
        drafts.acknowledge("nothing-here", APPROVED);
        drafts.record_submission("nothing-here", APPROVED);
        assert!(!drafts.reconcile("item", PENDING, drafts.submissions()));
        assert!(!drafts.is_dirty(), "nothing changed");

        drafts.update(&created.id, "b");
        assert!(drafts.take_dirty());
        drafts.reconcile("item", APPROVED, drafts.submissions());
        assert!(drafts.take_dirty());
        drafts.acknowledge("item", APPROVED);
        assert!(drafts.take_dirty());
        drafts.record_submission("item", APPROVED);
        assert!(drafts.take_dirty());
        drafts.remove(&created.id);
        assert!(drafts.take_dirty());
    }

    #[test]
    fn drafts_written_before_a_restart_are_there_afterwards() {
        let mut first = store(vec![]);
        first.add(draft("item", "survives"), None);
        let saved = first.flush();

        let reopened = DraftStore::new(saved);

        assert_eq!(bodies(&reopened.list("item")), ["survives"]);
    }

    fn written(head: Option<&str>, id: &str) -> DraftComment {
        DraftComment {
            id: id.into(),
            item_id: "item".into(),
            body: "a remark".into(),
            path: "src/a.ts".into(),
            new_line: Some(12),
            old_line: None,
            range: None,
            created_at: "2026-08-05T10:00:00Z".into(),
            refs: match head {
                Some(head) => DiffRefs {
                    base_sha: Some("base1".into()),
                    start_sha: Some("start1".into()),
                    head_sha: Some(head.into()),
                },
                None => DiffRefs::default(),
            },
        }
    }

    fn head(sha: &str) -> DiffRefs {
        DiffRefs {
            head_sha: Some(sha.into()),
            ..DiffRefs::default()
        }
    }

    #[test]
    fn head_moved_says_nothing_when_the_pull_request_has_not_moved() {
        let drafts = [written(Some("head1"), "a"), written(Some("head1"), "b")];

        assert!(!head_moved(&drafts, Some(&head("head1"))));
        assert!(!head_moved(
            &drafts,
            Some(&DiffRefs {
                base_sha: Some("base9".into()),
                start_sha: Some("start9".into()),
                head_sha: Some("head1".into()),
            })
        ));
        // Nothing drafted, nothing to warn about.
        assert!(!head_moved(&[], Some(&head("head2"))));
    }

    #[test]
    fn head_moved_spots_a_head_the_pull_request_no_longer_has() {
        assert!(head_moved(
            &[written(Some("head1"), "head1")],
            Some(&head("head2"))
        ));

        // One stale draft among fresh ones is still a push the reviewer should know about.
        let mixed = [
            written(Some("head2"), "a"),
            written(Some("head1"), "b"),
            written(Some("head2"), "c"),
        ];
        assert!(head_moved(&mixed, Some(&head("head2"))));
    }

    #[test]
    fn head_moved_refuses_to_warn_on_a_guess() {
        // Without both sides there is nothing to compare, and a warning nobody can act on
        // is one the reviewer learns to dismiss.
        assert!(!head_moved(&[written(None, "none")], Some(&head("head2"))));
        assert!(!head_moved(&[written(Some("head1"), "head1")], None));
        assert!(!head_moved(
            &[written(Some("head1"), "head1")],
            Some(&DiffRefs::default())
        ));
        assert!(!head_moved(
            &[written(Some("head1"), "head1")],
            Some(&DiffRefs {
                base_sha: Some("base1".into()),
                ..DiffRefs::default()
            })
        ));
        // An empty head is no head, on either side.
        assert!(!head_moved(&[written(Some("head1"), "a")], Some(&head(""))));
        assert!(!head_moved(&[written(Some(""), "a")], Some(&head("head2"))));
    }

    #[test]
    fn drafted_heads_names_each_head_once_in_the_order_it_was_first_written_against() {
        assert_eq!(
            drafted_heads(&[
                written(Some("head1"), "a"),
                written(Some("head2"), "b"),
                written(Some("head1"), "c"),
            ]),
            ["head1", "head2"]
        );
        assert!(drafted_heads(&[written(None, "none")]).is_empty());
        assert!(drafted_heads(&[]).is_empty());
    }

    fn check(
        baseline: Option<MyReviewState>,
        current: MyReviewState,
        at_start: u64,
        now: u64,
    ) -> DivergenceCheck {
        DivergenceCheck {
            baseline,
            current,
            submissions_at_sync_start: at_start,
            submissions_now: now,
        }
    }

    #[test]
    fn has_diverged_reports_a_review_the_reviewer_submitted_somewhere_else() {
        assert!(has_diverged(check(Some(PENDING), APPROVED, 0, 0)));
    }

    #[test]
    fn has_diverged_says_nothing_when_the_state_has_not_moved() {
        assert!(!has_diverged(check(Some(PENDING), PENDING, 3, 3)));
    }

    #[test]
    fn has_diverged_says_nothing_without_a_baseline_to_compare_against() {
        assert!(!has_diverged(check(None, APPROVED, 0, 0)));
    }

    #[test]
    fn has_diverged_never_reports_this_app_own_submission_as_somebody_else() {
        // A sync that began before the submission is holding the state from before it.
        // Reading that as a divergence would accuse the reviewer of their own review.
        assert!(!has_diverged(check(Some(APPROVED), PENDING, 0, 1)));
    }

    #[test]
    fn a_draft_set_takes_its_baseline_from_the_state_when_it_began() {
        let mut drafts = store(vec![]);

        drafts.add(draft("item", "first"), Some(PENDING));
        // A later draft does not move the baseline the set started from.
        drafts.add(draft("item", "second"), Some(APPROVED));

        assert!(!drafts.reconcile("item", PENDING, 0));
        assert!(drafts.reconcile("item", APPROVED, 0));
        assert!(drafts.diverged("item"));
    }

    #[test]
    fn a_set_created_after_an_external_change_starts_from_what_is_there_now() {
        // The reviewer approved in a browser, then came back and started drafting. The
        // change is already reflected, so there is nothing to reconcile.
        let mut drafts = store(vec![]);
        drafts.add(draft("item", "written after the fact"), Some(APPROVED));

        assert!(!drafts.reconcile("item", APPROVED, 0));
        assert!(!drafts.diverged("item"));
    }

    #[test]
    fn an_item_with_no_drafts_never_reports_divergence() {
        let mut drafts = store(vec![]);

        assert!(!drafts.reconcile("untouched", APPROVED, 0));
        assert!(!drafts.diverged("untouched"));

        // Nor once its drafts are gone.
        drafts.add(draft("item", "one"), Some(PENDING));
        drafts.reconcile("item", APPROVED, 0);
        assert!(drafts.diverged("item"));
        drafts.clear("item");
        assert!(!drafts.diverged("item"));
    }

    #[test]
    fn the_app_own_submission_rebaselines_instead_of_diverging() {
        let mut drafts = store(vec![]);
        drafts.add(draft("item", "one"), Some(PENDING));

        let before = drafts.submissions();
        drafts.record_submission("item", APPROVED);
        assert_eq!(drafts.submissions(), before + 1);

        // A sync that started before it is ignored, and the state it settled on is the
        // new baseline, so the next sync agrees.
        assert!(!drafts.reconcile("item", PENDING, before));
        let now = drafts.submissions();
        assert!(!drafts.reconcile("item", APPROVED, now));
        assert!(!drafts.diverged("item"));
    }

    #[test]
    fn keeping_drafts_clears_the_mark_and_touches_not_one_character_of_them() {
        let mut drafts = store(vec![]);
        drafts.add(draft("item", "a carefully worded remark"), Some(PENDING));
        drafts.reconcile("item", APPROVED, 0);
        assert!(drafts.diverged("item"));

        let before = drafts.list("item");
        drafts.acknowledge("item", APPROVED);

        assert!(!drafts.diverged("item"));
        assert_eq!(drafts.list("item"), before);

        // And the same change is not reported a second time.
        assert!(!drafts.reconcile("item", APPROVED, 0));
    }

    #[test]
    fn the_diverged_mark_is_written_down_so_it_is_still_there_after_a_restart() {
        let mut first = store(vec![]);
        first.add(draft("item", "one"), Some(PENDING));
        first.reconcile("item", APPROVED, 0);
        let saved = first.flush();

        let reopened = DraftStore::new(saved);

        assert!(reopened.diverged("item"));
        assert_eq!(reopened.count("item"), 1);
    }

    #[test]
    fn divergence_is_per_item_and_does_not_spread() {
        let mut drafts = store(vec![]);
        drafts.add(draft("item-a", "one"), Some(PENDING));
        drafts.add(draft("item-b", "two"), Some(PENDING));

        drafts.reconcile("item-a", APPROVED, 0);

        assert!(drafts.diverged("item-a"));
        assert!(!drafts.diverged("item-b"));
    }

    #[test]
    fn list_sorts_oldest_first_whatever_order_the_vault_holds() {
        let mut later = written(Some("head1"), "later");
        later.created_at = "2026-08-05T11:00:00Z".into();
        let earlier = written(Some("head1"), "earlier");
        let drafts = store(vec![later, earlier]);
        let ids: Vec<String> = drafts
            .list("item")
            .into_iter()
            .map(|draft| draft.id)
            .collect();
        assert_eq!(ids, ["earlier", "later"]);
    }

    #[test]
    fn draft_state_has_the_vault_shape() {
        let mut drafts = store(vec![]);
        drafts.add(draft("item", "one"), Some(PENDING));
        drafts.reconcile("item", APPROVED, 0);

        let value = serde_json::to_value(drafts.snapshot()).expect("serialisable");
        assert_eq!(
            value,
            json!({
                "drafts": [{
                    "id": "d0",
                    "itemId": "item",
                    "body": "one",
                    "path": "src/a.ts",
                    "newLine": 12,
                    "createdAt": "2026-08-05T10:00:00Z",
                    "refs": { "baseSha": "base1", "startSha": "start1", "headSha": "head1" },
                }],
                "draftSets": { "item": { "baseline": "pending", "diverged": true } },
            })
        );
        let back: DraftState = serde_json::from_value(value).expect("deserialisable");
        assert_eq!(back, drafts.snapshot());
    }

    #[test]
    fn draft_state_flattens_into_the_vault_and_tolerates_what_it_cannot_read() {
        #[derive(Deserialize)]
        struct Vault {
            version: u32,
            #[serde(flatten)]
            drafts: DraftState,
        }

        let vault: Vault = serde_json::from_value(json!({
            "version": 1,
            "drafts": [
                {
                    "id": "d1", "itemId": "item", "body": "kept", "path": "a.ts",
                    "createdAt": "2026-08-05T10:00:00Z", "refs": {},
                },
                { "id": "broken" },
                "not a draft",
            ],
            "draftSets": {
                "item": { "baseline": "approved", "diverged": false },
                "odd": { "baseline": "sideways", "diverged": false },
            },
        }))
        .expect("a vault with a bad draft still loads");
        assert_eq!(vault.version, 1);
        assert_eq!(bodies(&vault.drafts.comments), ["kept"]);
        assert_eq!(vault.drafts.sets.keys().collect::<Vec<_>>(), ["item"]);

        // Written before drafts existed, or with nulls: empty, not an error.
        let older: Vault = serde_json::from_value(json!({ "version": 1 })).expect("loads");
        assert_eq!(older.drafts, DraftState::default());
        let nulls: Vault =
            serde_json::from_value(json!({ "version": 1, "drafts": null, "draftSets": null }))
                .expect("loads");
        assert_eq!(nulls.drafts, DraftState::default());
    }
}
