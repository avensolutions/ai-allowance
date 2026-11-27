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
import { readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

type ErrorKind = "sign_in_required" | "rate_limited" | "error";

type HelperResult =
  | { ok: true; data: unknown }
  | {
      ok: false;
      error: { kind: ErrorKind; message: string; retry_after_secs?: number };
    };

// The bundled CLI's debug log is the only place it says why a usage read
// came back without rate limits. It goes to a private temporary file that is
// removed before exit, never to %USERPROFILE%\.claude.
const debugFile = join(
  tmpdir(),
  `aiallowance-claude-usage-${process.pid}-${Date.now()}.log`,
);

// Streaming input that never yields: the session initializes but no prompt
// is ever submitted, so no model tokens are consumed.
async function* noPrompt(): AsyncGenerator<never> {
  await new Promise<never>(() => {});
}

const q = query({ prompt: noPrompt(), options: { debugFile } });

function emit(result: HelperResult, exitCode: number): never {
  // End the CLI subprocess so it releases the debug file, then remove it.
  // Cleanup failures must not cost the result.
  try {
    q.close();
  } catch {
    // Already closed.
  }
  try {
    rmSync(debugFile, { force: true, maxRetries: 5, retryDelay: 100 });
  } catch (e: unknown) {
    console.error("claude-usage helper: could not remove debug log:", String(e));
  }
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

// Observed in CLI 2.1.325 (SDK 0.3.325): the CLI asks /api/oauth/usage with a
// 5 s timeout and reports `rate_limits: null` when that read fails. A 429 or
// 403 is remembered for the rest of the process and logged as
// "fetchUtilization: 429 remembered for this bearer; not asking again for 300s".
const REMEMBERED = /fetchUtilization: (\d{3}) remembered for this bearer; not asking again for (\d+)s/;
const FAILURE =
  /(Failed to load usage data: .*|Usage fetch returned a fieldless or non-object body.*|fetchUtilization: .* replayed.*)$/;

type Diagnosis =
  | { kind: "rate_limited"; status: string; retryAfterSecs: number }
  | { kind: "failed"; detail: string | null };

function diagnose(): Diagnosis {
  let log = "";
  try {
    log = readFileSync(debugFile, "utf8");
  } catch {
    // No debug output: report the failure without detail.
  }
  let detail: string | null = null;
  for (const line of log.split(/\r?\n/)) {
    const held = REMEMBERED.exec(line);
    if (held) {
      return { kind: "rate_limited", status: held[1], retryAfterSecs: Number(held[2]) };
    }
    const failure = FAILURE.exec(line);
    if (failure) {
      detail = failure[1].slice(0, 200);
    }
  }
  return { kind: "failed", detail };
}

const RETRY_DELAY_MS = 2_000;

async function main(): Promise<void> {
  try {
    const read = () =>
      q.usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET({ skipBehaviors: true });
    let usage: any = await read();
    let diagnosis: Diagnosis | null = null;
    // rate_limits_available true with rate_limits null means the CLI's
    // endpoint read failed. A rate limit is final for this process; anything
    // else (slow answer, network error) gets one more read.
    if (usage?.rate_limits_available === true && usage?.rate_limits == null) {
      diagnosis = diagnose();
      if (diagnosis.kind === "failed") {
        console.error("claude-usage helper: usage read failed, retrying once");
        await new Promise((resolve) => setTimeout(resolve, RETRY_DELAY_MS));
        usage = await read();
        diagnosis = usage?.rate_limits == null ? diagnose() : null;
      }
    }
    clearTimeout(watchdog);
    if (diagnosis?.kind === "rate_limited") {
      emit(
        {
          ok: false,
          error: {
            kind: "rate_limited",
            message: `Claude usage endpoint answered HTTP ${diagnosis.status}; retry in ${diagnosis.retryAfterSecs}s`,
            retry_after_secs: diagnosis.retryAfterSecs,
          },
        },
        1,
      );
    }
    if (diagnosis?.kind === "failed") {
      const detail = diagnosis.detail ?? "no reason logged";
      emit(
        {
          ok: false,
          error: {
            kind: "error",
            message: `Claude usage endpoint read failed twice (${detail})`,
          },
        },
        1,
      );
    }
    const data = {
      subscription_type: usage?.subscription_type ?? null,
      rate_limits_available: usage?.rate_limits_available ?? null,
      rate_limits: usage?.rate_limits ?? null,
    };
    emit({ ok: true, data }, 0);
  } catch (e: unknown) {
    const message = e instanceof Error ? e.message : String(e);
    console.error("claude-usage helper:", message);
    clearTimeout(watchdog);
    emit({ ok: false, error: { kind: classify(message), message } }, 1);
  }
}

void main();
