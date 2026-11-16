// AIAllowance Claude usage helper.
//
// Opens a Claude Agent SDK session using the user's existing Claude Code
// sign-in, calls the experimental usage method WITHOUT submitting any prompt,
// prints exactly one JSON document to stdout and exits. Diagnostics go to
// stderr only. The SDK method is unstable and may change; the Rust side
// treats missing fields as unavailable.
//
// Requires Node.js 23.6+ (native TypeScript type stripping) and
// `npm install` in this directory. Pinned to @anthropic-ai/claude-agent-sdk
// 0.3.325, validated 2026-11-16.

import { query } from "@anthropic-ai/claude-agent-sdk";

type HelperResult =
  | { ok: true; data: unknown }
  | { ok: false; error: { kind: "sign_in_required" | "error"; message: string } };

function emit(result: HelperResult, exitCode: number): never {
  process.stdout.write(JSON.stringify(result) + "\n");
  process.exit(exitCode);
}

function classify(message: string): "sign_in_required" | "error" {
  const lower = message.toLowerCase();
  if (
    lower.includes("auth") ||
    lower.includes("login") ||
    lower.includes("sign in") ||
    lower.includes("sign-in") ||
    lower.includes("credential") ||
    lower.includes("api key")
  ) {
    return "sign_in_required";
  }
  return "error";
}

// Bounded lifetime even if the SDK hangs.
const watchdog = setTimeout(() => {
  emit(
    { ok: false, error: { kind: "error", message: "helper timed out after 80s" } },
    2,
  );
}, 80_000);

// Streaming input that never yields: the session initializes but no prompt
// is ever submitted, so no model tokens are consumed.
async function* noPrompt(): AsyncGenerator<never> {
  await new Promise<never>(() => {});
}

async function main(): Promise<void> {
  const q = query({ prompt: noPrompt(), options: {} });
  try {
    const usage = await q.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({
      skipBehaviors: true,
    });
    const data = {
      subscription_type: (usage as any)?.subscription_type ?? null,
      rate_limits_available: (usage as any)?.rate_limits_available ?? null,
      rate_limits: (usage as any)?.rate_limits ?? null,
    };
    clearTimeout(watchdog);
    try {
      await q.interrupt();
    } catch {
      // Session teardown failures are irrelevant once usage was read.
    }
    emit({ ok: true, data }, 0);
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : String(e);
    console.error("claude-usage helper:", message);
    clearTimeout(watchdog);
    emit({ ok: false, error: { kind: classify(message), message } }, 1);
  }
}

void main();
