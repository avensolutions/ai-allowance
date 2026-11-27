# Provider validation

Observed provider behaviour and limitations. All probes below were read-only:
no model prompt was submitted, no allowance was consumed and no reset credit
was redeemed. Validation date: 2026-11-16, on Windows 11 x64.

## Summary

| Provider | Interface | Live validation | Status |
| --- | --- | --- | --- |
| Claude | Agent SDK experimental usage method via Node helper | Yes (2026-11-16) | Working, explicitly experimental |
| Work/Codex | `codex app-server` JSON-RPC `account/rateLimits/read` | Yes (2026-11-16) | Working |
| Copilot | `gh api /copilot_internal/user` quota snapshots | Yes (2026-11-18) | Working, explicitly experimental |

## Claude (experimental)

- Package: `@anthropic-ai/claude-agent-sdk` 0.3.325 (latest at validation
  time), pinned in `helpers/claude-usage/package.json`.
- Method: `Query.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors: true })`,
  which issues a `get_usage` control request to the bundled CLI runtime.
- Authentication: the SDK session reuses the existing Claude Code sign-in
  (`%USERPROFILE%\.claude`). No cookies are read and no credential files are
  edited. If no sign-in exists, the helper reports `sign_in_required`.
- Proven: a streaming-input SDK session that never yields a prompt can call
  the usage method and receive fresh data. The probe returned
  `session.total_cost_usd: 0` and `model_usage: {}`, confirming no model call
  was made. Round trip took about 6 seconds (SDK process spawn dominates).
