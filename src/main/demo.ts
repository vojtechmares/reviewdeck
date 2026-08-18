/**
 * Development fixtures. Enabled with `REVIEWDECK_DEMO=1` so the whole UI -
 * deck, diff, checks, conversation - can be exercised without handing a real
 * token to a dev build. Never reachable in a packaged app.
 */

import type { Account, PullDetail, ReviewItem } from '@shared/types.ts'
import { parseUnifiedDiff } from '@shared/diff.ts'

export function demoEnabled(): boolean {
  return process.env['REVIEWDECK_DEMO'] === '1'
}

export const DEMO_ACCOUNTS: Account[] = [
  {
    id: 'demo-github',
    kind: 'github',
    label: 'Work GitHub',
    baseUrl: 'https://api.github.com',
    webUrl: 'https://github.com',
    username: 'vojtechmares',
    displayName: 'Vojtěch Mareš',
    avatarUrl: '',
    addedAt: new Date().toISOString(),
  },
  {
    id: 'demo-gitlab',
    kind: 'gitlab',
    label: 'Client GitLab',
    baseUrl: 'https://gitlab.acme.dev/api/v4',
    webUrl: 'https://gitlab.acme.dev',
    username: 'vmares',
    displayName: 'Vojtěch Mareš',
    avatarUrl: '',
    addedAt: new Date().toISOString(),
  },
  {
    id: 'demo-forgejo',
    kind: 'forgejo',
    label: 'Codeberg',
    baseUrl: 'https://codeberg.org/api/v1',
    webUrl: 'https://codeberg.org',
    username: 'vmares',
    displayName: 'Vojtěch Mareš',
    avatarUrl: '',
    addedAt: new Date().toISOString(),
  },
]

const minutesAgo = (minutes: number): string =>
  new Date(Date.now() - minutes * 60_000).toISOString()

export const DEMO_ITEMS: ReviewItem[] = [
  {
    id: 'demo-github:acme/checkout-api:412',
    accountId: 'demo-github',
    provider: 'github',
    repoKey: 'acme/checkout-api',
    repo: 'acme/checkout-api',
    number: 412,
    title: 'Retry idempotent payment captures instead of failing the webhook',
    url: 'https://github.com/acme/checkout-api/pull/412',
    author: { name: 'hkramer', avatarUrl: '' },
    createdAt: minutesAgo(180),
    updatedAt: minutesAgo(14),
    draft: false,
    sourceBranch: 'fix/capture-retry',
    targetBranch: 'main',
    labels: ['payments', 'needs-review'],
    myReviewState: 'pending',
    checks: {
      status: 'failed',
      passed: 4,
      failed: 1,
      running: 0,
      total: 5,
      runs: [
        { id: '1', name: 'build', status: 'passed', description: 'Compiled in 42s' },
        { id: '2', name: 'unit', status: 'passed', description: '812 passed' },
        { id: '3', name: 'lint', status: 'passed' },
        { id: '4', name: 'integration', status: 'failed', description: '2 failing in capture_test.go' },
        { id: '5', name: 'license/cla', status: 'passed' },
      ],
    },
    additions: 38,
    deletions: 12,
    changedFiles: 2,
  },
  {
    id: 'demo-gitlab:2841:77',
    accountId: 'demo-gitlab',
    provider: 'gitlab',
    repoKey: '2841',
    repo: 'acme/platform/edge-router',
    number: 77,
    title: 'Bump Envoy to 1.31 and drop the deprecated ext_authz shim',
    url: 'https://gitlab.acme.dev/acme/platform/edge-router/-/merge_requests/77',
    author: { name: 'dnovak', avatarUrl: '' },
    createdAt: minutesAgo(1500),
    updatedAt: minutesAgo(52),
    draft: false,
    sourceBranch: 'chore/envoy-1.31',
    targetBranch: 'main',
    labels: ['infra'],
    myReviewState: 'pending',
    checks: {
      status: 'running',
      passed: 3,
      failed: 0,
      running: 2,
      total: 5,
      runs: [
        { id: '1', name: 'lint:yaml', status: 'passed', description: 'lint' },
        { id: '2', name: 'build:image', status: 'passed', description: 'build' },
        { id: '3', name: 'test:unit', status: 'passed', description: 'test' },
        { id: '4', name: 'test:e2e', status: 'running', description: 'test' },
        { id: '5', name: 'deploy:staging', status: 'running', description: 'deploy' },
      ],
    },
    additions: 121,
    deletions: 340,
    changedFiles: 9,
  },
  {
    id: 'demo-forgejo:vmares/dotfiles:9',
    accountId: 'demo-forgejo',
    provider: 'forgejo',
    repoKey: 'vmares/dotfiles',
    repo: 'vmares/dotfiles',
    number: 9,
    title: 'Add a zsh widget for fuzzy-jumping into worktrees',
    url: 'https://codeberg.org/vmares/dotfiles/pulls/9',
    author: { name: 'sbergman', avatarUrl: '' },
    createdAt: minutesAgo(60 * 26),
    updatedAt: minutesAgo(60 * 5),
    draft: true,
    sourceBranch: 'feat/worktree-widget',
    targetBranch: 'main',
    labels: [],
    myReviewState: 'commented',
    checks: { status: 'unknown', passed: 0, failed: 0, running: 0, total: 0, runs: [] },
    additions: 44,
    deletions: 3,
    changedFiles: 1,
  },
  {
    id: 'demo-github:acme/design-tokens:88',
    accountId: 'demo-github',
    provider: 'github',
    repoKey: 'acme/design-tokens',
    repo: 'acme/design-tokens',
    number: 88,
    title: 'Regenerate the dark palette from the new contrast targets',
    url: 'https://github.com/acme/design-tokens/pull/88',
    author: { name: 'lpeters', avatarUrl: '' },
    createdAt: minutesAgo(60 * 50),
    updatedAt: minutesAgo(60 * 20),
    draft: false,
    sourceBranch: 'design/contrast-pass',
    targetBranch: 'main',
    labels: ['design-system'],
    myReviewState: 'approved',
    checks: {
      status: 'passed',
      passed: 3,
      failed: 0,
      running: 0,
      total: 3,
      runs: [
        { id: '1', name: 'build', status: 'passed' },
        { id: '2', name: 'contrast-audit', status: 'passed', description: 'All pairs ≥ 4.5:1' },
        { id: '3', name: 'visual-diff', status: 'passed' },
      ],
    },
    additions: 210,
    deletions: 196,
    changedFiles: 4,
  },
]

