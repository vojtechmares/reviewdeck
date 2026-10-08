//! Development fixtures: a port of src/main/demo.ts.
//!
//! Enabled with `REVIEWDECK_DEMO=1` so the whole UI - deck, diff, checks,
//! conversation - can be exercised without handing a real token to a dev build.
//! The same fixtures drive the README screenshots, so their content is copied
//! verbatim.

use std::sync::LazyLock;

use crate::model::{
    Account, ApprovalOutcome, ApprovalSummary, CheckRun, CheckStatus, CheckSummary, CommentThread,
    DiffFile, DiffRefs, FileStatus, MyReviewState, ProviderKind, PullComment, PullDetail,
    ReviewItem, Side, User,
};
use crate::time::{format_iso, now_iso, now_ms};

pub fn demo_enabled() -> bool {
    std::env::var("REVIEWDECK_DEMO").is_ok_and(|value| value == "1")
}

fn demo_account(
    id: &str,
    kind: ProviderKind,
    label: &str,
    base_url: &str,
    web_url: &str,
    username: &str,
) -> Account {
    Account {
        id: id.into(),
        kind,
        label: label.into(),
        base_url: base_url.into(),
        web_url: web_url.into(),
        username: username.into(),
        display_name: "Vojtěch Mareš".into(),
        avatar_url: String::new(),
        added_at: now_iso(),
        agent_command: None,
    }
}

/// The demo accounts, dated the first time they are asked for (the TypeScript dated
/// them when the module loaded).
pub static DEMO_ACCOUNTS: LazyLock<Vec<Account>> = LazyLock::new(|| {
    vec![
        demo_account(
            "demo-github",
            ProviderKind::Github,
            "Work GitHub",
            "https://api.github.com",
            "https://github.com",
            "vojtechmares",
        ),
        demo_account(
            "demo-gitlab",
            ProviderKind::Gitlab,
            "Client GitLab",
            "https://gitlab.acme.dev/api/v4",
            "https://gitlab.acme.dev",
            "vmares",
        ),
        demo_account(
            "demo-forgejo",
            ProviderKind::Forgejo,
            "Codeberg",
            "https://codeberg.org/api/v1",
            "https://codeberg.org",
            "vmares",
        ),
    ]
});

fn minutes_ago(minutes: i64) -> String {
    format_iso(now_ms() - minutes * 60_000)
}

fn user(name: &str) -> User {
    User {
        name: name.into(),
        avatar_url: String::new(),
    }
}

fn run(id: &str, name: &str, status: CheckStatus, description: Option<&str>) -> CheckRun {
    CheckRun {
        id: id.into(),
        name: name.into(),
        status,
        url: None,
        description: description.map(Into::into),
    }
}

