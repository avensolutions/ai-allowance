# AIAllowance

AI subscription usage and reset times in your Windows system tray.

AIAllowance is a small native Rust utility that shows your remaining Claude,
ChatGPT Work/Codex and GitHub Copilot allowances without opening several usage
pages. Hover over the Aven Solutions starburst icon for a compact summary:

```text
Claude: 37% left | 6h
Work/Codex: 20% left | 7d
Copilot: 50% left | 14d
```

Separate quotas get separate entries, including Claude's overall and
model-scoped (for example Fable) weekly windows and multiple Work/Codex
buckets. Percentages are never added together, and the app never invents a
percentage without a valid denominator.

**Status:** first working version. The tray application builds and runs on
Windows 11 x64. Claude and Work/Codex collection has been validated against
live accounts; the Copilot adapter is implemented against the documented API
schema but has not yet been verified against live billing data (see
[docs/provider-validation.md](docs/provider-validation.md)).

## What works

- Native tray icon with hover tooltip and right-click menu (no .NET,
  Electron, Tauri or browser UI).
- Cached values show immediately; hovering refreshes providers whose data is
  older than 60 seconds, concurrently and with one request in flight per
  provider. Countdown text updates locally without network requests.
- Explicit per-provider states: loading, ready, stale (last good values are
  kept when a refresh fails), sign-in required, rate limited, unavailable,
  error and not configured. One provider's failure never hides another's
  data.
- Context menu: per-window details with exact local reset times and last
  update time, Refresh now, Open usage pages, Settings (opens the config
  file), opt-in Start with Windows, Quit.
- Optional background polling (default 5 minutes) and toast notifications at
  20% and 10% remaining plus confirmed resets, deduplicated per window and
  reset cycle. Notifications require polling to be useful unattended.
- Single application instance; no console window in release builds; no
  telemetry, server or database. Usage snapshots stay in memory.

## Providers

| Provider | Source | Notes |
| --- | --- | --- |
| Claude | Claude Agent SDK experimental usage API via a pinned Node.js helper | Experimental: the SDK method may change in any release. Uses your existing Claude Code sign-in. No prompt is submitted and no tokens are consumed. |
| Work/Codex | Official local `codex app-server` (`account/rateLimits/read`) | Uses your existing ChatGPT sign-in. Covers the Work/Codex buckets only, not other ChatGPT feature limits. Never consumes rate-limit reset credits. |
| Copilot | GitHub billing AI-credit usage REST API (personal account) | Needs a fine-grained PAT with the "Plan" read permission. The API does not expose your total allowance; configure `monthly_included_credits` to see a percentage, otherwise credits used are shown. Unverified against live data. |

## Requirements

- Windows 11 x64.
- Rust stable 1.85+ (tested with 1.97.1) to build.
- For Claude: Node.js 23.6+ on PATH and an existing Claude Code sign-in.
- For Work/Codex: a codex CLI (`codex` on PATH, the binary bundled with the
  ChatGPT VS Code extension is found automatically, or set
  `providers.codex.command`).
- For Copilot: a fine-grained GitHub PAT with the account permission
  "Plan" (read).

## Build and run

```text
cargo build --release
npm install --prefix helpers/claude-usage
target\release\aiallowance.exe
```

The helper install is only needed for the Claude provider. When the app runs
from a different directory, copy `helpers\claude-usage` (with its
`node_modules`) next to the executable or set `providers.claude.helper_dir`.

Run the tests (synthetic fixtures only, no credentials needed):

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Configuration

The first run writes a documented config file to
`%APPDATA%\AIAllowance\config.toml` (also reachable via the tray menu's
Settings item). Providers are enabled there; Copilot additionally needs:

```toml
[providers.copilot]
enabled = true
username = "your-github-username"
# Optional: your plan's total monthly included credits. Shown as a
# percentage marked "configured total". Never hardcoded by the app.
# monthly_included_credits = 300.0
```

Store the Copilot token in Windows Credential Manager (never in files):

```text
aiallowance --set-copilot-token
```

The compact tooltip shows the first window of each provider by default;
choose specific windows with `tooltip.windows` using the ids listed in
docs/provider-validation.md (for example `claude:overall`,
`claude:model:fable`, `codex:codex:primary`, `copilot:credits`). All windows
remain visible in the menu.

## Known issues and limitations

- The Claude usage API is explicitly experimental; a future SDK release may
  break it. The helper is pinned to `@anthropic-ai/claude-agent-sdk 0.3.325`.
- Claude refreshes take several seconds because the helper spawns the SDK
  runtime each time.
- Copilot field interpretation (included consumption = discounted quantity,
  overage = net quantity) follows the documented schema but is not yet
  validated against a live dashboard.
- Tooltip text that would exceed the 127 UTF-16 unit Windows limit is
  truncated; the menu always has the full details.
- Checks that need interactive sessions (suspend/resume, Explorer restart,
  display scaling changes) are manual; see docs/provider-validation.md for
  what was and was not verified.

## Development

Read [AGENTS.md](AGENTS.md) before making changes. Provider adapters live in
`src/providers/` and are independent of the tray UI (`src/tray.rs`,
Windows-only). Parsing is tested against synthetic fixtures in
`tests/fixtures/`; CI (`.github/workflows/ci.yml`) runs formatting, clippy,
tests and a release build on Windows without live credentials.

The tray icon is the Aven Solutions starburst favicon; see
[assets/ATTRIBUTION.md](assets/ATTRIBUTION.md) for provenance. The icon is
not covered by the MIT code license.

## License

MIT, see [LICENSE](LICENSE). Third-party dependencies and branding retain
their own terms.

AIAllowance is an independent project and is not affiliated with Anthropic,
OpenAI or GitHub.
