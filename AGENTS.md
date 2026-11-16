# AIAllowance project instructions

AIAllowance is a Windows system tray utility that shows AI subscription usage and reset times. The initial providers are Claude Ultra, ChatGPT Pro through its Work/Codex allowance, and personal GitHub Copilot Max.

Read this file before changing the project. Treat README.md as the public description of what actually works. Use KICKOFF_PROMPT.md for the initial implementation task when that file is available.

## Product contract

Build a small, local Rust application with an Aven Solutions starburst tray icon. Hovering over the icon shows remaining allowance and the time until reset. Use "left" consistently; never show an unexplained percentage.

Illustrative output, not live account data:

```text
Claude: 75% left | 7h
Work/Codex: 50% left | 18h
Copilot: 50% left | 7d
```

Show separate rows for separate allowance windows. Claude overall and Fable are distinct. Work/Codex may expose several buckets and windows. Do not add percentages together or silently switch between buckets. Let the user choose which windows appear in the compact tooltip; expose the rest in the menu.

Initial scope is Windows 11 x64. Keep provider code independent of the Windows UI so other platforms can be added later. Do not claim compatibility with untested systems.

## Implementation choices

Use Rust stable and a native tray implementation. Start with tray-icon and its compatible menu library, with a Windows message loop through an appropriate event-loop crate or the windows crate. No .NET, Electron, Tauri or embedded browser is required for the initial app.

Use tokio for asynchronous work, reqwest with normal TLS verification for HTTP, serde for provider payloads and configuration, and a maintained time library. Pick current compatible crate versions during implementation and commit Cargo.lock. Keep dependencies limited to actual requirements.

Run the tray and Windows message loop on the appropriate UI thread. Fetch providers and run helpers off that thread. Send results back through a channel or event-loop proxy; never block a tray callback on network or subprocess work.

A small TypeScript helper is acceptable only if needed for Claude's Agent SDK. Keep that dependency isolated, pinned and documented. The main application remains Rust. Do not claim a fully standalone executable if it requires Node.js, Claude Code or Codex to be installed.

Suggested modules, not a mandatory framework:

- src/main.rs: lifecycle and application startup.
- src/tray.rs: tray icon, menus, hover and UI updates.
- src/state.rs: account snapshots, quota windows and display state.
- src/refresh.rs: refresh scheduling, timeouts and duplicate suppression.
- src/providers/: independent Claude, Codex and Copilot adapters.
- src/config.rs: user preferences and credential references.
- src/notifications.rs: optional threshold and reset notifications.
- assets/: tray icons and source asset attribution.
- tests/fixtures/: synthetic or redacted provider payloads.
- docs/provider-validation.md: observed provider behaviour and limitations.
- helpers/claude-usage/: only if the SDK investigation requires it.

## Hover and refresh behaviour

Show the last successful snapshot immediately. On hover entry, refresh enabled providers concurrently if their data is older than 60 seconds. On first hover with no data, show "Loading" while requests run.

Allow only one request in flight per provider/account. Repeated hover and mouse-move events must not create duplicate fetches. A provider timeout must not delay the others. Use bounded request and startup timeouts, respect Retry-After, and back off after failures.

Update countdowns from reset timestamps locally. Countdown changes do not require a network request. Reaching a timestamp does not prove the provider has restored allowance: show "Checking reset" and refresh before announcing a reset.

A normal Windows tray tooltip has 127 usable UTF-16 code units. Keep the compact text within that limit, including line breaks. Use the context menu for additional windows, last-update times and full local reset dates. Start with a plain tooltip; add a custom popup only if native tooltip behaviour makes the required interaction unreliable.

The context menu should contain Refresh now, provider status/details, Open usage pages, Settings, Start with Windows and Quit. Settings can initially open a documented configuration file. Start with Windows is opt-in.

Use the user's Windows locale/time zone for absolute reset times. Do not hardcode Melbourne or Sydney. Handle suspend/resume, clock changes, Explorer restarts and application shutdown. Keep a single application instance and suppress an unwanted console window in release builds.

Default to hover-triggered refresh. Optional background polling, initially five minutes, is needed for unattended notifications. Make that dependency clear when enabling alerts.

## Provider discovery comes first

Do not implement from recollection alone. The sources below were reviewed on 2026-11-16; interfaces may have changed. Check current primary documentation and installed versions, then record observed results.

For each provider, document authentication, request/method, response shape, units, update latency, quota coverage, reset semantics and whether a model call is needed. Separate documented contracts from experimental interfaces and observations.

Do not send a model prompt just to measure quota. Do not consume a paid allowance, redeem a reset credit, change a plan, enable extra usage or modify spending limits as part of a probe. If a read-only quota check is unavailable, keep the adapter visibly unavailable and continue independent work.

### Claude

Investigate the TypeScript package @anthropic-ai/claude-agent-sdk first. Its changelog records an experimental Query method named:

```typescript
usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET()
```

The changelog also describes model_scoped entries in usage responses for model-specific weekly limits. These are leads to verify, not a guarantee of the current installed SDK's behaviour.

Prove whether an authenticated SDK session can call the usage method without submitting a prompt. Verify that it reports fresh Ultra overall and Fable windows. Test absent windows and the lack of a five-hour window on verified Ultra accounts.

The Agent SDK's rate_limit_event and Claude Code's status-line rate_limits fields are separate interfaces. Do not assume they share response shape, units or completeness. The documented Python event utilization is a fraction; status-line used_percentage is a percentage. Confirm units independently for the experimental method.

Keep experimental response parsing isolated and tolerant of unknown fields. Treat missing fields as unavailable. If a helper is necessary, emit a small JSON response over stdout, write redacted diagnostics to stderr, and bound its lifetime.

