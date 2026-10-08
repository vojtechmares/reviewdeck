//! Persistence: a port of src/main/store.ts.
//!
//! Accounts and settings land in a plain JSON file under the app's data directory -
//! the very file the Electron app wrote, `~/Library/Application Support/reviewdeck/
//! reviewdeck.json`, in the same shape - while tokens go to the macOS Keychain
//! through a [`TokenStore`], so they never sit on disk in the clear.
//!
//! The Electron app kept its tokens in the file's `tokens` map, encrypted by
//! `safeStorage`. Opening the vault moves every one it can into the Keychain and
//! drops it from the map; one that cannot be moved yet stays where it is and is
//! tried again on the next launch (see [`crate::keychain`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Result, msg};
use crate::keychain::{KeychainTokens, decrypt_safe_storage};
use crate::model::{Account, DraftComment, NewAccount, ReviewItem, Settings, merge_settings};
use crate::time::{now_iso, now_ms};

/// What `get_token` says when there is nothing to hand back.
pub const NO_TOKEN: &str = "No token stored for this account. Remove it and sign in again.";

/// How many seen ids the vault keeps; closed PRs never come back with the same id.
const SEEN_LIMIT: usize = 2000;

/// The deck cache envelope version this build reads and writes (`DECK_CACHE_VERSION`
/// of src/shared/deck-cache.ts). Bumped whenever a ReviewItem gains a field the
/// cards rely on, so a deck written by an older build is discarded rather than
/// drawn with holes in it.
const DECK_CACHE_VERSION: u32 = 2;

/// Where account tokens live. The app uses [`KeychainTokens`]; tests use
/// [`MemoryTokens`].
pub trait TokenStore: Send + Sync {
    /// The token stored for an account, or `None` when there is none.
    fn get(&self, account_id: &str) -> Result<Option<String>>;
    /// Stores (or replaces) an account's token.
    fn set(&self, account_id: &str, token: &str) -> Result<()>;
    /// Forgets an account's token. Forgetting one that is not there is not an error.
    fn delete(&self, account_id: &str) -> Result<()>;
    /// Reads a token the Electron app left in the vault's `tokens` map (the base64 of
    /// a `safeStorage` blob), or `None` when it cannot be read - yet, or ever.
    fn decrypt_legacy(&self, _blob: &str) -> Option<String> {
        None
    }
}

/// Tokens in memory, for tests and for anything that must not touch the Keychain.
#[derive(Default)]
pub struct MemoryTokens {
    tokens: Mutex<HashMap<String, String>>,
    /// The Electron Safe Storage password to read legacy blobs with, if any.
    legacy_password: Option<String>,
}

impl MemoryTokens {
    pub fn new() -> MemoryTokens {
        MemoryTokens::default()
    }

    /// A store that can read `safeStorage` blobs encrypted under `password`, the way
    /// [`KeychainTokens`] reads them with the password Electron kept.
    pub fn with_legacy_password(password: impl Into<String>) -> MemoryTokens {
        MemoryTokens {
            tokens: Mutex::default(),
            legacy_password: Some(password.into()),
        }
    }

    /// Every token held, by account id.
    pub fn snapshot(&self) -> HashMap<String, String> {
        self.tokens.lock().clone()
    }
}

impl TokenStore for MemoryTokens {
    fn get(&self, account_id: &str) -> Result<Option<String>> {
        Ok(self.tokens.lock().get(account_id).cloned())
    }

    fn set(&self, account_id: &str, token: &str) -> Result<()> {
        self.tokens
            .lock()
            .insert(account_id.to_string(), token.to_string());
        Ok(())
    }

    fn delete(&self, account_id: &str) -> Result<()> {
        self.tokens.lock().remove(account_id);
        Ok(())
    }

    fn decrypt_legacy(&self, blob: &str) -> Option<String> {
        decrypt_safe_storage(blob, self.legacy_password.as_deref()?)
    }
}

/// The last synced deck, so a launch has reviews to show before the fan-out
/// (`DeckCache` of src/shared/deck-cache.ts).
#[derive(Debug, Clone, PartialEq, Serialize)]
struct StoredDeck {
    version: u32,
    /// accountId -> the items that account returned on the last completed sync.
    items: BTreeMap<String, Vec<ReviewItem>>,
}

impl StoredDeck {
    fn empty() -> StoredDeck {
        StoredDeck {
            version: DECK_CACHE_VERSION,
            items: BTreeMap::new(),
        }
    }
}

/// A deck cache read back off the vault, or an empty one when there is nothing
/// usable - the `readDeckCache` of src/shared/deck-cache.ts, applied to the vault's
/// `deck` field.
///
/// The cache is a hint, never truth. An envelope that is not what this build
/// writes (a version from another build, a shape that is not an object) takes the
/// whole cache with it. A single item that does not read as a [`ReviewItem`] only takes
/// itself, because the rest of the deck is still worth showing and the sync on its
/// way replaces all of it anyway.
fn read_stored_deck(stored: Option<&Value>) -> StoredDeck {
    let Some(Value::Object(stored)) = stored else {
        return StoredDeck::empty();
    };
    if stored.get("version").and_then(Value::as_f64) != Some(f64::from(DECK_CACHE_VERSION)) {
        return StoredDeck::empty();
    }
    let Some(Value::Object(stored_items)) = stored.get("items") else {
        return StoredDeck::empty();
    };
    let items = stored_items
        .iter()
        .filter_map(|(account_id, cached)| {
            let Value::Array(cached) = cached else {
                return None;
            };
            let items = cached
                .iter()
                .filter_map(|item| ReviewItem::deserialize(item).ok())
                .collect();
            Some((account_id.clone(), items))
        })
        .collect();
    StoredDeck {
        version: DECK_CACHE_VERSION,
        items,
    }
}

