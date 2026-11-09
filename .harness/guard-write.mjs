// .harness/guard-write.mjs
import fs from "node:fs";

function readStdinSafe() {
  try {
    if (process.stdin.isTTY) return "";
    return fs.readFileSync(0, "utf8");
  } catch {
    return "";
  }
}

const raw = readStdinSafe();
let data = {};
try {
  data = JSON.parse(raw || "{}");
} catch {
  data = { raw };
}

const isAntigravity = Boolean(
  data && typeof data === "object" && "conversationId" in data && "toolCall" in data
);

// In Antigravity, inspect only toolCall so chat history (lastUserInput) does not cause false positives
const targetPayload = isAntigravity ? data.toolCall : data;
const serialized = JSON.stringify(targetPayload).toLowerCase().replace(/\\/g, "/");

// Allow legitimate harness CLI invocations in shell commands
const isLegitimateHarnessCall =
  serialized.includes("harness.mjs") || serialized.includes("harness.ts");

const touchesProtectedFile =
  serialized.includes(".agent/state.json") ||
  serialized.includes(".agent/history.jsonl") ||
  serialized.includes("00-projected-state.mdc") ||
  serialized.includes("agents.md");

if (touchesProtectedFile && !isLegitimateHarnessCall) {
  const msg =
    "DENIED BY HARNESS: Direct edits to .agent/state.json, .agent/history.jsonl, 00-projected-state.mdc, or AGENTS.md are forbidden. Use 'node .harness/harness.mjs checkpoint' or 'fail'.";
  if (isAntigravity) {
    console.log(
      JSON.stringify({
        decision: "deny",
        reason: msg
      })
    );
    process.exit(0);
  }
  console.error(msg);
  console.log(
    JSON.stringify({
      permission: "deny",
      user_message: msg,
      agent_message: msg
    })
  );
  process.exit(2);
}

if (isAntigravity) {
  console.log(JSON.stringify({ decision: "allow" }));
} else {
  console.log(JSON.stringify({ continue: true, permission: "allow" }));
}
