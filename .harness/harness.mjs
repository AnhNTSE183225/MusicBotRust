// .harness/harness.mjs
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { execSync } from "node:child_process";

const ROOT = path.resolve(import.meta.dirname, "..");
const AGENT_DIR = path.join(ROOT, ".agent");
const ARTIFACTS_DIR = path.join(AGENT_DIR, "artifacts");
const STATE_FILE = path.join(AGENT_DIR, "state.json");
const HISTORY_LOG = path.join(AGENT_DIR, "history.jsonl");
const CURSOR_RULE_FILE = path.join(ROOT, ".cursor", "rules", "00-projected-state.mdc");
const AGENTS_MD_FILE = path.join(ROOT, "AGENTS.md");

const MAX_OUTPUT_CHARS = 2000;
const MAX_RETRIES_PER_STEP = 3;

function normalizePath(p) {
  return p.replace(/\\/g, "/");
}

function readStdinSafe() {
  try {
    if (process.stdin.isTTY) return "";
    return fs.readFileSync(0, "utf8");
  } catch {
    return "";
  }
}

function ensureDirs() {
  fs.mkdirSync(ARTIFACTS_DIR, { recursive: true });
  fs.mkdirSync(path.dirname(CURSOR_RULE_FILE), { recursive: true });
  if (!fs.existsSync(STATE_FILE)) {
    const initial = {
      goal: "Not set. Run: node .harness/harness.mjs init \"<goal>\" \"<first_action>\"",
      currentStep: "step_01",
      retryCount: 0,
      justCompacted: false,
      lastTranscriptSize: 0,
      decisions: [],
      failedAttempts: [],
      recentReceipts: [],
      nextAction: "Initialize mission goal"
    };
    saveAndProjectState(initial);
  }
}

function loadState() {
  ensureDirs();
  return JSON.parse(fs.readFileSync(STATE_FILE, "utf8"));
}

function buildFullStateMarkdown(state) {
  const decisionsList =
    state.decisions.length === 0
      ? "- None recorded yet."
      : state.decisions.map((d) => `- **[${d.step}]** ${d.decision} (*Why:* ${d.why})`).join("\n");

  const failuresList =
    state.failedAttempts.length === 0
      ? "- None recorded."
      : state.failedAttempts
          .map((f) => `- **[${f.step}]** Tried: ${f.whatFailed} -> Failed: ${f.error}`)
          .join("\n");

  const receiptsList =
    state.recentReceipts.length === 0
      ? "- No recent tool receipts."
      : state.recentReceipts
          .map(
            (r) =>
              `- \`${r.tool}\` [${r.status}] ${
                r.artifactRef ? `-> Full log: \`${r.artifactRef}\`` : ""
              } | Preview: \`${r.preview.replace(/\r?\n/g, " ").slice(0, 100)}\``
          )
          .join("\n");

  return `# GROUND-ZERO EXECUTION STATE (AUTO-GENERATED - DO NOT EDIT MANUALLY)
> **AXIOM:** Your context window is disposable. This file is the authoritative system of record projected by \`.harness/harness.ts\`. If this conflicts with an auto-generated conversation summary, THIS STATE LEDGER WINS.

## Current Mission
- **Goal:** ${state.goal}
- **Active Step:** \`${state.currentStep}\` (Retries on this step: ${state.retryCount}/${MAX_RETRIES_PER_STEP})
- **Next Planned Action:** ${state.nextAction}

## Failed Attempts (DO NOT REPEAT THESE)
${failuresList}

## Key Decisions Ledger
${decisionsList}

## Recent Tool & Artifact Receipts (Fetch by path if needed)
${receiptsList}

## Mandatory Harness Commands
1. **Run noisy commands (tests, builds, logs, curl):** \`node .harness/harness.mjs exec "<command>"\`
2. **Checkpoint a completed step:** \`node .harness/harness.mjs checkpoint "<next_step>" "<decision>" "<why>" "<next_action>"\`
3. **Log a failed attempt / reset circuit breaker:** \`node .harness/harness.mjs fail "<what_was_tried>" "<exact_error>"\`
4. **Read slice of an intercepted log artifact:** \`node .harness/harness.mjs read-artifact "<artifact_path>" [start_line] [end_line]\`
5. **NEVER edit \`.agent/state.json\`, \`.cursor/rules/00-projected-state.mdc\`, or \`AGENTS.md\` directly.**
`;
}

