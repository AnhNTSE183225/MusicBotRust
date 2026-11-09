# INSTRUCTIONS.md — Autonomous Implementation Specification
**Target System:** Universal Continuous Context-Projection & Sandwich Harness (TypeScript / Windows)  
**Target Environments:** 
1. **Google Antigravity CLI (`agy`)** on Windows
2. **Enterprise Cursor IDE (No-MCP Mode)** on Windows  
**Deliverable Verification File:** `RESULTS.md`

---

## 0. MISSION BRIEF FOR THE AI AGENT
You are tasked with implementing a deterministic **TypeScript Agent State & Context-Projection Harness** that eliminates AI context rot, tool output bloat, and auto-compaction amnesia across long-running, single-session conversations.

### Core Architectural Rules You Are Building:
1. **The Context Window is Disposable:** The authoritative system of record is `.agent/state.json`, managed strictly by deterministic code—never edited directly by the LLM.
2. **The Sandwich Architecture (Continuous Single-Chat Mode):**
   - **Top Pin:** Every state change automatically compiles `.agent/state.json` into `.cursor/rules/00-projected-state.mdc` (`alwaysApply: true`) and `AGENTS.md`.
   - **Tail Pulse:** Every tool execution (`postToolUse` / `PostToolUse` / `exec`) appends a 5-line `[HARNESS TAIL PULSE]` at the bottom of the output to anchor recency bias.
   - **Compaction Shield:** When the IDE/CLI auto-summarizes a long conversation (`preCompact` or transcript size drop), the harness sets `justCompacted = true` and injects the full structured state ledger into the bottom of the very next turn.
3. **Tool Output Interceptor:** Any command run via the harness producing more than `2000` characters is automatically saved to `.agent/artifacts/` and replaced inline with a head/tail preview + artifact path.
4. **Hard Circuit Breaker:** 3 consecutive command failures on the same step lock execution (`exit code 2`) until the agent explicitly records the failure in the "Do Not Repeat" ledger (`fail` command).
5. **Write-Protection Guard:** Pre-tool hooks block any direct file-write tool from modifying `.agent/state.json`, `.agent/history.jsonl`, `.cursor/rules/00-projected-state.mdc`, or `AGENTS.md` (`exit code 2`).