const DEMO_DIFF = `diff --git a/internal/payments/capture.go b/internal/payments/capture.go
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
`

export function demoDetail(item: ReviewItem): PullDetail {
  const threaded = item.provider === 'gitlab'
  return {
    item,
    description:
      'The payments webhook has been failing about 40 times a day because the gateway ' +
      '502s under load. Captures are idempotent on their side, so this retries transient ' +
      'failures with a capped exponential backoff and only gives up on a permanent error.\n\n' +
      'Closes ACME-2214.',
    files: parseUnifiedDiff(DEMO_DIFF),
    // Only GitLab can reply and resolve so far, so the fixture mirrors that: on any
    // other host the same conversation arrives as threads with no affordances.
    threads: [
      {
        id: 't1',
        resolved: false,
        outdated: false,
        canReply: threaded,
        canResolve: threaded,
        comments: [
          {
            id: 'c1',
            author: { name: 'mnovotna', avatarUrl: '' },
            body: 'Nice - can we get the max elapsed time into config rather than hard-coding 20s?',
            createdAt: minutesAgo(40),
          },
          {
            id: 'c2',
            author: { name: 'hkramer', avatarUrl: '' },
            body: 'Good call, will do in a follow-up so this can ship today.',
            createdAt: minutesAgo(25),
          },
        ],
      },
      {
        id: 't2',
        resolved: threaded,
        outdated: false,
        canReply: threaded,
        canResolve: threaded,
        comments: [
          {
            id: 'c3',
            author: { name: 'hkramer', avatarUrl: '' },
            body: 'Rebased onto main - the flaky integration test was unrelated.',
            createdAt: minutesAgo(30),
          },
        ],
      },
      {
        id: 't3',
        resolved: false,
        outdated: false,
        canReply: threaded,
        canResolve: threaded,
        path: 'internal/payments/capture.go',
        line: 55,
        side: 'new',
        comments: [
          {
            id: 'c4',
            author: { name: 'mnovotna', avatarUrl: '' },
            body: 'backoff.Permanent already unwraps, so this branch reads a little redundant - but harmless.',
            createdAt: minutesAgo(20),
          },
        ],
      },
    ],
    refs: { baseSha: 'a'.repeat(40), startSha: 'a'.repeat(40), headSha: 'b'.repeat(40) },
  }
}