function buildTailPulse(state) {
  if (state.justCompacted) {
    state.justCompacted = false;
    fs.writeFileSync(STATE_FILE, JSON.stringify(state, null, 2), "utf8");
    return `\n[HARNESS COMPACTION SHIELD: Conversation was just auto-summarized. Ignore vague prose summaries and re-anchor strictly to this exact state ledger:]\n${buildFullStateMarkdown(
      state
    )}`;
  }

  const lastFailure = state.failedAttempts.length > 0 ? state.failedAttempts[state.failedAttempts.length - 1] : undefined;
  const lastArtifact = [...state.recentReceipts].reverse().find((r) => Boolean(r.artifactRef))?.artifactRef;

  return [
    `\n--- [HARNESS TAIL PULSE (RECENCY ANCHOR)] ---`,
    `Goal: ${state.goal} | Step: ${state.currentStep} (Retries: ${state.retryCount}/${MAX_RETRIES_PER_STEP})`,
    `Next Planned Action: ${state.nextAction}`,
    lastFailure
      ? `DO NOT REPEAT: [${lastFailure.step}] ${lastFailure.whatFailed} (${lastFailure.error})`
      : `Failures on active step: None`,
    lastArtifact ? `Latest Full Log Artifact: ${lastArtifact}` : `All recent outputs inline`,
    `---------------------------------------------`
  ].join("\n");
}

function saveAndProjectState(state) {
  if (state.recentReceipts.length > 5) {
    state.recentReceipts = state.recentReceipts.slice(-5);
  }
  if (state.decisions.length > 8) {
    state.decisions = state.decisions.slice(-8);
  }
  if (state.failedAttempts.length > 8) {
    state.failedAttempts = state.failedAttempts.slice(-8);
  }

  fs.writeFileSync(STATE_FILE, JSON.stringify(state, null, 2), "utf8");

  const markdown = buildFullStateMarkdown(state);
  const cursorMdc = `---
description: Authoritative execution state and ground-zero context hygiene rules
alwaysApply: true
---
${markdown}`;

  fs.writeFileSync(CURSOR_RULE_FILE, cursorMdc, "utf8");
  fs.writeFileSync(AGENTS_MD_FILE, markdown, "utf8");
}