### Strict Implementation Constraints:
- **No placeholders or `TODO`s:** Write 100% complete, production-ready code.
- **Windows Compatibility:** Normalize all file paths (`\` to `/`), handle non-TTY/empty `stdin` safely so PowerShell hooks never hang, and compile the `.ts` files into zero-dependency `.mjs` files so hooks execute in under 20ms via `node`.
- **Mandatory Verification:** You MUST execute all 7 verification tests in Phase 6, capture their exact outputs and exit codes, reset the workspace to a clean state, and write the full audit report to `RESULTS.md`.

---

## PHASE 1: DIRECTORY SCAFFOLDING
Create the following directories in the workspace root if they do not exist:
- `.harness/` (holds TypeScript source `.ts` and compiled `.mjs` runtime files)
- `.agent/` (holds `state.json` and `history.jsonl`)
- `.agent/artifacts/` (holds intercepted large command logs)
- `.cursor/rules/` (holds `00-projected-state.mdc`)
- `.agents/` (holds Antigravity CLI `hooks.json`)

---

## PHASE 2: CORE TYPESCRIPT HARNESS (`.harness/harness.ts`)
Create `.harness/harness.ts` with the exact TypeScript implementation below.

```typescript
// .harness/harness.ts
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

export interface ToolReceipt {
  ts: string;
  tool: string;
  status: "OK" | "ERROR";
  artifactRef?: string;
  preview: string;
}

export interface DecisionRecord {
  step: string;
  decision: string;
  why: string;
}

export interface FailureRecord {
  step: string;
  whatFailed: string;
  error: string;
}

export interface AgentState {
  goal: string;
  currentStep: string;
  retryCount: number;
  justCompacted: boolean;
  lastTranscriptSize: number;
  decisions: DecisionRecord[];
  failedAttempts: FailureRecord[];
  recentReceipts: ToolReceipt[];
  nextAction: string;
}

function normalizePath(p: string): string {
  return p.replace(/\\/g, "/");
}

function readStdinSafe(): string {
  try {
    if (process.stdin.isTTY) return "";
    return fs.readFileSync(0, "utf8");
  } catch {
    return "";
  }
}

function ensureDirs(): void {
  fs.mkdirSync(ARTIFACTS_DIR, { recursive: true });
  fs.mkdirSync(path.dirname(CURSOR_RULE_FILE), { recursive: true });
  if (!fs.existsSync(STATE_FILE)) {
    const initial: AgentState = {
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

function loadState(): AgentState {
  ensureDirs();
  return JSON.parse(fs.readFileSync(STATE_FILE, "utf8")) as AgentState;
}

function buildFullStateMarkdown(state: AgentState): string {
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

function buildTailPulse(state: AgentState): string {
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

function saveAndProjectState(state: AgentState): void {
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

function runGuardedCommand(cmd: string): void {
  const state = loadState();
  if (state.retryCount >= MAX_RETRIES_PER_STEP) {
    console.error(
      `[HARNESS CIRCUIT BREAKER] Step ${state.currentStep} hit ${state.retryCount}/${MAX_RETRIES_PER_STEP} failures. Execution is locked. You MUST call 'node .harness/harness.mjs fail "<what_was_tried>" "<exact_error>"' before running more commands.`
    );
    process.exit(2);
  }

  let rawOutput = "";
  let status: "OK" | "ERROR" = "OK";
  try {
    rawOutput = execSync(cmd, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  } catch (err: unknown) {
    status = "ERROR";
    const e = err as { stdout?: string; stderr?: string; message?: string };
    rawOutput = `${e.stdout || ""}\n${e.stderr || e.message || ""}`.trim();
    state.retryCount += 1;
  }

  let artifactRef: string | undefined;
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

function readArtifactSlice(relPath: string, startLineStr?: string, endLineStr?: string): void {
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
    try {
      const payload = JSON.parse(input || "{}");
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
        payload.toolInput || payload.tool_input || payload.file_path || payload.command || {}
      );
      state.recentReceipts.push({
        ts: new Date().toISOString(),
        tool: String(toolName),
        status: "OK",
        preview: rawPreview.slice(0, 100)
      });
      saveAndProjectState(state);
    } catch {
      // Ignore malformed stdin
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

  default:
    console.log(
      "Commands: init | exec | checkpoint | fail | read-artifact | hook-pre-compact | hook-post-tool"
    );
}
```

---

## PHASE 3: STATE WRITE-PROTECTION GUARD (`.harness/guard-write.ts`)
Create `.harness/guard-write.ts`. This script inspects `stdin` payloads from both Antigravity CLI (`PreToolUse`) and Cursor (`beforeShellExecution` / `preToolUse`) and returns **Exit Code 2** if the agent attempts to directly overwrite the protected state files instead of using the harness commands.

```typescript
// .harness/guard-write.ts
import fs from "node:fs";

function readStdinSafe(): string {
  try {
    if (process.stdin.isTTY) return "";
    return fs.readFileSync(0, "utf8");
  } catch {
    return "";
  }
}

const raw = readStdinSafe();
let data: Record<string, unknown> = {};
try {
  data = JSON.parse(raw || "{}");
} catch {
  data = { raw };
}

const serialized = JSON.stringify(data).toLowerCase().replace(/\\/g, "/");

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

console.log(JSON.stringify({ continue: true, permission: "allow" }));
```

---

## PHASE 4: COMPILE ZERO-DEPENDENCY `.mjs` RUNTIME FILES
To guarantee sub-20ms hook execution on Windows PowerShell without relying on `npx tsx` cold starts:
1. Create `.harness/harness.mjs` and `.harness/guard-write.mjs` by stripping TypeScript type annotations from `.harness/harness.ts` and `.harness/guard-write.ts` (either via `npx tsc` / `esbuild` or direct clean ES module output).
2. Verify both `.harness/harness.mjs` and `.harness/guard-write.mjs` run cleanly using standard `node` (`node .harness/harness.mjs` and `node .harness/guard-write.mjs`).

---

## PHASE 5: CONFIGURE HOOKS FOR ANTIGRAVITY CLI & ENTERPRISE CURSOR

### 5.1 Create `.agents/hooks.json` (Antigravity CLI on Windows)
```json
{
  "continuous-memory-pulse": {
    "PostToolUse": [
      {
        "matcher": "write_to_file|replace_file_content|multi_replace_file_content|run_command",
        "hooks": [
          {
            "type": "command",
            "command": "node .harness/harness.mjs hook-post-tool",
            "timeout": 10
          }
        ]
      }
    ]
  },
  "protect-state-files": {
    "PreToolUse": [
      {
        "matcher": "write_to_file|replace_file_content|multi_replace_file_content|run_command",
        "hooks": [
          {
            "type": "command",
            "command": "node .harness/guard-write.mjs",
            "timeout": 5
          }
        ]
      }
    ]
  }
}
```

### 5.2 Create `.cursor/hooks.json` (Enterprise Cursor IDE on Windows)
```json
{
  "version": 1,
  "hooks": {
    "preCompact": [
      {
        "command": "node .harness/harness.mjs hook-pre-compact",
        "timeout": 5
      }
    ],
    "postToolUse": [
      {
        "command": "node .harness/harness.mjs hook-post-tool",
        "timeout": 5
      }
    ],
    "afterFileEdit": [
      {
        "command": "node .harness/harness.mjs hook-post-tool",
        "timeout": 5
      }
    ],
    "beforeShellExecution": [
      {
        "command": "node .harness/guard-write.mjs",
        "timeout": 5,
        "failClosed": true
      }
    ],
    "preToolUse": [
      {
        "command": "node .harness/guard-write.mjs",
        "timeout": 5,
        "failClosed": true
      }
    ]
  }
}
```

### 5.3 Update `.gitignore`
Append `.agent/artifacts/` to `.gitignore` (create `.gitignore` if it does not exist) so large intercepted logs do not bloat Git commits, while keeping `.harness/`, `.agents/hooks.json`, and `.cursor/hooks.json` tracked.

---

## PHASE 6: MANDATORY 7-POINT SELF-VERIFICATION SUITE
You MUST execute the following 7 tests in the terminal and record their exact outputs and exit codes for `RESULTS.md`:

1. **Test 1 — Initialization & Projection:**
   Run: `node .harness/harness.mjs init "Verify Sandwich Harness" "Run verification suite"`
   - Verify `.agent/state.json`, `.cursor/rules/00-projected-state.mdc`, and `AGENTS.md` exist and contain `"Verify Sandwich Harness"`.
2. **Test 2 — Tool Output Interceptor (> 2,000 chars):**
   Run: `node .harness/harness.mjs exec "node -e \"console.log('X'.repeat(5000))\""`
   - Verify stdout prints `[HARNESS INTERCEPTED 5001 CHARS -> SAVED TO .agent/artifacts/cmd_...]` and a `.log` file is created inside `.agent/artifacts/`.
3. **Test 3 — Artifact Slice Reader:**
   Run: `node .harness/harness.mjs read-artifact "<path_to_artifact_from_test_2>" 1 5`
   - Verify it prints the requested line slice cleanly.
4. **Test 4 — 3-Strike Circuit Breaker:**
   Run a failing command 3 times: `node .harness/harness.mjs exec "node -e \"process.exit(1)\""` (3 times), then run it a **4th time**.
   - Verify the 4th attempt is blocked before execution with `[HARNESS CIRCUIT BREAKER]` and returns **Exit Code 2**.
5. **Test 5 — Failure Ledger Recovery & Step Checkpoint:**
   - Run: `node .harness/harness.mjs fail "Simulated 3 failing runs" "Exit code 1"` (verify `retryCount` resets to `0` and `failedAttempts` updates in `00-projected-state.mdc`).
   - Run: `node .harness/harness.mjs checkpoint "step_02" "Verified circuit breaker" "All 4 runs behaved deterministically" "Test write guard and compaction"` (verify `.agent/history.jsonl` has 1 line and `currentStep` is `"step_02"`).
6. **Test 6 — State Write-Protection Guard (`exit code 2`):**
   - Pipe a simulated unauthorized write payload into `guard-write.mjs`:
     `node -e "const {spawnSync}=require('child_process'); const r=spawnSync('node',['.harness/guard-write.mjs'],{input:JSON.stringify({TargetFile:'.agent/state.json'}),encoding:'utf8'}); console.log('EXIT:',r.status,'STDOUT:',r.stdout,'STDERR:',r.stderr);"`
   - Verify it returns `EXIT: 2` and `DENIED BY HARNESS`.
7. **Test 7 — Auto-Compaction Shield (`hook-pre-compact` + `hook-post-tool`):**
   - Run: `node .harness/harness.mjs hook-pre-compact` (verify `justCompacted` becomes `true` in `.agent/state.json`).
   - Run: `node -e "const {spawnSync}=require('child_process'); const r=spawnSync('node',['.harness/harness.mjs','hook-post-tool'],{input:JSON.stringify({toolName:'view_file'}),encoding:'utf8'}); console.log(r.stdout);"`
   - Verify stdout JSON contains `[HARNESS COMPACTION SHIELD: Conversation was just auto-summarized...]` and `justCompacted` resets to `false`.

---

## PHASE 7: CLEAN RESET & GENERATE `RESULTS.md`
1. After all 7 tests pass, delete temporary test files inside `.agent/artifacts/` and `.agent/history.jsonl`, then run:
   `node .harness/harness.mjs init "Ready for Production" "Await user task"`
   so the repository starts clean.
2. Create **`RESULTS.md`** in the workspace root using the exact structure below:
   - **Section 1: Environment Metadata** (OS version, Node.js version, working directory, timestamp).
   - **Section 2: Created Files Inventory** (Table of every created file, its exact byte size, and SHA-256 or MD5 checksum).
   - **Section 3: 7-Point Verification Suite Audit Log** (For each of the 7 tests: the exact command executed, the exit code returned, and the raw stdout/stderr output).
   - **Section 4: Final Projected State Snapshot** (The exact final contents of `.cursor/rules/00-projected-state.mdc` and `.agent/state.json`).