/// The file's contents. Field order is the order the Electron app wrote them in.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultData {
    version: u32,
    accounts: Vec<Account>,
    /// accountId -> base64 of a `safeStorage`-encrypted token the Electron app wrote
    /// and this app has not yet moved into the Keychain. Empty once migrated.
    tokens: BTreeMap<String, String>,
    settings: Settings,
    /// Item ids we have already notified about, so a restart does not re-announce them.
    seen: Vec<String>,
    /// Line comments written but not yet submitted, across every pull request.
    drafts: Vec<DraftComment>,
    /// What is known about each item's draft set beyond the text: baseline,
    /// divergence. Kept as stored; the draft store gives the entries their type.
    draft_sets: Map<String, Value>,
    /// The last synced deck, so a launch has reviews to show before the fan-out.
    deck: StoredDeck,
    /// windowId -> the local day its roll-up last fired, so once a day survives a quit.
    windows_fired: BTreeMap<String, String>,
}

impl VaultData {
    fn empty() -> VaultData {
        VaultData {
            version: 1,
            accounts: Vec::new(),
            tokens: BTreeMap::new(),
            settings: Settings::default(),
            seen: Vec::new(),
            drafts: Vec::new(),
            draft_sets: Map::new(),
            deck: StoredDeck::empty(),
            windows_fired: BTreeMap::new(),
        }
    }

    /// The vault as stored, with every field read the way `parsed.field ?? default`
    /// reads it.
    ///
    /// Leniency goes one step further than the TypeScript, which trusted the shape
    /// outright: an entry that does not read as its type (an account or a draft
    /// edited by hand into nonsense) is dropped on its own instead of failing the
    /// whole load, so one bad field never costs the user the vault.
    fn from_json(parsed: &Map<String, Value>) -> VaultData {
        fn list<T: DeserializeOwned>(value: Option<&Value>) -> Vec<T> {
            match value {
                Some(Value::Array(entries)) => entries
                    .iter()
                    .filter_map(|entry| T::deserialize(entry).ok())
                    .collect(),
                _ => Vec::new(),
            }
        }
        fn strings(value: Option<&Value>) -> BTreeMap<String, String> {
            match value {
                Some(Value::Object(entries)) => entries
                    .iter()
                    .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
                    .collect(),
                _ => BTreeMap::new(),
            }
        }

        VaultData {
            version: 1,
            accounts: list(parsed.get("accounts")),
            tokens: strings(parsed.get("tokens")),
            settings: merge_settings(parsed.get("settings")),
            seen: list(parsed.get("seen")),
            drafts: list(parsed.get("drafts")),
            draft_sets: match parsed.get("draftSets") {
                Some(Value::Object(sets)) => sets.clone(),
                _ => Map::new(),
            },
            deck: read_stored_deck(parsed.get("deck")),
            windows_fired: strings(parsed.get("windowsFired")),
        }
    }
}

/// The app's data directory: `~/Library/Application Support/reviewdeck`, the
/// Electron app's `userData`, so both read the same vault.
pub fn data_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| msg("Could not find the home folder to keep Reviewdeck's data in."))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("reviewdeck"))
}

/// The vault file: `<data dir>/reviewdeck.json`.
pub fn vault_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("reviewdeck.json"))
}

/// `path` with `suffix` appended to its file name, e.g. `reviewdeck.json.tmp`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Reads the vault at `path`: empty when there is none, and empty too - with the
/// bad copy kept beside it for forensics - when it cannot be read, because a
/// corrupt file should not brick the app.
fn load(path: &Path) -> VaultData {
    let parsed = match fs::read_to_string(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return VaultData::empty(),
        Err(error) => Err(error.to_string()),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(parsed)) => Ok(parsed),
            Ok(_) => Err("the vault is not a JSON object".to_string()),
            Err(error) => Err(error.to_string()),
        },
    };
    match parsed {
        Ok(parsed) => VaultData::from_json(&parsed),
        Err(error) => {
            eprintln!("[store] could not read vault, starting fresh: {error}");
            // Best effort: losing the forensic copy is not worth failing over.
            let _ = fs::rename(path, with_suffix(path, &format!(".corrupt-{}", now_ms())));
            VaultData::empty()
        }
    }
}

/// The vault: accounts, settings and everything else the app remembers between
/// launches, held in memory and written through to disk on every change.
///
/// Every method takes `&self` (the data sits behind a lock), so one `Arc<Vault>`
/// can be shared by whatever needs it. Every method of store.ts has its namesake
/// here; the ones that write return the write's error, as the TypeScript threw it.
pub struct Vault {
    path: PathBuf,
    tokens: Arc<dyn TokenStore>,
    data: Mutex<VaultData>,
}

impl Vault {
    /// The app's vault, with tokens in the Keychain.
    pub fn open() -> Result<Vault> {
        Ok(Vault::open_at(
            vault_path()?,
            Arc::new(KeychainTokens::new()),
        ))
    }

    /// The vault at `path`, with tokens in `tokens`. Moves any token the Electron app
    /// left in the file into `tokens`; one that cannot be moved stays for next time.
    pub fn open_at(path: impl Into<PathBuf>, tokens: Arc<dyn TokenStore>) -> Vault {
        let path = path.into();
        let vault = Vault {
            data: Mutex::new(load(&path)),
            path,
            tokens,
        };
        vault.migrate_legacy_tokens();
        vault
    }