function runGuardedCommand(cmd) {
  const state = loadState();
  if (state.retryCount >= MAX_RETRIES_PER_STEP) {
    console.error(
      `[HARNESS CIRCUIT BREAKER] Step ${state.currentStep} hit ${state.retryCount}/${MAX_RETRIES_PER_STEP} failures. Execution is locked. You MUST call 'node .harness/harness.mjs fail "<what_was_tried>" "<exact_error>"' before running more commands.`
    );
    process.exit(2);
  }

  let rawOutput = "";
  let status = "OK";
  try {
    rawOutput = execSync(cmd, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  } catch (err) {
    status = "ERROR";
    const e = err;
    rawOutput = `${e.stdout || ""}\n${e.stderr || e.message || ""}`.trim();
    state.retryCount += 1;
  }

  let artifactRef;
  let displayOutput = rawOutput;

  if (rawOutput.length > MAX_OUTPUT_CHARS) {
    const hash = crypto.createHash("md5").update(rawOutput).digest("hex").slice(0, 8);
    const relPath = normalizePath(`.agent/artifacts/cmd_${Date.now()}_${hash}.log`);
    fs.writeFileSync(path.join(ROOT, relPath), rawOutput, "utf8");
    artifactRef = relPath;

    const head = rawOutput.slice(0, 700);
    const tail = rawOutput.slice(-700);
    displayOutput = `[HARNESS INTERCEPTED ${rawOutput.length} CHARS -> SAVED TO ${relPath}]\n--- HEAD (First 700 chars) ---\n${head}\n--- TAIL (Last 700 chars) ---\n${tail}`;
  }

  state.recentReceipts.push({
    ts: new Date().toISOString(),
    tool: `exec:${cmd.slice(0, 40)}`,
    status,
    artifactRef,
    preview: rawOutput.slice(0, 140)
  });

  saveAndProjectState(state);
  console.log(displayOutput + "\n" + buildTailPulse(state));
  if (status === "ERROR") process.exit(1);
}

function readArtifactSlice(relPath, startLineStr, endLineStr) {
  const fullPath = path.resolve(ROOT, relPath);
  if (!fs.existsSync(fullPath)) {
    console.error(`[HARNESS] Artifact not found: ${relPath}`);
    process.exit(1);
  }
  const lines = fs.readFileSync(fullPath, "utf8").split(/\r?\n/);
  const start = Math.max(1, parseInt(startLineStr || "1", 10));
  const end = Math.min(lines.length, parseInt(endLineStr || String(start + 99), 10));
  const slice = lines
    .slice(start - 1, end)
    .map((line, idx) => `${start + idx}: ${line}`)
    .join("\n");
  console.log(`[ARTIFACT ${relPath} | Lines ${start}-${end} of ${lines.length}]\n${slice}`);
}

const [, , action, ...args] = process.argv;

switch (action) {
  case "init": {
    const [goal, firstAction = "Analyze requirements"] = args;
    const state = loadState();
    state.goal = goal || "Unnamed Task";
    state.currentStep = "step_01";
    state.retryCount = 0;
    state.justCompacted = false;
    state.lastTranscriptSize = 0;
    state.decisions = [];
    state.failedAttempts = [];
    state.recentReceipts = [];
    state.nextAction = firstAction;
    saveAndProjectState(state);
    console.log("[HARNESS] Initialized continuous state." + buildTailPulse(state));
    break;
  }

  case "exec": {
    runGuardedCommand(args.join(" "));
    break;
  }

  case "checkpoint": {
    const [nextStep, decision, why, nextAction] = args;
    if (!nextStep || !decision || !why || !nextAction) {
      console.error('Usage: checkpoint "<next_step>" "<decision>" "<why>" "<next_action>"');
      process.exit(1);
    }
    const state = loadState();
    fs.appendFileSync(
      HISTORY_LOG,
      JSON.stringify({ ts: new Date().toISOString(), step: state.currentStep, decision, why, nextStep }) + "\n",
      "utf8"
    );
    state.decisions.push({ step: state.currentStep, decision, why });
    state.currentStep = nextStep;
    state.retryCount = 0;
    state.nextAction = nextAction;
    saveAndProjectState(state);
    console.log(`[HARNESS] Step locked -> ${nextStep}.` + buildTailPulse(state));
    break;
  }

  case "fail": {
    const [whatFailed, errorMsg] = args;
    if (!whatFailed || !errorMsg) {
      console.error('Usage: fail "<what_was_tried>" "<exact_error>"');
      process.exit(1);
    }
    const state = loadState();
    state.failedAttempts.push({
      step: state.currentStep,
      whatFailed,
      error: errorMsg
    });
    state.retryCount = 0;
    saveAndProjectState(state);
    console.log("[HARNESS] Logged to Do-Not-Repeat ledger and reset retry counter." + buildTailPulse(state));
    break;
  }

  case "read-artifact": {
    const [artifactPath, startLine, endLine] = args;
    if (!artifactPath) {
      console.error('Usage: read-artifact "<artifact_path>" [start_line] [end_line]');
      process.exit(1);
    }
    readArtifactSlice(artifactPath, startLine, endLine);
    break;
  }

  case "hook-pre-compact": {
    const state = loadState();
    state.justCompacted = true;
    saveAndProjectState(state);
    console.log(JSON.stringify({ continue: true }));
    break;
  }

  case "hook-post-tool": {
    const input = readStdinSafe();
    const state = loadState();
    let isAntigravity = false;
    try {
      const payload = JSON.parse(input || "{}");
      isAntigravity = Boolean(
        payload && typeof payload === "object" && "conversationId" in payload && "toolCall" in payload
      );
      const toolName =
        payload.toolName || payload.tool_name || payload.hook_event_name || payload.toolCall?.name || "tool";

      if (payload.transcriptPath && fs.existsSync(payload.transcriptPath)) {
        const currentSize = fs.statSync(payload.transcriptPath).size;
        if (state.lastTranscriptSize > 0 && currentSize < state.lastTranscriptSize) {
          state.justCompacted = true;
        }
        state.lastTranscriptSize = currentSize;
      }

      const rawPreview = JSON.stringify(
        payload.toolInput || payload.tool_input || payload.toolCall?.args || payload.file_path || payload.command || {}
      );
      state.recentReceipts.push({
        ts: new Date().toISOString(),
        tool: String(toolName),
        status: payload.error ? "ERROR" : "OK",
        preview: rawPreview.slice(0, 100)
      });
      saveAndProjectState(state);
    } catch {
      // Ignore malformed stdin
    }

    if (isAntigravity) {
      // Antigravity PostToolUse contract requires an empty JSON object {}
      console.log(JSON.stringify({}));
      break;
    }

    const pulse = buildTailPulse(state);
    console.log(
      JSON.stringify({
        continue: true,
        additional_context: pulse,
        injectSteps: [{ systemMessage: { content: pulse } }]
      })
    );
    break;
  }

  case "hook-post-invocation": {
    const state = loadState();
    const pulse = buildTailPulse(state);
    console.log(
      JSON.stringify({
        injectSteps: [
          {
            ephemeralMessage: pulse
          }
        ]
      })
    );
    break;
  }

  default:
    console.log(
      "Commands: init | exec | checkpoint | fail | read-artifact | hook-pre-compact | hook-post-tool | hook-post-invocation"
    );
}