fn labels(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

/// The demo deck, dated the first time it is asked for (the TypeScript dated it
/// when the module loaded).
pub static DEMO_ITEMS: LazyLock<Vec<ReviewItem>> = LazyLock::new(|| {
    use CheckStatus::{Failed, Passed, Running, Unknown};
    vec![
        ReviewItem {
            id: "demo-github:acme/checkout-api:412".into(),
            account_id: "demo-github".into(),
            provider: ProviderKind::Github,
            repo_key: "acme/checkout-api".into(),
            repo: "acme/checkout-api".into(),
            number: 412,
            title: "Retry idempotent payment captures instead of failing the webhook".into(),
            url: "https://github.com/acme/checkout-api/pull/412".into(),
            author: user("hkramer"),
            created_at: minutes_ago(180),
            updated_at: minutes_ago(14),
            draft: false,
            source_branch: "fix/capture-retry".into(),
            target_branch: "main".into(),
            labels: labels(&["payments", "needs-review"]),
            my_review_state: MyReviewState::Pending,
            approvals: ApprovalSummary {
                given: 1,
                required: None,
                outcome: ApprovalOutcome::Pending,
            },
            checks: CheckSummary {
                status: Failed,
                passed: 4,
                failed: 1,
                running: 0,
                total: 5,
                runs: vec![
                    run("1", "build", Passed, Some("Compiled in 42s")),
                    run("2", "unit", Passed, Some("812 passed")),
                    run("3", "lint", Passed, None),
                    run(
                        "4",
                        "integration",
                        Failed,
                        Some("2 failing in capture_test.go"),
                    ),
                    run("5", "license/cla", Passed, None),
                ],
            },
            additions: Some(38),
            deletions: Some(12),
            changed_files: Some(2),
        },
        ReviewItem {
            id: "demo-gitlab:2841:77".into(),
            account_id: "demo-gitlab".into(),
            provider: ProviderKind::Gitlab,
            repo_key: "2841".into(),
            repo: "acme/platform/edge-router".into(),
            number: 77,
            title: "Bump Envoy to 1.31 and drop the deprecated ext_authz shim".into(),
            url: "https://gitlab.acme.dev/acme/platform/edge-router/-/merge_requests/77".into(),
            author: user("dnovak"),
            created_at: minutes_ago(1500),
            updated_at: minutes_ago(52),
            draft: false,
            source_branch: "chore/envoy-1.31".into(),
            target_branch: "main".into(),
            labels: labels(&["infra"]),
            my_review_state: MyReviewState::Pending,
            approvals: ApprovalSummary {
                given: 2,
                required: Some(2),
                outcome: ApprovalOutcome::Satisfied,
            },
            checks: CheckSummary {
                status: Running,
                passed: 3,
                failed: 0,
                running: 2,
                total: 5,
                runs: vec![
                    run("1", "lint:yaml", Passed, Some("lint")),
                    run("2", "build:image", Passed, Some("build")),
                    run("3", "test:unit", Passed, Some("test")),
                    run("4", "test:e2e", Running, Some("test")),
                    run("5", "deploy:staging", Running, Some("deploy")),
                ],
            },
            additions: Some(121),
            deletions: Some(340),
            changed_files: Some(9),
        },
        ReviewItem {
            id: "demo-forgejo:vmares/dotfiles:9".into(),
            account_id: "demo-forgejo".into(),
            provider: ProviderKind::Forgejo,
            repo_key: "vmares/dotfiles".into(),
            repo: "vmares/dotfiles".into(),
            number: 9,
            title: "Add a zsh widget for fuzzy-jumping into worktrees".into(),
            url: "https://codeberg.org/vmares/dotfiles/pulls/9".into(),
            author: user("sbergman"),
            created_at: minutes_ago(60 * 26),
            updated_at: minutes_ago(60 * 5),
            draft: true,
            source_branch: "feat/worktree-widget".into(),
            target_branch: "main".into(),
            labels: Vec::new(),
            my_review_state: MyReviewState::Commented,
            approvals: ApprovalSummary {
                given: 0,
                required: None,
                outcome: ApprovalOutcome::NoneRequired,
            },
            checks: CheckSummary {
                status: Unknown,
                passed: 0,
                failed: 0,
                running: 0,
                total: 0,
                runs: Vec::new(),
            },
            additions: Some(44),
            deletions: Some(3),
            changed_files: Some(1),
        },
        ReviewItem {
            id: "demo-github:acme/design-tokens:88".into(),
            account_id: "demo-github".into(),
            provider: ProviderKind::Github,
            repo_key: "acme/design-tokens".into(),
            repo: "acme/design-tokens".into(),
            number: 88,
            title: "Regenerate the dark palette from the new contrast targets".into(),
            url: "https://github.com/acme/design-tokens/pull/88".into(),
            author: user("lpeters"),
            created_at: minutes_ago(60 * 50),
            updated_at: minutes_ago(60 * 20),
            draft: false,
            source_branch: "design/contrast-pass".into(),
            target_branch: "main".into(),
            labels: labels(&["design-system"]),
            my_review_state: MyReviewState::Approved,
            approvals: ApprovalSummary {
                given: 1,
                required: None,
                outcome: ApprovalOutcome::Satisfied,
            },
            checks: CheckSummary {
                status: Passed,
                passed: 3,
                failed: 0,
                running: 0,
                total: 3,
                runs: vec![
                    run("1", "build", Passed, None),
                    run("2", "contrast-audit", Passed, Some("All pairs ≥ 4.5:1")),
                    run("3", "visual-diff", Passed, None),
                ],
            },
            additions: Some(210),
            deletions: Some(196),
            changed_files: Some(4),
        },
    ]
});

/// The diff every demo pull request opens onto.
pub const DEMO_DIFF: &str = r##"diff --git a/internal/payments/capture.go b/internal/payments/capture.go
index 8a1f2c3..b7d9e04 100644
--- a/internal/payments/capture.go
+++ b/internal/payments/capture.go
@@ -14,9 +14,11 @@ import (
 	"context"
 	"errors"
 	"fmt"
+	"time"
 
 	"github.com/acme/checkout-api/internal/gateway"
 	"github.com/acme/checkout-api/internal/telemetry"
+	"github.com/cenkalti/backoff/v4"
 )
 
 // ErrAlreadyCaptured is returned when the gateway has seen this capture before.
@@ -38,18 +40,29 @@ func (s *Service) Capture(ctx context.Context, id string, amount Money) error {
 	if err != nil {
 		return fmt.Errorf("load intent %s: %w", id, err)
 	}
 
-	res, err := s.gateway.Capture(ctx, intent.GatewayID, amount)
-	if err != nil {
-		telemetry.CaptureFailed(ctx, id, err)
-		return err
-	}
+	// The gateway occasionally 502s under load. Captures are idempotent on the
+	// gateway side, so retrying is safe and much kinder than failing the webhook.
+	var res *gateway.CaptureResult
+	retry := backoff.NewExponentialBackOff()
+	retry.MaxElapsedTime = 20 * time.Second
+
+	err = backoff.Retry(func() error {
+		var attemptErr error
+		res, attemptErr = s.gateway.Capture(ctx, intent.GatewayID, amount)
+		if errors.Is(attemptErr, gateway.ErrPermanent) {
+			return backoff.Permanent(attemptErr)
+		}
+		return attemptErr
+	}, backoff.WithContext(retry, ctx))
+	if err != nil {
+		telemetry.CaptureFailed(ctx, id, err)
+		return err
+	}
 
 	if res.AlreadyCaptured {
 		return ErrAlreadyCaptured
 	}
 
-	return s.store.MarkCaptured(ctx, id, res.Reference)
+	return s.store.MarkCaptured(ctx, id, res.Reference)
 }
diff --git a/internal/payments/capture_test.go b/internal/payments/capture_test.go
index 2b4c5d6..9e8f7a1 100644
--- a/internal/payments/capture_test.go
+++ b/internal/payments/capture_test.go
@@ -61,6 +61,18 @@ func TestCaptureAlreadyCaptured(t *testing.T) {
 	}
 }
 
