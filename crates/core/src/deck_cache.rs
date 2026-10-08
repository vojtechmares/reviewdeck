//! The deck as it goes to disk, so a relaunch has reviews on screen before the
//! first fan-out returns. A port of src/shared/deck-cache.ts.
//!
//! Only what the sidebar and the menu bar draw. A [`ReviewItem`] is already the
//! list-level view of a pull request - the description, diff and threads that make
//! up its detail are fetched when it is opened and never written down, so opening a
//! cached review still costs exactly the round-trip it always did.
//!
//! The cache is a hint, never truth: a completed sync replaces it wholesale, and
//! anything unrecognised here is dropped rather than allowed to reach the UI
//! half-formed. Starting cold is a slower launch; rendering a card built from a
//! shape nobody wrote is a crash.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use crate::model::ReviewItem;

/// Bumped whenever a [`ReviewItem`] gains a field the cards rely on, so a deck
/// written by an older build is discarded rather than drawn with holes in it.
pub const DECK_CACHE_VERSION: u32 = 2;

/// The vault's `deck`: `{ "version": 2, "items": { "<accountId>": [ReviewItem...] } }`.
///
/// `Default` is [`empty_deck_cache`]. Deserialising goes through [`read_deck_cache`],
/// so a stale or malformed deck in the vault loads as an empty one (or as the items
/// that survive) instead of failing the whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeckCache {
    pub version: u32,
    /// accountId -> the items that account returned on the last completed sync.
    pub items: BTreeMap<String, Vec<ReviewItem>>,
}

impl Default for DeckCache {
    fn default() -> Self {
        empty_deck_cache()
    }
}

impl<'de> Deserialize<'de> for DeckCache {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = Value::deserialize(deserializer)?;
        Ok(read_deck_cache(Some(&stored)))
    }
}

pub fn empty_deck_cache() -> DeckCache {
    DeckCache {
        version: DECK_CACHE_VERSION,
        items: BTreeMap::new(),
    }
}

/// A cache read back off the vault, or an empty one when there is nothing usable.
/// `None` is a vault with no `deck` at all.
///
/// An envelope that is not what this build writes - a version from another build, a
/// shape that is not an object - takes the whole cache with it. A single item that
/// does not survive its own check only takes itself, because the rest of the deck is
/// still worth showing and the sync on its way replaces all of it anyway.
pub fn read_deck_cache(stored: Option<&Value>) -> DeckCache {
    let Some(Value::Object(stored)) = stored else {
        return empty_deck_cache();
    };
    // A number equal to the version, however it is spelled (`2` or `2.0`), as `===`
    // compares numbers in JavaScript; anything else is another build's.
    let version = stored.get("version").and_then(Value::as_f64);
    if version != Some(f64::from(DECK_CACHE_VERSION)) {
        return empty_deck_cache();
    }
    let Some(Value::Object(stored_items)) = stored.get("items") else {
        return empty_deck_cache();
    };

    let items = stored_items
        .iter()
        .filter_map(|(account_id, cached)| {
            let Value::Array(cached) = cached else {
                return None;
            };
            let kept = cached.iter().filter_map(read_review_item).collect();
            Some((account_id.clone(), kept))
        })
        .collect();
    DeckCache {
        version: DECK_CACHE_VERSION,
        items,
    }
}

/// An item that passes the shape check the TypeScript makes and then deserialises
/// into a [`ReviewItem`]. The second step is stricter than the first - an unknown
/// provider or review state, a negative or fractional count - and is just as much a
/// reason to drop the item: a card cannot be drawn from what does not parse.
fn read_review_item(value: &Value) -> Option<ReviewItem> {
    if !is_review_item(value) {
        return None;
    }
    ReviewItem::deserialize(value).ok()
}

fn is_string(record: &Map<String, Value>, key: &str) -> bool {
    matches!(record.get(key), Some(Value::String(_)))
}

fn is_number(record: &Map<String, Value>, key: &str) -> bool {
    matches!(record.get(key), Some(Value::Number(_)))
}

fn is_user(value: Option<&Value>) -> bool {
    let Some(Value::Object(record)) = value else {
        return false;
    };
    is_string(record, "name") && is_string(record, "avatarUrl")
}

/// The counts a check pill draws; the runs behind it may legitimately be empty.
fn is_check_summary(value: Option<&Value>) -> bool {
    let Some(Value::Object(record)) = value else {
        return false;
    };
    is_string(record, "status")
        && is_number(record, "passed")
        && is_number(record, "failed")
        && is_number(record, "running")
        && is_number(record, "total")
        && matches!(record.get("runs"), Some(Value::Array(_)))
}