- Units: `utilization` and `percent` are used percentages 0-100 (not the
  0-1 fractions used by the Python SDK's rate_limit_event). `resets_at` is an
  ISO 8601 timestamp with offset. These units were confirmed independently
  for this method; they are not assumed from the status-line or rate-limit
  event interfaces, which are separate.
- Windows observed on the validated account (subscription_type `max`):
  - `rate_limits.five_hour`: null (absent window; consistent with the
    account's plan not exposing a five-hour window).
  - `rate_limits.seven_day`: overall weekly window with `utilization` and
    `resets_at`.
  - `rate_limits.model_scoped`: ABSENT in the live response even though the
    SDK type declares it. The same data appeared in `rate_limits.limits[]` as
    a `weekly_scoped` entry with `scope.model.display_name: "Fable"`.
    The adapter therefore prefers `model_scoped` and falls back to
    `limits[]` entries of kind `weekly_scoped`. The overall window falls back
    to kind `weekly_all` when `seven_day` is missing.
  - `rate_limits_available: false` means plan limits do not apply (API key,
    Bedrock, Vertex); the adapter reports this as unavailable.
- The response carries many undocumented experimental fields; parsing is
  tolerant and ignores unknown fields. Missing values are shown as
  unavailable, never 0% used or 100% left.
- Update latency: values come from the claude.ai usage endpoint at call time;
  no cache-staleness indicator is exposed. Collection time is recorded
  locally.
- Observed 2026-11-28 (bundled CLI 2.1.325): the runtime reads
  `/api/oauth/usage` with a 5 s timeout. When that read fails (timeout,
  network error, in-band error body, HTTP 429 or 403) the usage method still
  returns `rate_limits_available: true` but `rate_limits: null`. A 429 or 403
  is remembered for the rest of that process (Retry-After, else 300 s). The
  response does not say why, so the helper passes `debugFile` to a temporary
  file, reads the runtime's `fetchUtilization` lines, and removes the file
  before exit. A remembered rate limit is reported as `rate_limited` with
  `retry_after_secs`; any other failed read is retried once after 2 s in the
  same session. Earlier builds reported "usage response contained no rate
  limit data" and marked the last good values stale after a single failure.
- Runtime dependency: Node.js 23.6+ (tested with 24.22.1) plus
  `npm install` in `helpers/claude-usage`. The helper's lifetime is bounded
  (80 s internal watchdog, 90 s Rust-side timeout) and it writes diagnostics
  to stderr only.
- Note: the account validated reports plan `max`. Ultra accounts could not be
  validated here; the parser treats any additional windows as absent rather
  than inventing them.

## Work/Codex

- Binary: `codex-cli 0.169.0-alpha.11.1`, bundled inside the ChatGPT VS Code
  extension (`%USERPROFILE%\.vscode\extensions\openai.chatgpt-*\bin\windows-x86_64\codex.exe`).
  The adapter finds codex via config override, then PATH, then the newest
  extension bundle.
- Protocol: newline-delimited JSON-RPC over stdio per the app-server
  documentation: `initialize` request (id 0) with `clientInfo`, `initialized`
  notification, then `account/rateLimits/read` (id 1). The server rejects
  requests sent before the handshake.
- Authentication: the app-server reuses the existing ChatGPT sign-in
  (`%USERPROFILE%\.codex\auth.json`), managed entirely by codex itself.
- Proven: the read returned live data without starting a model turn.
  Observed response shape:
  - `rateLimitsByLimitId`: map keyed by `limitId`; one bucket (`codex`) on
    the validated account with `primary: { usedPercent: 80,
    windowDurationMins: 10080, resetsAt: 1794866591 }` and `secondary: null`.
  - `rateLimits`: identical legacy single-bucket view. The adapter uses it
    only as a fallback, never as an additional bucket.
  - `usedPercent` is 0-100 within the window; `windowDurationMins` is the
    window length (10080 = 7 days); `resetsAt` is a Unix timestamp in
    seconds.
  - `planType` (`prolite` observed), `accountId` (used as the account cache
    key), and `rateLimitResetCredits` (3 earned resets available; the adapter
    never calls `account/rateLimitResetCredit/consume`).
- Reset semantics: countdown is computed locally from `resetsAt`; the window
  restores on the provider side on its own schedule, so the app refreshes
  before announcing a reset.
- Update latency: values are served by the app-server at call time;
  `account/rateLimits/updated` notifications exist but the bounded one-shot
  process lifecycle does not keep a subscription open.
- Coverage: these are the Work/Codex buckets only. They do not describe other
  ChatGPT model, image, voice or research limits, and API-platform billing is
  not used as a substitute.

## GitHub Copilot

- Validated on 2026-11-18 with GitHub CLI 2.97.0 and an existing sign-in.
- Command:
  `gh api /copilot_internal/user --jq '{plan: .copilot_plan, reset: .quota_reset_date, quota: .quota_snapshots}'`.
  This internal endpoint is deliberately enabled at the maintainer's explicit
  request. It is not a documented public API contract.
- Authentication stays entirely with GitHub CLI. The app also calls
  `gh api /user --jq .id` to identify the account for cache isolation.
  Requests explicitly select github.com and do not require a repository
  working directory.
- Observed response: `plan`, a `YYYY-MM-DD` reset date, and snapshots keyed
  by `chat`, `completions` and `premium_interactions`. Synthetic fixtures
  retain this shape without storing account identifiers.
- Snapshot fields retained: `percent_remaining` (0-100 percentage left),
  `credits_used`, `entitlement`, `overage_count`, `timestamp_utc` and
  `quota_reset_at`. The reported percentage takes precedence over a derived
  credits ratio; the two can differ due to provider rounding.
- Explicitly unlimited chat and completion snapshots are not shown as finite
  allowances. All finite windows remain separate; premium interactions come
  first in the default compact view.
- Observed 2026-11-23: once the premium allowance is exhausted, its snapshot
  reports `has_quota: false` with `unlimited: false`, the positive
  `entitlement`, `percent_remaining: 0`, `credits_used` above the entitlement
  and negative `quota_remaining`/`remaining`. `has_quota` therefore means
  "allowance remaining", not "this snapshot has an allowance". A snapshot is
  skipped only when it is unlimited, or when `has_quota` is false and there is
  no positive entitlement. Earlier builds skipped the exhausted snapshot,
  reported "No finite Copilot allowance was reported" and kept showing the
  last good value.
- Reset convention: use a positive per-window Unix timestamp when present.
  A zero or absent timestamp falls back to the monthly date at 00:00 UTC.
  This is experimental interpretation, not a verified reset-boundary guarantee.
- The complete two-command lifecycle has a 30-second timeout, an output
  size limit, hidden consoles, no shell and kill-on-drop supervision.
  It does not submit a model prompt or modify the account.
- The provider timestamp and local collection timestamp are separate. A
  successful check does not imply real-time accounting.

## Tray and settings validation (2026-11-18)

- Copilot settings expose enablement and an optional GitHub CLI executable.
  The earlier billing adapter and its configuration fields have been removed.
- Missing snapshots, malformed quantities and missing usage produce explicit
  failures instead of invented zero consumption or full allowance.
- The default tooltip includes `claude:model:fable` below Claude's first
  window when present. Explicit tooltip selections remain authoritative.
- Native Windows control tests verify settings round trips, validation,
  page changes, controls fitting the client area, the three usage sections,
  exact allowance-left progress values, no bar for unknown denominators,
  window centering and close behavior. Persistence tests verify replacement
  of an existing configuration and preservation after invalid edits.
- Settings are written next to the executable; an existing AppData
  configuration is copied on first run. Provider changes cancel obsolete
  requests and discard their queued results before refreshing new settings.
- Usage links support separate Chrome profile directories. The GitHub link
  is `https://github.com/settings/copilot/features`.
- A release-executable smoke test exercised startup, single left click
  without a menu, double left click opening Usage, right click opening the
  context menu, all Settings pages, saving portable configuration and Quit.
  Rendered usage bars were inspected with live Claude and Work/Codex values.
- Multi-monitor DPI changes, light/dark taskbars, suspend/resume and Explorer
  restart still require interactive validation.
- The usage dialog now uses light grey rounded provider cards, original
  provider logos, compact Segoe UI text and thin blue allowance-left bars.
  Settings uses the same smaller base font and native themed controls.
  Own-window captures verified all four live allowance rows. Repaint tests
  cover repeated rebuilds, opaque label backgrounds and native client
  rendering to prevent overlapping text and stale bars.
- The legacy shell tooltip clipped after roughly 64 characters. A tracked
  native tooltip now retains complete Work/Codex and Copilot rows; a native
  control regression test checks text beyond that boundary.

## Dialog refinements and pace lights (2026-11-18)

- Claude desktop's bundled CSS uses the proprietary `anthropic-sans` face
  with `system-ui, "Segoe UI"` fallbacks: body 14px, caption 12px, heading
  15px, weight 600. The dialogs use Segoe UI at those sizes; the face is
  not copied. Claude's bar colours (fill rgb(106,171,241), track
  rgb(228,239,251)) were sampled from a screenshot.
- Provider logos are loaded with `LoadImageW` at the exact drawn size from
  multi-size ICOs; the previous 32px-only resources were stretched.
- Settings edits are borderless and centred inside painted rounded boxes;
  a native test checks edit and box centres agree within 1 pixel.
- Path and profile fields use `IFileOpenDialog`. Chrome profiles must be a
  direct child of a folder containing Chrome's `Local State` file, because
  `--profile-directory` is relative to the user data folder.
- The tooltip selector writes an empty `tooltip.windows` for Automatic and
  rejects an empty manual selection; a native test covers both.
- Hover rows show pace lights computed from the window length: Claude
  weekly windows are 7 days and Claude 5h is 5 hours (fixed by the API
  field names), Work/Codex uses `windowDurationMins`, and Copilot's window
  is assumed to be the calendar month ending at its reset. The month
  length is an inference, not a reported value. Unit tests cover the
  thresholds for weekly and 30-day windows and unknown inputs; a native
  test checks the painted circle colours.

## Error semantics shared by all adapters

- Sign-in, rate-limit (with Retry-After when provided), unsupported,
  timeout and generic errors are classified separately.
- A transient failure keeps the last good snapshot and shows the failure
  reason. The values are labelled stale once the last success is 30 minutes
  old, so one failed refresh of recent values is not reported as stale data.
- One adapter's failure never erases another adapter's data.
- Fetches are deduplicated per provider with exponential backoff
  (30 s base, 15 min cap).