+func TestCaptureRetriesTransientGatewayErrors(t *testing.T) {
+	gw := &fakeGateway{failures: 2}
+	svc := newService(gw)
+
+	if err := svc.Capture(context.Background(), "pi_1", Money{Cents: 4200}); err != nil {
+		t.Fatalf("expected the capture to succeed after retries, got %v", err)
+	}
+	if gw.calls != 3 {
+		t.Fatalf("expected 3 gateway calls, got %d", gw.calls)
+	}
+}
+
 func TestCapturePropagatesPermanentErrors(t *testing.T) {
 	gw := &fakeGateway{permanent: true}
 	svc := newService(gw)
"##;

/// The files of [`DEMO_DIFF`], split the way `parseUnifiedDiff` splits them.
///
/// A minimal reading of git's format - enough for this fixture, which is a plain
/// two-file modification - so the fixture does not depend on the diff module:
/// the `---`/`+++` paths without their `a/`/`b/` prefixes, everything from the
/// first hunk to the next file header as the patch, and the `+`/`-` lines counted.
fn demo_files() -> Vec<DiffFile> {
    let lines: Vec<&str> = DEMO_DIFF.split('\n').collect();
    let mut files = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].starts_with("diff ") {
            i += 1;
            continue;
        }
        i += 1;
        let mut old_path = "";
        let mut new_path = "";
        while i < lines.len() && !lines[i].starts_with("@@") && !lines[i].starts_with("diff ") {
            let line = lines[i];
            let strip = |path: &'static str| {
                path.trim()
                    .strip_prefix("a/")
                    .or_else(|| path.trim().strip_prefix("b/"))
                    .unwrap_or(path.trim())
            };
            if let Some(path) = line.strip_prefix("--- ") {
                old_path = strip(path);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                new_path = strip(path);
            }
            i += 1;
        }
        let start = i;
        while i < lines.len() && !lines[i].starts_with("diff ") {
            i += 1;
        }
        let body = &lines[start..i];
        let additions = body
            .iter()
            .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
            .count() as u32;
        let deletions = body
            .iter()
            .filter(|line| line.starts_with('-') && !line.starts_with("---"))
            .count() as u32;
        files.push(DiffFile {
            path: new_path.to_string(),
            old_path: old_path.to_string(),
            status: FileStatus::Modified,
            additions,
            deletions,
            patch: Some(body.join("\n")),
            binary: false,
        });
    }
    files
}

