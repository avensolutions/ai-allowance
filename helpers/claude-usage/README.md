# claude-usage helper

Small pinned Node.js helper used by the AIAllowance Claude adapter.

It opens a Claude Agent SDK session with your existing Claude Code sign-in,
calls the SDK's experimental usage method without submitting any prompt, and
prints one JSON document to stdout. It consumes no model tokens.

Requirements:

- Node.js 23.6 or later (the TypeScript file runs via native type stripping;
  tested with Node.js 24).
- An existing Claude Code sign-in on this machine.
- `npm install` executed in this directory (installs the pinned
  `@anthropic-ai/claude-agent-sdk` 0.3.325).

Run manually to test:

```text
npm install
node usage.ts
```

Output contract consumed by the Rust adapter:

```json
{ "ok": true, "data": { "subscription_type": "...", "rate_limits_available": true, "rate_limits": { } } }
{ "ok": false, "error": { "kind": "sign_in_required" | "error", "message": "..." } }
```

The SDK method is explicitly experimental
(`usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET`) and may change
or disappear in any SDK release. Upgrade the pin deliberately and re-validate
against docs/provider-validation.md.