fn is_approval_summary(value: Option<&Value>) -> bool {
    let Some(Value::Object(record)) = value else {
        return false;
    };
    // `required` absent or a number; `null` is neither.
    is_number(record, "given")
        && is_string(record, "outcome")
        && (record.get("required").is_none() || is_number(record, "required"))
}

fn is_review_item(value: &Value) -> bool {
    let Value::Object(record) = value else {
        return false;
    };
    [
        "id",
        "accountId",
        "provider",
        "repoKey",
        "repo",
        "title",
        "url",
        "createdAt",
        "updatedAt",
        "sourceBranch",
        "targetBranch",
        "myReviewState",
    ]
    .iter()
    .all(|key| is_string(record, key))
        && is_number(record, "number")
        && matches!(record.get("draft"), Some(Value::Bool(_)))
        && matches!(record.get("labels"), Some(Value::Array(_)))
        && is_user(record.get("author"))
        && is_approval_summary(record.get("approvals"))
        && is_check_summary(record.get("checks"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn review(number: u64) -> Value {
        json!({
            "id": format!("acc:acme/design-tokens:{number}"),
            "accountId": "acc",
            "provider": "github",
            "repoKey": "acme/design-tokens",
            "repo": "acme/design-tokens",
            "number": number,
            "title": format!("Pull request {number}"),
            "url": format!("https://example.test/pull/{number}"),
            "author": { "name": "lpeters", "avatarUrl": "" },
            "createdAt": "2026-08-01T10:00:00Z",
            "updatedAt": "2026-08-01T10:00:00Z",
            "draft": false,
            "sourceBranch": "topic",
            "targetBranch": "main",
            "labels": [],
            "myReviewState": "pending",
            "approvals": { "given": 0, "outcome": "none_required" },
            "checks": { "status": "unknown", "passed": 0, "failed": 0, "running": 0, "total": 0, "runs": [] },
        })
    }

    /// `{ ...review(n), key: value }`, where a `null` value stands for `undefined`.
    fn patched(number: u64, key: &str, value: Value) -> Value {
        let mut item = review(number);
        if let Value::Object(record) = &mut item {
            if value.is_null() {
                record.remove(key);
            } else {
                record.insert(key.into(), value);
            }
        }
        item
    }

    fn written() -> Value {
        json!({ "version": DECK_CACHE_VERSION, "items": { "acc": [review(1), review(2)] } })
    }

    fn with_version(version: Value) -> Value {
        let mut cache = written();
        if let Value::Object(record) = &mut cache {
            if version.is_null() {
                record.remove("version");
            } else {
                record.insert("version".into(), version);
            }
        }
        cache
    }

    fn numbers(items: &[ReviewItem]) -> Vec<u64> {
        items.iter().map(|item| item.number).collect()
    }

    fn keys(cache: &DeckCache) -> Vec<&str> {
        cache.items.keys().map(String::as_str).collect()
    }

    #[test]
    fn read_deck_cache_reads_back_what_was_written_keyed_by_account() {
        let cache = read_deck_cache(Some(&written()));
        assert_eq!(cache.version, DECK_CACHE_VERSION);
        assert_eq!(keys(&cache), ["acc"]);
        let expected: Vec<ReviewItem> = [review(1), review(2)]
            .iter()
            .map(|item| ReviewItem::deserialize(item).expect("a valid item"))
            .collect();
        assert_eq!(cache.items["acc"], expected);
    }

    #[test]
    fn read_deck_cache_starts_empty_rather_than_throwing_on_a_vault_with_no_deck() {
        assert!(read_deck_cache(None).items.is_empty());
        assert!(read_deck_cache(Some(&Value::Null)).items.is_empty());
    }

    #[test]
    fn read_deck_cache_discards_a_cache_written_by_another_build() {
        assert!(
            read_deck_cache(Some(&with_version(json!(DECK_CACHE_VERSION + 1))))
                .items
                .is_empty()
        );
        assert!(
            read_deck_cache(Some(&with_version(Value::Null)))
                .items
                .is_empty()
        );
        assert!(
            read_deck_cache(Some(&with_version(json!("1"))))
                .items
                .is_empty()
        );
        // Not a version but its string spelling, either.
        assert!(
            read_deck_cache(Some(&with_version(json!("2"))))
                .items
                .is_empty()
        );
    }

    #[test]
    fn read_deck_cache_accepts_the_version_however_the_number_is_spelled() {
        assert_eq!(
            keys(&read_deck_cache(Some(&with_version(json!(2.0))))),
            ["acc"]
        );
    }

    #[test]
    fn read_deck_cache_discards_a_cache_whose_shape_is_not_what_it_writes() {
        assert!(read_deck_cache(Some(&json!("reviews"))).items.is_empty());
        assert!(read_deck_cache(Some(&json!([review(1)]))).items.is_empty());
        assert!(
            read_deck_cache(Some(&json!({ "version": DECK_CACHE_VERSION, "items": [] })))
                .items
                .is_empty()
        );
        assert!(
            read_deck_cache(Some(&json!({ "version": DECK_CACHE_VERSION })))
                .items
                .is_empty()
        );
    }

    #[test]
    fn read_deck_cache_drops_an_account_whose_entry_is_not_a_list_of_reviews() {
        let cache = read_deck_cache(Some(&json!({
            "version": DECK_CACHE_VERSION,
            "items": { "acc": [review(1)], "broken": { "number": 7 } },
        })));
        assert_eq!(keys(&cache), ["acc"]);
    }

    #[test]
    fn read_deck_cache_drops_the_items_that_would_not_draw_and_keeps_the_rest() {
        let cache = read_deck_cache(Some(&json!({
            "version": DECK_CACHE_VERSION,
            "items": {
                "acc": [
                    review(1),
                    null,
                    "not an item",
                    patched(2, "author", Value::Null),
                    patched(3, "checks", json!({ "status": "passed" })),
                    patched(4, "labels", json!("ux")),
                    patched(5, "number", json!("5")),
                    review(6),
                ],
            },
        })));
        assert_eq!(numbers(&cache.items["acc"]), [1, 6]);
    }

    #[test]
    fn read_deck_cache_drops_what_passes_the_shape_check_but_does_not_parse() {
        // Shapes the TypeScript check lets through and a typed item cannot hold.
        let cache = read_deck_cache(Some(&json!({
            "version": DECK_CACHE_VERSION,
            "items": {
                "acc": [
                    patched(1, "provider", json!("sourcehut")),
                    patched(2, "number", json!(-2)),
                    patched(3, "approvals", json!({ "given": 0, "outcome": "none_required", "required": null })),
                    review(4),
                ],
            },
        })));
        assert_eq!(numbers(&cache.items["acc"]), [4]);
    }

    #[test]
    fn a_deck_cached_without_approvals_is_discarded_rather_than_drawn_with_holes() {
        // From test/approvals.test.ts.
        let mut satisfied = review(3);
        if let Value::Object(record) = &mut satisfied {
            record.insert(
                "approvals".into(),
                json!({ "given": 2, "required": 2, "outcome": "satisfied" }),
            );
            record.insert("id".into(), json!("3"));
        }
        let mut older = satisfied.clone();
        if let Value::Object(record) = &mut older {
            record.remove("approvals");
        }
        let mut short = review(2);
        if let Value::Object(record) = &mut short {
            record.insert(
                "approvals".into(),
                json!({ "given": 1, "required": 2, "outcome": "pending" }),
            );
            record.insert("id".into(), json!("2"));
        }
        let cache = read_deck_cache(Some(&json!({
            "version": DECK_CACHE_VERSION,
            "items": { "acc": [older, short] },
        })));
        let ids: Vec<&str> = cache.items["acc"]
            .iter()
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(ids, ["2"]);
    }

    #[test]
    fn the_cache_serialises_to_the_vault_shape_and_reads_back_through_serde() {
        let cache = read_deck_cache(Some(&written()));
        let value = serde_json::to_value(&cache).expect("serialisable");
        assert_eq!(value, written());

        let parsed: DeckCache = serde_json::from_value(value).expect("deserialisable");
        assert_eq!(parsed, cache);
        // A deck nobody can read loads as an empty one rather than failing the vault.
        let stale: DeckCache =
            serde_json::from_value(json!({ "version": 1, "items": {} })).expect("tolerant");
        assert_eq!(stale, empty_deck_cache());
        assert_eq!(DeckCache::default(), empty_deck_cache());
        assert_eq!(
            serde_json::to_value(empty_deck_cache()).expect("serialisable"),
            json!({ "version": 2, "items": {} })
        );
    }
}