    /// Where this vault is written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate_legacy_tokens(&self) {
        let mut data = self.data.lock();
        let before = data.tokens.len();
        data.tokens.retain(|account_id, blob| {
            let Some(token) = self.tokens.decrypt_legacy(blob) else {
                return true;
            };
            match self.tokens.set(account_id, &token) {
                Ok(()) => false,
                Err(error) => {
                    eprintln!("[store] could not move a token into the Keychain: {error}");
                    true
                }
            }
        });
        if data.tokens.len() != before
            && let Err(error) = self.persist(&data)
        {
            // The tokens are in the Keychain already; the next launch finds the
            // blobs again and migrates them again, which is harmless.
            eprintln!("[store] could not save the vault after migrating tokens: {error}");
        }
    }

    /// Writes the vault atomically: a private temp file renamed over the real one, so
    /// a crash mid-write leaves the old vault rather than half of a new one.
    fn persist(&self, data: &VaultData) -> Result<()> {
        let fail = |error: &dyn std::fmt::Display| {
            msg(format!(
                "Could not save Reviewdeck's data to {} ({error}).",
                self.path.display()
            ))
        };
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|e| fail(&e))?;
        }
        let json = serde_json::to_string_pretty(data).map_err(|e| fail(&e))?;
        let tmp = with_suffix(&self.path, ".tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| fail(&e))?;
        file.write_all(json.as_bytes()).map_err(|e| fail(&e))?;
        drop(file);
        fs::rename(&tmp, &self.path).map_err(|e| fail(&e))
    }

    pub fn list_accounts(&self) -> Vec<Account> {
        self.data.lock().accounts.clone()
    }

    pub fn get_account(&self, id: &str) -> Option<Account> {
        self.data
            .lock()
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
    }

    /// Files a newly connected account under a fresh id, with its token in the token
    /// store. The token goes first, so an account never lands without one.
    pub fn add_account(&self, account: NewAccount, token: &str) -> Result<Account> {
        let created = account.into_account(uuid::Uuid::new_v4().to_string(), now_iso());
        self.tokens.set(&created.id, token)?;
        let mut data = self.data.lock();
        data.accounts.push(created.clone());
        self.persist(&data)?;
        Ok(created)
    }

    /// Applies `patch` to an account. Its id survives whatever the patch does to it;
    /// an unknown id is quietly nothing to do.
    pub fn update_account(&self, id: &str, patch: impl FnOnce(&mut Account)) -> Result<()> {
        let mut data = self.data.lock();
        let Some(account) = data.accounts.iter_mut().find(|account| account.id == id) else {
            return Ok(());
        };
        patch(account);
        account.id = id.to_string();
        self.persist(&data)
    }

    pub fn remove_account(&self, id: &str) -> Result<()> {
        if let Err(error) = self.tokens.delete(id) {
            eprintln!("[store] could not remove a token from the Keychain: {error}");
        }
        let mut data = self.data.lock();
        data.accounts.retain(|account| account.id != id);
        data.tokens.remove(id);
        // A signed-out host does not get to leave its reviews on disk.
        data.deck.items.remove(id);
        self.persist(&data)
    }

    pub fn get_token(&self, id: &str) -> Result<String> {
        match self.tokens.get(id)? {
            Some(token) if !token.is_empty() => Ok(token),
            _ => Err(msg(NO_TOKEN)),
        }
    }

    pub fn set_token(&self, id: &str, token: &str) -> Result<()> {
        let mut data = self.data.lock();
        if !data.accounts.iter().any(|account| account.id == id) {
            return Err(msg("No such account."));
        }
        self.tokens.set(id, token)?;
        // A blob the Electron app left for this account is older than the token just
        // saved; left in place, the next launch would migrate it over the new one.
        if data.tokens.remove(id).is_some() {
            self.persist(&data)?;
        }
        Ok(())
    }

    /// The drafts and the per-item draft sets (`loadDraftState`). `S` is the draft
    /// store's set type; a stored set that does not read as one is left out.
    ///
    /// ```ignore
    /// let (comments, sets): (_, HashMap<String, DraftSet>) = vault.load_draft_state();
    /// ```
    pub fn load_draft_state<S, C>(&self) -> (Vec<DraftComment>, C)
    where
        S: DeserializeOwned,
        C: FromIterator<(String, S)>,
    {
        let data = self.data.lock();
        let sets = data
            .draft_sets
            .iter()
            .filter_map(|(item_id, set)| Some((item_id.clone(), S::deserialize(set).ok()?)))
            .collect();
        (data.drafts.clone(), sets)
    }

    /// Replaces the drafts and the draft sets wholesale (`persistDraftState`).
    pub fn persist_draft_state<K, S>(
        &self,
        comments: &[DraftComment],
        sets: impl IntoIterator<Item = (K, S)>,
    ) -> Result<()>
    where
        K: AsRef<str>,
        S: Serialize,
    {
        let mut draft_sets = Map::new();
        for (item_id, set) in sets {
            let value = serde_json::to_value(set)
                .map_err(|error| msg(format!("Could not save a draft ({error}).")))?;
            draft_sets.insert(item_id.as_ref().to_string(), value);
        }
        let mut data = self.data.lock();
        data.drafts = comments.to_vec();
        data.draft_sets = draft_sets;
        self.persist(&data)
    }

    /// The cached deck, minus anything belonging to an account that is no longer here -
    /// a vault edited by hand, or an account dropped by a build that did not prune.
    pub fn load_deck_cache(&self) -> HashMap<String, Vec<ReviewItem>> {
        let data = self.data.lock();
        let connected: HashSet<&str> = data
            .accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect();
        data.deck
            .items
            .iter()
            .filter(|(account_id, _)| connected.contains(account_id.as_str()))
            .map(|(account_id, items)| (account_id.clone(), items.clone()))
            .collect()
    }

    /// Replaces the cached deck wholesale, so a sync's removals are removals here too.
    pub fn persist_deck_cache(
        &self,
        items: impl IntoIterator<Item = (String, Vec<ReviewItem>)>,
    ) -> Result<()> {
        let mut data = self.data.lock();
        data.deck = StoredDeck {
            version: DECK_CACHE_VERSION,
            items: items.into_iter().collect(),
        };
        self.persist(&data)
    }

    /// The local day each review window last fired, so a relaunch mid-span knows.
    pub fn load_windows_fired(&self) -> HashMap<String, String> {
        self.data
            .lock()
            .windows_fired
            .iter()
            .map(|(id, day)| (id.clone(), day.clone()))
            .collect()
    }

    /// Records a roll-up against every window that raised it, on the day it went out.
    pub fn record_windows_fired(&self, window_ids: &[String], day: &str) -> Result<()> {
        if window_ids.is_empty() {
            return Ok(());
        }
        let mut data = self.data.lock();
        for id in window_ids {
            data.windows_fired.insert(id.clone(), day.to_string());
        }
        self.persist(&data)
    }

    /// `getSettings`.
    pub fn settings(&self) -> Settings {
        self.data.lock().settings.clone()
    }

    /// Applies `patch` to the settings, saves them, and returns the result.
    pub fn save_settings(&self, patch: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut data = self.data.lock();
        patch(&mut data.settings);
        self.persist(&data)?;
        Ok(data.settings.clone())
    }

    /// Returns the ids that are new since the last call and records them, so the
    /// poller only ever raises a notification once per review request.
    pub fn mark_seen(&self, ids: &[String]) -> Result<Vec<String>> {
        let mut data = self.data.lock();
        let known: HashSet<&str> = data.seen.iter().map(String::as_str).collect();
        let fresh: Vec<String> = ids
            .iter()
            .filter(|id| !known.contains(id.as_str()))
            .cloned()
            .collect();
        if !fresh.is_empty() {
            // Keep the list bounded; closed PRs never come back with the same id.
            data.seen.extend(fresh.iter().cloned());
            let overflow = data.seen.len().saturating_sub(SEEN_LIMIT);
            data.seen.drain(..overflow);
            self.persist(&data)?;
        }
        Ok(fresh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ApprovalOutcome, DiffViewMode, MyReviewState, ProviderKind, ThemeMode};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt as _;

    /// The Electron Safe Storage password the fixture's blob was encrypted under
    /// (independently, with openssl - see keychain.rs).
    const LEGACY_PASSWORD: &str = "Xk3vQ9bL0mZp7Rt2Wy5uHg==";
    const LEGACY_BLOB: &str =
        "djEwYAo1slY1oluFXS34vk1PusCzbYGFYTANJzYgfr6IC92FDzOM2iiiRwSwfeD8pVv3";
    const LEGACY_TOKEN: &str = "ghp_ExampleToken1234567890abcdefXYZ";
    /// A blob no password on hand opens.
    const STUCK_BLOB: &str = "djEwAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

    const GITHUB_ID: &str = "0b6f1f0e-3c51-4c5b-9a43-2f5d8f1c9e01";
    const GITLAB_ID: &str = "7d2c4b8a-1e9f-4a63-8b5d-0c3e2f1a9b77";

    /// A directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("reviewdeck-store-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir).expect("temp dir");
            TempDir(dir)
        }

        fn vault(&self) -> PathBuf {
            self.0.join("reviewdeck.json")
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.0)
                .expect("read dir")
                .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .collect();
            names.sort();
            names
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn review_item(account_id: &str, number: u64) -> Value {
        json!({
            "id": format!("{account_id}:acme/api:{number}"),
            "accountId": account_id,
            "provider": "github",
            "repoKey": "acme/api",
            "repo": "acme/api",
            "number": number,
            "title": "Retry idempotent payment captures",
            "url": format!("https://github.com/acme/api/pull/{number}"),
            "author": { "name": "hkramer", "avatarUrl": "" },
            "createdAt": "2026-08-01T10:00:00.000Z",
            "updatedAt": "2026-08-02T10:00:00.000Z",
            "draft": false,
            "sourceBranch": "fix/capture-retry",
            "targetBranch": "main",
            "labels": ["payments"],
            "myReviewState": "pending",
            "approvals": { "given": 1, "required": 2, "outcome": "pending" },
            "checks": {
                "status": "failed", "passed": 1, "failed": 1, "running": 0, "total": 2,
                "runs": [
                    { "id": "1", "name": "build", "status": "passed", "description": "Compiled in 42s" },
                    { "id": "2", "name": "unit", "status": "failed", "url": "https://ci.example/2" }
                ]
            },
            "additions": 38,
            "deletions": 12,
            "changedFiles": 2
        })
    }

    /// A vault exactly as the Electron app writes it: `JSON.stringify(vault, null, 2)`
    /// of store.ts's `Vault`, with accounts spread from `connect` and then given an
    /// id and a date, settings from a build that predates the agent command and the
    /// draft filter, and each token still in safeStorage form.
    fn electron_fixture() -> Value {
        json!({
            "version": 1,
            "accounts": [
                {
                    "kind": "github",
                    "label": "Work GitHub",
                    "baseUrl": "https://api.github.com",
                    "webUrl": "https://github.com",
                    "username": "vojtechmares",
                    "displayName": "Vojtěch Mareš",
                    "avatarUrl": "https://avatars.githubusercontent.com/u/1?v=4",
                    "id": GITHUB_ID,
                    "addedAt": "2026-07-01T08:30:00.000Z"
                },
                {
                    "kind": "gitlab",
                    "label": "Client GitLab",
                    "baseUrl": "https://gitlab.acme.dev/api/v4",
                    "webUrl": "https://gitlab.acme.dev",
                    "username": "vmares",
                    "displayName": "Vojtěch Mareš",
                    "avatarUrl": "",
                    "agentCommand": "claude-acme",
                    "id": GITLAB_ID,
                    "addedAt": "2026-07-02T09:00:00.000Z"
                }
            ],
            "tokens": {
                GITHUB_ID: LEGACY_BLOB,
                GITLAB_ID: STUCK_BLOB
            },
            "settings": {
                "pollInterval": 300,
                "checkPollInterval": 45,
                "notificationsEnabled": true,
                "playSound": false,
                "diffView": "unified",
                "theme": "dark",
                "hideApproved": true,
                "hideFullyApproved": true,
                "showMenuBarCount": true,
                "launchAtLogin": false,
                "reviewWindows": [
                    { "id": "w1", "enabled": true, "days": [1, 2, 3, 4, 5], "start": "09:00", "end": "10:00", "minimum": 1 }
                ]
            },
            "seen": [
                format!("{GITHUB_ID}:acme/api:411"),
                format!("{GITHUB_ID}:acme/api:412")
            ],
            "drafts": [
                {
                    "id": "d1",
                    "itemId": format!("{GITHUB_ID}:acme/api:412"),
                    "body": "Could this be configurable?",
                    "path": "internal/payments/capture.go",
                    "newLine": 55,
                    "range": {
                        "startLine": 53,
                        "start": { "kind": "add", "oldPos": 43, "newPos": 53 },
                        "end": { "kind": "add", "oldPos": 43, "newPos": 55 }
                    },
                    "createdAt": "2026-08-02T11:00:00.000Z",
                    "refs": { "headSha": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }
                }
            ],
            "draftSets": {
                format!("{GITHUB_ID}:acme/api:412"): { "baseline": "pending", "diverged": false }
            },
            "deck": {
                "version": 2,
                "items": {
                    GITHUB_ID: [review_item(GITHUB_ID, 412)],
                    "gone-account": [review_item("gone-account", 7)]
                }
            },
            "windowsFired": { "w1": "2026-08-03" }
        })
    }

    fn write_json(path: &Path, value: &Value) {
        let text = serde_json::to_string_pretty(value).expect("json");
        fs::write(path, text).expect("write fixture");
    }

    fn read_json(path: &Path) -> Value {
        let text = fs::read_to_string(path).expect("read vault");
        serde_json::from_str(&text).expect("vault is JSON")
    }

    fn open_fixture(dir: &TempDir, tokens: Arc<MemoryTokens>) -> Vault {
        write_json(&dir.vault(), &electron_fixture());
        Vault::open_at(dir.vault(), tokens)
    }

    fn new_account(label: &str) -> NewAccount {
        NewAccount {
            kind: ProviderKind::Forgejo,
            label: label.into(),
            base_url: "https://codeberg.org/api/v1".into(),
            web_url: "https://codeberg.org".into(),
            username: "vmares".into(),
            display_name: "Vojtěch Mareš".into(),
            avatar_url: String::new(),
            agent_command: None,
        }
    }

    fn item(account_id: &str, number: u64) -> ReviewItem {
        ReviewItem::deserialize(review_item(account_id, number)).expect("a review item")
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// What `DraftSet` of src/main/drafts.ts looks like, for the typed draft state.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct DraftSet {
        baseline: MyReviewState,
        diverged: bool,
    }

    #[test]
    fn an_electron_vault_loads_with_everything_in_it() {
        let dir = TempDir::new();
        let vault = open_fixture(&dir, Arc::new(MemoryTokens::new()));

        let accounts = vault.list_accounts();
        assert_eq!(accounts.len(), 2);
        assert_eq!(accounts[0].id, GITHUB_ID);
        assert_eq!(accounts[0].kind, ProviderKind::Github);
        assert_eq!(accounts[0].display_name, "Vojtěch Mareš");
        assert_eq!(accounts[0].added_at, "2026-07-01T08:30:00.000Z");
        assert_eq!(accounts[0].agent_command, None);
        assert_eq!(accounts[1].agent_command.as_deref(), Some("claude-acme"));
        assert_eq!(
            vault.get_account(GITLAB_ID).map(|account| account.label),
            Some("Client GitLab".to_string())
        );
        assert_eq!(vault.get_account("nope"), None);

        // Settings from an older build: stored values kept, the rest from defaults.
        let settings = vault.settings();
        assert_eq!(settings.poll_interval, 300);
        assert!(!settings.play_sound);
        assert!(settings.hide_approved);
        assert_eq!(settings.diff_view, DiffViewMode::Unified);
        assert_eq!(settings.theme, ThemeMode::Dark);
        assert!(settings.hide_drafts, "missing, so the default");
        assert_eq!(settings.agent_command, "claude", "missing, so the default");
        assert_eq!(settings.review_windows.len(), 1);
        assert!(settings.review_windows[0].accounts.is_empty());

        let (drafts, sets): (_, HashMap<String, DraftSet>) = vault.load_draft_state();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].new_line, Some(55));
        assert_eq!(drafts[0].range.map(|range| range.start_line), Some(53));
        assert_eq!(
            sets.get(&format!("{GITHUB_ID}:acme/api:412")),
            Some(&DraftSet {
                baseline: MyReviewState::Pending,
                diverged: false
            })
        );

        let deck = vault.load_deck_cache();
        assert_eq!(
            deck.len(),
            1,
            "a departed account's reviews are not handed out"
        );
        assert_eq!(deck[GITHUB_ID], vec![item(GITHUB_ID, 412)]);
        assert_eq!(
            deck[GITHUB_ID][0].approvals.outcome,
            ApprovalOutcome::Pending
        );

        assert_eq!(
            vault.load_windows_fired(),
            HashMap::from([("w1".to_string(), "2026-08-03".to_string())])
        );
    }

    #[test]
    fn what_is_written_back_has_the_shape_the_electron_app_wrote() {
        let dir = TempDir::new();
        let fixture = electron_fixture();
        let vault = open_fixture(&dir, Arc::new(MemoryTokens::new()));
        // Nothing migrated (no password), so nothing was written yet.
        assert_eq!(read_json(&dir.vault()), fixture);

        vault.save_settings(|_| {}).expect("save");
        let written = read_json(&dir.vault());
        let Value::Object(written) = written else {
            panic!("the vault is an object");
        };
        assert_eq!(
            written.keys().collect::<Vec<_>>(),
            [
                "version",
                "accounts",
                "tokens",
                "settings",
                "seen",
                "drafts",
                "draftSets",
                "deck",
                "windowsFired"
            ]
        );
        for key in [
            "version",
            "accounts",
            "tokens",
            "seen",
            "drafts",
            "draftSets",
            "deck",
            "windowsFired",
        ] {
            assert_eq!(written[key], fixture[key], "{key} round-trips");
        }
        // Settings come back whole: every stored value, every missing default.
        let mut settings = fixture["settings"].clone();
        settings["hideDrafts"] = json!(true);
        settings["agentCommand"] = json!("claude");
        settings["reviewWindows"][0]["accounts"] = json!([]);
        assert_eq!(written["settings"], settings);

        // Written privately, atomically, with no temp file left behind.
        let mode = fs::metadata(dir.vault())
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(dir.entries(), ["reviewdeck.json"]);

        // And it reads back to the same vault.
        let reopened = Vault::open_at(dir.vault(), Arc::new(MemoryTokens::new()));
        assert_eq!(reopened.list_accounts(), vault.list_accounts());
        assert_eq!(reopened.settings(), vault.settings());
    }

    #[test]
    fn electron_tokens_move_into_the_token_store_and_out_of_the_file() {
        let dir = TempDir::new();
        let tokens = Arc::new(MemoryTokens::with_legacy_password(LEGACY_PASSWORD));
        let vault = open_fixture(&dir, tokens.clone());

        assert_eq!(
            vault.get_token(GITHUB_ID).ok().as_deref(),
            Some(LEGACY_TOKEN)
        );
        assert_eq!(
            tokens.snapshot(),
            HashMap::from([(GITHUB_ID.to_string(), LEGACY_TOKEN.to_string())])
        );
        // The one that would not open stays for the next launch, and until then the
        // account has no token to give.
        assert_eq!(
            read_json(&dir.vault())["tokens"],
            json!({ GITLAB_ID: STUCK_BLOB })
        );
        assert_eq!(
            vault.get_token(GITLAB_ID).map_err(|e| e.to_string()),
            Err(NO_TOKEN.to_string())
        );

        // The next launch tries again, and leaves what it still cannot open.
        let again = Vault::open_at(dir.vault(), tokens.clone());
        assert_eq!(
            again.get_token(GITHUB_ID).ok().as_deref(),
            Some(LEGACY_TOKEN)
        );
        assert_eq!(
            read_json(&dir.vault())["tokens"],
            json!({ GITLAB_ID: STUCK_BLOB })
        );
    }

    #[test]
    fn a_missing_vault_is_an_empty_one_and_nothing_is_written_until_something_changes() {
        let dir = TempDir::new();
        let path = dir.0.join("nested").join("reviewdeck.json");
        let vault = Vault::open_at(&path, Arc::new(MemoryTokens::new()));
        assert!(vault.list_accounts().is_empty());
        assert_eq!(vault.settings(), Settings::default());
        assert!(vault.load_deck_cache().is_empty());
        assert!(vault.load_windows_fired().is_empty());
        let (drafts, sets): (_, HashMap<String, DraftSet>) = vault.load_draft_state();
        assert!(drafts.is_empty() && sets.is_empty());
        assert!(!path.exists());

        // The first write creates the directory too.
        vault.mark_seen(&ids(&["a"])).expect("mark");
        assert_eq!(
            read_json(&path),
            json!({
                "version": 1,
                "accounts": [],
                "tokens": {},
                "settings": serde_json::to_value(Settings::default()).expect("settings"),
                "seen": ["a"],
                "drafts": [],
                "draftSets": {},
                "deck": { "version": 2, "items": {} },
                "windowsFired": {}
            })
        );
    }

    #[test]
    fn a_corrupt_vault_is_kept_aside_and_a_fresh_one_started() {
        for garbage in ["{ not json", "null", "[1, 2]"] {
            let dir = TempDir::new();
            fs::write(dir.vault(), garbage).expect("write");
            let vault = Vault::open_at(dir.vault(), Arc::new(MemoryTokens::new()));
            assert!(vault.list_accounts().is_empty());
            let entries = dir.entries();
            assert_eq!(entries.len(), 1, "{entries:?}");
            let kept = &entries[0];
            let stamp = kept
                .strip_prefix("reviewdeck.json.corrupt-")
                .expect("renamed with a timestamp");
            assert!(stamp.parse::<i64>().is_ok_and(|ms| ms > 0), "{kept}");
            assert_eq!(fs::read_to_string(dir.0.join(kept)).expect("kept"), garbage);
        }
    }

    #[test]
    fn malformed_entries_are_dropped_one_by_one_rather_than_losing_the_vault() {
        let dir = TempDir::new();
        let mut fixture = electron_fixture();
        fixture["accounts"]
            .as_array_mut()
            .expect("accounts")
            .push(json!({ "id": "half", "kind": "sourcehut" }));
        fixture["seen"]
            .as_array_mut()
            .expect("seen")
            .push(json!(42));
        fixture["tokens"]["odd"] = json!(17);
        fixture["deck"]["items"][GITHUB_ID]
            .as_array_mut()
            .expect("items")
            .push(json!({ "id": "broken" }));
        fixture["draftSets"]["weird"] = json!("nope");
        write_json(&dir.vault(), &fixture);

        let vault = Vault::open_at(dir.vault(), Arc::new(MemoryTokens::new()));
        assert_eq!(vault.list_accounts().len(), 2);
        assert_eq!(vault.load_deck_cache()[GITHUB_ID].len(), 1);
        let (_, sets): (_, HashMap<String, DraftSet>) = vault.load_draft_state();
        assert_eq!(sets.len(), 1);
        vault.mark_seen(&[]).expect("nothing to do");
        assert_eq!(
            vault.mark_seen(&ids(&["new"])).expect("mark"),
            ids(&["new"])
        );
        assert_eq!(
            read_json(&dir.vault())["seen"],
            json!([
                format!("{GITHUB_ID}:acme/api:411"),
                format!("{GITHUB_ID}:acme/api:412"),
                "new"
            ])
        );
    }

    #[test]
    fn add_account_files_it_under_a_fresh_id_with_its_token() {
        let dir = TempDir::new();
        let tokens = Arc::new(MemoryTokens::new());
        let vault = Vault::open_at(dir.vault(), tokens.clone());

        let created = vault
            .add_account(new_account("Codeberg"), "forgejo-token")
            .expect("add");
        let parsed = uuid::Uuid::parse_str(&created.id).expect("a uuid");
        assert_eq!(parsed.get_version_num(), 4);
        assert!(crate::time::parse_iso(&created.added_at).is_some());
        assert_eq!(created.added_at.len(), "2026-08-01T10:00:00.000Z".len());
        assert_eq!(created.label, "Codeberg");

        assert_eq!(vault.list_accounts(), vec![created.clone()]);
        assert_eq!(
            vault.get_token(&created.id).ok().as_deref(),
            Some("forgejo-token")
        );
        let written = read_json(&dir.vault());
        assert_eq!(written["accounts"][0]["id"], json!(created.id));
        assert_eq!(written["tokens"], json!({}), "the token is not in the file");
        assert!(
            !fs::read_to_string(dir.vault())
                .expect("read")
                .contains("forgejo-token")
        );

        let second = vault.add_account(new_account("Other"), "t2").expect("add");
        assert_ne!(second.id, created.id);
    }

    #[test]
    fn update_account_patches_everything_but_the_id() {
        let dir = TempDir::new();
        let vault = open_fixture(&dir, Arc::new(MemoryTokens::new()));
        vault
            .update_account(GITHUB_ID, |account| {
                account.label = "Personal GitHub".into();
                account.agent_command = Some("claude-personal".into());
                account.id = "hijacked".into();
            })
            .expect("update");
        let account = vault.get_account(GITHUB_ID).expect("still there");
        assert_eq!(account.label, "Personal GitHub");
        assert_eq!(account.agent_command.as_deref(), Some("claude-personal"));
        assert_eq!(
            read_json(&dir.vault())["accounts"][0]["label"],
            json!("Personal GitHub")
        );

        // An unknown id is nothing to do, and nothing is written.
        let before = fs::read_to_string(dir.vault()).expect("read");
        vault
            .update_account("nope", |account| account.label = "x".into())
            .expect("no-op");
        assert_eq!(fs::read_to_string(dir.vault()).expect("read"), before);
    }

    #[test]
    fn remove_account_takes_its_token_and_its_cached_reviews_with_it() {
        let dir = TempDir::new();
        let tokens = Arc::new(MemoryTokens::with_legacy_password(LEGACY_PASSWORD));
        let vault = open_fixture(&dir, tokens.clone());

        vault.remove_account(GITHUB_ID).expect("remove");
        assert_eq!(vault.get_account(GITHUB_ID), None);
        assert!(tokens.snapshot().is_empty());
        assert!(vault.load_deck_cache().is_empty());
        let written = read_json(&dir.vault());
        assert_eq!(written["accounts"].as_array().map(Vec::len), Some(1));
        assert!(written["deck"]["items"].get(GITHUB_ID).is_none());

        // A stuck legacy blob goes too.
        vault.remove_account(GITLAB_ID).expect("remove");
        assert_eq!(read_json(&dir.vault())["tokens"], json!({}));
        assert!(vault.list_accounts().is_empty());
    }

    #[test]
    fn tokens_are_only_set_for_accounts_that_exist() {
        let dir = TempDir::new();
        let tokens = Arc::new(MemoryTokens::new());
        let vault = open_fixture(&dir, tokens.clone());

        assert_eq!(
            vault.set_token("nope", "t").map_err(|e| e.to_string()),
            Err("No such account.".to_string())
        );
        assert_eq!(
            vault.get_token("nope").map_err(|e| e.to_string()),
            Err(NO_TOKEN.to_string())
        );

        // Setting a token replaces the stuck blob, so the next launch cannot migrate
        // the stale one over it.
        vault.set_token(GITLAB_ID, "fresh").expect("set");
        assert_eq!(vault.get_token(GITLAB_ID).ok().as_deref(), Some("fresh"));
        assert_eq!(
            read_json(&dir.vault())["tokens"],
            json!({ GITHUB_ID: LEGACY_BLOB })
        );
    }

    #[test]
    fn an_empty_token_is_no_token() {
        let dir = TempDir::new();
        let tokens = Arc::new(MemoryTokens::new());
        let vault = Vault::open_at(dir.vault(), tokens.clone());
        let created = vault.add_account(new_account("x"), "").expect("add");
        assert_eq!(
            vault.get_token(&created.id).map_err(|e| e.to_string()),
            Err(NO_TOKEN.to_string())
        );
    }

    #[test]
    fn draft_state_round_trips_and_is_replaced_wholesale() {
        let dir = TempDir::new();
        let vault = open_fixture(&dir, Arc::new(MemoryTokens::new()));
        let (mut drafts, mut sets): (_, HashMap<String, DraftSet>) = vault.load_draft_state();
        drafts[0].body = "Edited".into();
        sets.clear();
        sets.insert(
            "other".into(),
            DraftSet {
                baseline: MyReviewState::ChangesRequested,
                diverged: true,
            },
        );
        vault.persist_draft_state(&drafts, &sets).expect("persist");

        let written = read_json(&dir.vault());
        assert_eq!(written["drafts"][0]["body"], json!("Edited"));
        assert_eq!(
            written["draftSets"],
            json!({ "other": { "baseline": "changes_requested", "diverged": true } })
        );
        let reopened = Vault::open_at(dir.vault(), Arc::new(MemoryTokens::new()));
        let (again, again_sets): (_, BTreeMap<String, DraftSet>) = reopened.load_draft_state();
        assert_eq!(again, drafts);
        assert_eq!(again_sets.into_iter().collect::<HashMap<_, _>>(), sets);
    }

    #[test]
    fn the_deck_cache_is_replaced_wholesale_and_read_back_for_connected_accounts_only() {
        let dir = TempDir::new();
        let vault = open_fixture(&dir, Arc::new(MemoryTokens::new()));
        vault
            .persist_deck_cache([
                (
                    GITLAB_ID.to_string(),
                    vec![item(GITLAB_ID, 1), item(GITLAB_ID, 2)],
                ),
                ("stranger".to_string(), vec![item("stranger", 3)]),
            ])
            .expect("persist");

        let deck = vault.load_deck_cache();
        assert_eq!(deck.len(), 1);
        assert_eq!(deck[GITLAB_ID].len(), 2);
        let written = read_json(&dir.vault());
        assert_eq!(written["deck"]["version"], json!(2));
        assert!(
            written["deck"]["items"].get(GITHUB_ID).is_none(),
            "removals are removals"
        );
        assert_eq!(
            written["deck"]["items"][GITLAB_ID][1],
            review_item(GITLAB_ID, 2)
        );
    }

    #[test]
    fn a_deck_from_another_build_is_discarded_whole() {
        for deck in [
            json!({ "version": 1, "items": { GITHUB_ID: [review_item(GITHUB_ID, 1)] } }),
            json!({ "items": { GITHUB_ID: [review_item(GITHUB_ID, 1)] } }),
            json!({ "version": 2, "items": [] }),
            json!([1, 2]),
            json!(null),
        ] {
            let dir = TempDir::new();
            let mut fixture = electron_fixture();
            fixture["deck"] = deck.clone();
            write_json(&dir.vault(), &fixture);
            let vault = Vault::open_at(dir.vault(), Arc::new(MemoryTokens::new()));
            assert!(vault.load_deck_cache().is_empty(), "{deck}");
        }
        // An account entry that is not a list only takes itself.
        let dir = TempDir::new();
        let mut fixture = electron_fixture();
        fixture["deck"]["items"][GITLAB_ID] = json!("nope");
        write_json(&dir.vault(), &fixture);
        let vault = Vault::open_at(dir.vault(), Arc::new(MemoryTokens::new()));
        assert_eq!(vault.load_deck_cache().len(), 1);
    }

    #[test]
    fn windows_fired_are_recorded_against_every_window_that_raised_the_roll_up() {
        let dir = TempDir::new();
        let vault = open_fixture(&dir, Arc::new(MemoryTokens::new()));
        let before = fs::read_to_string(dir.vault()).expect("read");
        vault
            .record_windows_fired(&[], "2026-08-04")
            .expect("no-op");
        assert_eq!(fs::read_to_string(dir.vault()).expect("read"), before);

        vault
            .record_windows_fired(&ids(&["w1", "w2"]), "2026-08-04")
            .expect("record");
        assert_eq!(
            vault.load_windows_fired(),
            HashMap::from([
                ("w1".to_string(), "2026-08-04".to_string()),
                ("w2".to_string(), "2026-08-04".to_string()),
            ])
        );
        assert_eq!(
            read_json(&dir.vault())["windowsFired"],
            json!({ "w1": "2026-08-04", "w2": "2026-08-04" })
        );
    }

    #[test]
    fn save_settings_lays_the_patch_over_what_is_there() {
        let dir = TempDir::new();
        let vault = open_fixture(&dir, Arc::new(MemoryTokens::new()));
        let saved = vault
            .save_settings(|settings| settings.hide_drafts = false)
            .expect("save");
        assert!(!saved.hide_drafts);
        assert_eq!(saved.poll_interval, 300, "the rest untouched");
        assert_eq!(vault.settings(), saved);
        assert_eq!(
            read_json(&dir.vault())["settings"]["hideDrafts"],
            json!(false)
        );
    }

    #[test]
    fn mark_seen_returns_only_what_is_new_and_keeps_the_list_bounded() {
        let dir = TempDir::new();
        let vault = Vault::open_at(dir.vault(), Arc::new(MemoryTokens::new()));
        assert_eq!(
            vault.mark_seen(&ids(&["a", "b"])).expect("mark"),
            ids(&["a", "b"])
        );
        assert_eq!(
            vault.mark_seen(&ids(&["b", "c"])).expect("mark"),
            ids(&["c"])
        );
        assert!(vault.mark_seen(&ids(&["a", "c"])).expect("mark").is_empty());

        let many: Vec<String> = (0..2100).map(|n| format!("id-{n}")).collect();
        assert_eq!(vault.mark_seen(&many).expect("mark").len(), 2100);
        let seen = read_json(&dir.vault())["seen"].clone();
        let seen = seen.as_array().expect("seen");
        assert_eq!(seen.len(), 2000);
        assert_eq!(seen[0], json!("id-100"), "the oldest fall off the front");
        assert_eq!(seen[1999], json!("id-2099"));
        // What fell off counts as new again, as it would in the TypeScript.
        assert_eq!(vault.mark_seen(&ids(&["a"])).expect("mark"), ids(&["a"]));
    }

    #[test]
    fn the_vault_lives_where_the_electron_app_kept_it() {
        let path = vault_path().expect("HOME is set in tests");
        assert!(path.ends_with("Library/Application Support/reviewdeck/reviewdeck.json"));
        assert_eq!(path.parent(), data_dir().ok().as_deref());
    }
}