Use the user's existing supported Claude authentication flow. Do not extract browser cookies or silently replace supported interfaces with private endpoints. Do not edit the vendor's credential files. A future unofficial integration must be explicitly identified and deliberately enabled.

Sources:
- https://github.com/anthropics/claude-agent-sdk-typescript/blob/main/CHANGELOG.md
- https://code.claude.com/docs/en/agent-sdk/typescript
- https://code.claude.com/docs/en/agent-sdk/python
- https://code.claude.com/docs/en/statusline
- https://support.claude.com/en/articles/17728952-what-is-the-ultra-plan

### ChatGPT through Codex

Use the official local codex app-server with the user's existing ChatGPT sign-in. Implement the documented initialization handshake, request IDs and JSON-RPC framing.

Read account/rateLimits/read and process account/rateLimits/updated notifications. Prefer rateLimitsByLimitId when present; use the legacy rateLimits value as a fallback, not an additional bucket to count twice. Retain each limit ID, window duration, usedPercent and resetsAt.

Keep a bounded, supervised process lifecycle. Requesting quota must not start a model turn. Check available methods against the installed version and handle unsupported methods gracefully.

Label the source Work/Codex. Do not imply that it covers all ChatGPT model, image, voice or research limits. Do not use API-platform billing data as a substitute for subscription allowance. Do not redeem available reset credits.

Source:
- https://developers.openai.com/codex/app-server

### GitHub Copilot

Use the documented personal-account billing REST API:

```text
GET https://api.github.com/users/{username}/settings/billing/ai_credit/usage
```

The endpoint reference documents a fine-grained PAT with Plan read permission. Confirm current authentication requirements and API version. Avoid broader scopes than the endpoint needs.

Query the correct calendar month and filter the applicable Copilot usage. Interpret gross, discounted and net quantities against the current documentation and actual account dashboard. A billed amount of zero does not mean no included allowance was consumed.

The usage report alone may not supply the complete allowance. Prefer a verified entitlement source; otherwise allow the user to configure the account's total monthly included credits. Mark a configured allowance as such. If the total is unknown, show credits used and reset time without inventing a percentage.

Do not hardcode a plan total: flex allotments and promotions can change. Distinguish included usage from purchased overage. Included credits currently reset on the first day of each calendar month at 00:00 UTC; verify this before implementation. Preserve UTC semantics when displaying the date locally.

Billing data can lag. Record collection time and any available provider data timestamp separately; a successful poll does not prove real-time accounting. Never use an organization-only usage API for a personal subscription.

Sources:
- https://docs.github.com/en/rest/billing/usage
- https://docs.github.com/en/copilot/concepts/billing-and-usage/individuals/billing

## Data and error semantics

Keep raw provider values separate from normalized display values. Each window needs a stable provider/account/window identity, label, optional used percentage or units, optional allowance, optional UTC reset time, collection time, and freshness/source information.

Represent not configured, loading, ready, stale, sign-in required, rate limited, unsupported and error explicitly. Retain the last good values on transient failure and label them stale. Clear cached data when the account changes.

Convert percentages only when their units are established. Compute remaining percentage from a valid quota denominator, bound the displayed value to 0 through 100, and retain overage separately. Missing, malformed or unexpected values must never become 0% used, 100% left or "unlimited".

Use per-provider/account cache entries. A failure in one adapter must not erase another adapter's successful data.

## Notifications and local data

Notifications are optional. Initial thresholds are 20% and 10% remaining, plus a confirmed reset notification. Alert on crossings, not every refresh. Deduplicate per account, quota window and reset cycle. Do not issue threshold or reset alerts from stale or unavailable data.

Store configuration in the Windows user application-data directory. Store user-supplied secrets in Windows Credential Manager through a suitable maintained Rust library. Never put tokens in TOML, logs, fixtures, command arguments or repository files. Keep normal usage snapshots in memory initially.

Launch helpers without visible console windows. Pass input through structured stdin and arguments without a shell. Redact credentials and personal identifiers from diagnostics. No telemetry, remote service, administrator requirement or cloud synchronization is part of the initial scope.

## Branding

Use the actual starburst favicon from https://avensolutions.ai/ as the reference requested by the maintainer. Inspect the site's icon links and retrieve the original asset. Preserve provenance; do not invent a replacement or copy another vendor's mark.

Prepare suitable Windows ICO sizes, including 16 and 32 pixels, and verify legibility on light/dark taskbars and scaled displays. If the source asset cannot be retrieved, use a clearly identified temporary development icon and report the remaining work.

## Verification and delivery

Test meaningful behaviour: provider parsing and units, absent windows, configurable allowances, multiple Codex buckets, reset boundaries, stale/error states, account changes, duplicate refresh suppression and notification deduplication. Use synthetic or redacted fixtures. CI must not need live credentials.

Manually verify hover, tooltip updates, menus, keyboard access, startup/shutdown, suspend/resume, Explorer restart and display scaling on Windows. Identify checks that the environment cannot run.

Run formatting, compilation, linting and relevant tests. Add a Windows CI workflow once there is buildable code. Do not add tests that only restate constants or mirror implementation details.

Keep README.md honest about implemented, experimental and planned features. Include exact build commands only after they work. Record any external CLI/helper requirements. Default to the MIT license for code unless the repository already specifies another license; add the license file during bootstrap using AIAllowance contributors as the copyright holder. Keep code licensing separate from third-party dependencies and branding. Do not create releases or publish a repository unless requested.

Write concise technical prose. Use ASCII punctuation, no emojis, no hype and no invented benchmark claims. Make routine reversible implementation choices and continue; do not stop at a plan. Summarize what works, what was tested and what is still blocked.