fn comment(id: &str, author: &str, body: &str, minutes: i64) -> PullComment {
    PullComment {
        id: id.into(),
        author: user(author),
        body: body.into(),
        created_at: minutes_ago(minutes),
    }
}

pub fn demo_detail(item: &ReviewItem) -> PullDetail {
    // Every host has real threads now; only Forgejo cannot resolve one.
    let threaded = true;
    let resolvable = item.provider != ProviderKind::Forgejo;
    PullDetail {
        item: item.clone(),
        description: concat!(
            "The payments webhook has been failing about 40 times a day because the gateway ",
            "502s under load. Captures are idempotent on their side, so this retries transient ",
            "failures with a capped exponential backoff and only gives up on a permanent error.\n\n",
            "Closes ACME-2214.",
        )
        .into(),
        files: demo_files(),
        // The fixture mirrors the capability model: on a host without real threads the
        // same conversation arrives as threads with no affordances.
        threads: vec![
            CommentThread {
                id: "t1".into(),
                resolved: false,
                outdated: false,
                can_reply: threaded,
                can_resolve: resolvable,
                path: None,
                line: None,
                start_line: None,
                side: None,
                comments: vec![
                    comment(
                        "c1",
                        "mnovotna",
                        "Nice - can we get the max elapsed time into config rather than hard-coding 20s?",
                        40,
                    ),
                    comment(
                        "c2",
                        "hkramer",
                        "Good call, will do in a follow-up so this can ship today.",
                        25,
                    ),
                ],
            },
            CommentThread {
                id: "t2".into(),
                resolved: resolvable,
                outdated: false,
                can_reply: threaded,
                can_resolve: resolvable,
                path: None,
                line: None,
                start_line: None,
                side: None,
                comments: vec![comment(
                    "c3",
                    "hkramer",
                    "Rebased onto main - the flaky integration test was unrelated.",
                    30,
                )],
            },
            CommentThread {
                id: "t3".into(),
                resolved: false,
                outdated: false,
                can_reply: threaded,
                can_resolve: resolvable,
                path: Some("internal/payments/capture.go".into()),
                line: Some(55),
                // Covers the whole branch, so the demo shows a thread over several lines.
                start_line: Some(53),
                side: Some(Side::New),
                comments: vec![comment(
                    "c4",
                    "mnovotna",
                    "backoff.Permanent already unwraps, so this branch reads a little redundant - but harmless.",
                    20,
                )],
            },
        ],
        refs: DiffRefs {
            base_sha: Some("a".repeat(40)),
            start_sha: Some("a".repeat(40)),
            head_sha: Some("b".repeat(40)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::parse_iso;

    #[test]
    fn the_demo_diff_splits_into_the_files_the_typescript_parser_finds() {
        // Expected values from running src/shared/diff.ts parseUnifiedDiff on the
        // TypeScript fixture.
        let files = demo_files();
        assert_eq!(files.len(), 2);

        let capture = &files[0];
        assert_eq!(capture.path, "internal/payments/capture.go");
        assert_eq!(capture.old_path, "internal/payments/capture.go");
        assert_eq!(capture.status, FileStatus::Modified);
        assert_eq!((capture.additions, capture.deletions), (21, 6));
        assert!(!capture.binary);
        let patch = capture.patch.as_deref().unwrap_or_default();
        assert_eq!(patch.len(), 1428);
        assert!(patch.starts_with("@@ -14,9 +14,11 @@ import (\n \t\"context\"\n"));
        assert!(patch.ends_with("red(ctx, id, res.Reference)\n }"));

        let test = &files[1];
        assert_eq!(test.path, "internal/payments/capture_test.go");
        assert_eq!((test.additions, test.deletions), (12, 0));
        let patch = test.patch.as_deref().unwrap_or_default();
        assert_eq!(patch.len(), 574);
        assert!(patch.starts_with("@@ -61,6 +61,18 @@ func TestCaptureAlrea"));
        assert!(patch.ends_with("true}\n \tsvc := newService(gw)\n"));
    }

    #[test]
    fn the_fixtures_are_the_ones_the_screenshots_were_taken_of() {
        let accounts: Vec<&str> = DEMO_ACCOUNTS.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(accounts, ["demo-github", "demo-gitlab", "demo-forgejo"]);
        assert!(
            DEMO_ACCOUNTS
                .iter()
                .all(|a| parse_iso(&a.added_at).is_some())
        );

        let items: Vec<&str> = DEMO_ITEMS.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(
            items,
            [
                "demo-github:acme/checkout-api:412",
                "demo-gitlab:2841:77",
                "demo-forgejo:vmares/dotfiles:9",
                "demo-github:acme/design-tokens:88",
            ]
        );
        // Every item belongs to a demo account and carries its provider.
        for item in DEMO_ITEMS.iter() {
            let account = DEMO_ACCOUNTS
                .iter()
                .find(|a| a.id == item.account_id)
                .expect("a demo account");
            assert_eq!(account.kind, item.provider);
            assert!(parse_iso(&item.created_at) < parse_iso(&item.updated_at));
            let summary = crate::model::summarise_checks(item.checks.runs.clone());
            assert_eq!(summary, item.checks, "{} counts add up", item.id);
        }
    }

    #[test]
    fn the_detail_mirrors_the_capability_model() {
        let github = demo_detail(&DEMO_ITEMS[0]);
        assert_eq!(github.item, DEMO_ITEMS[0]);
        assert_eq!(github.files.len(), 2);
        assert!(github.threads.iter().all(|t| t.can_reply && t.can_resolve));
        assert!(github.threads[1].resolved);
        assert_eq!(github.threads[2].start_line, Some(53));
        assert_eq!(
            github.refs.head_sha.as_deref(),
            Some("b".repeat(40).as_str())
        );
        assert!(github.description.ends_with("\n\nCloses ACME-2214."));

        // Forgejo can reply but not resolve, so nothing there reads as resolved.
        let forgejo = demo_detail(&DEMO_ITEMS[2]);
        assert!(
            forgejo
                .threads
                .iter()
                .all(|t| t.can_reply && !t.can_resolve)
        );
        assert!(forgejo.threads.iter().all(|t| !t.resolved));
    }

    #[test]
    fn demo_mode_is_opt_in() {
        // Only the exact value turns it on; the test runner does not set it.
        if std::env::var_os("REVIEWDECK_DEMO").is_none() {
            assert!(!demo_enabled());
        }
    }
}
