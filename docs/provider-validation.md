# Provider validation

Observed provider behaviour and limitations. All probes below were read-only:
no model prompt was submitted, no allowance was consumed and no reset credit
was redeemed. Validation date: 2026-11-16, on Windows 11 x64.

## Summary

| Provider | Interface | Live validation | Status |
| --- | --- | --- | --- |
| Claude | Agent SDK experimental usage method via Node helper | Yes (2026-11-16) | Working, explicitly experimental |
| Work/Codex | `codex app-server` JSON-RPC `account/rateLimits/read` | Yes (2026-11-16) | Working |
| Copilot | `GET /users/{username}/settings/billing/ai_credit/usage` | No (blocked, see below) | Implemented against documented schema, unverified against live data |

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

- Endpoint: `GET https://api.github.com/users/{username}/settings/billing/ai_credit/usage?year=Y&month=M`
  with `Accept: application/vnd.github+json` and `X-GitHub-Api-Version:
  2022-11-28`. The docs example also shows version `2026-03-10`; the adapter
  sends the stable `2022-11-28`.
- Authentication: fine-grained PAT with the account permission "Plan"
  (read). AIAllowance stores it in Windows Credential Manager (service
  `AIAllowance`, user `github-copilot-pat`) via `--set-copilot-token`.
- Live validation is BLOCKED on this machine: the available `gh` CLI OAuth
  token lacks the required permission (the API answered 404 and gh suggested
  the classic `user` scope). No broader-scope token was created for the
  probe. The adapter is implemented against the documented response schema
  and synthetic fixtures only; field interpretation below needs confirmation
  against a real account dashboard once a PAT is available.
- Documented response schema (confirmed from the endpoint reference):
  `timePeriod {year, month, day}`, `user`, and `usageItems[]` with `product`,
  `sku`, `model`, `unitType`, `pricePerUnit`, `grossQuantity`, `grossAmount`,
  `discountQuantity`, `discountAmount`, `netQuantity`, `netAmount`.
- Interpretation implemented (to verify): `discountQuantity` is consumption
  covered by the included allowance, `netQuantity` is billed overage. A net
  amount of zero does not mean no included allowance was consumed. Items are
  filtered to products containing `copilot` (configurable via
  `providers.copilot.products`); the user-level endpoint is personal-account
  only and no organization endpoint is used.
- Allowance: the usage report does not return the plan's total included
  credits, and no verified entitlement source was found. The user may set
  `monthly_included_credits` in config; the UI marks the resulting
  percentage as configured. Without it, the app shows credits used and the
  reset time, and does not invent a percentage. Plan totals are never
  hardcoded.
- Reset semantics: included credits reset on the first day of each calendar
  month at 00:00 UTC per GitHub's Copilot billing documentation (verified in
  the docs on 2026-11-16, not against live data). The app computes the reset
  timestamp in UTC and displays it in the local time zone.
- Latency: billing data can lag; the endpoint exposes no data timestamp, so
  only the local collection time is recorded and a successful poll is not
  treated as proof of real-time accounting.

## Error semantics shared by all adapters

- Sign-in, rate-limit (with Retry-After when provided), unsupported,
  timeout and generic errors are classified separately.
- A transient failure keeps the last good snapshot, labelled stale.
- One adapter's failure never erases another adapter's data.
- Fetches are deduplicated per provider with exponential backoff
  (30 s base, 15 min cap).
