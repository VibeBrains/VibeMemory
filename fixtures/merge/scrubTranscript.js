// Prepares a transcript fixture from a real Claude Code JSONL file.
//
// Whitelist, not blacklist: a value survives only if its key is listed below or it is not a
// string at all. Identifiers keep their shape but are rehashed, so equality and the parentUuid
// graph stay intact while the owner's real ids do not. Everything else becomes "<scrubbed>".
//
// Node is the tool of choice because the CLI writes transcripts with JSON.stringify: the same
// serializer reproduces key order and escaping byte for byte, and the round-trip probe below
// (JSON.stringify(JSON.parse(line)) === line) is only meaningful with it.
//
// Usage: node fixtures/merge/scrubTranscript.js <source.jsonl> [--lines a..b] > <fixture.jsonl>
//        (a..b is a half-open range of 0-based line numbers; a manifest goes to stderr)

const { readFileSync } = require("node:fs");
const { createHash } = require("node:crypto");

/// Values of these keys are recorded as they are: they drive merge rules or classify a record,
/// and none of them carries the owner's content.
const KEEP = new Set([
  "type", "subtype", "operation", "role", "model", "version", "userType", "entrypoint",
  "permissionMode", "origin", "promptSource", "level", "mode", "stop_reason", "stop_sequence",
  "name", "agentType", "effort", "atis", "timestamp", "service_tier", "status", "reason",
]);
/// Keys whose values are identifiers: rehashed, never kept, never blanked.
const ID_KEYS = new Set([
  "uuid", "parentUuid", "leafUuid", "sessionId", "promptId", "toolUseID", "tool_use_id",
  "agentId", "requestId", "id", "key", "bridgeSessionId", "sourceToolAssistantUUID",
  "startUuid", "rewindAnchorUuid", "messageId",
]);
/// Working directories are replaced by the one scratch path fixtures are allowed to name.
const CWD_KEYS = new Set(["cwd", "originCwd", "projectRoot", "relocatedCwd", "path", "file_path"]);

const SCRUBBED = "<scrubbed>";
const SCRATCH_CWD = "/private/tmp/scratch/work";
const SYNTHETIC_SESSION_ID = "11111111-1111-4111-8111-111111111111";
const SYNTHETIC_ACCOUNT_UUID = "22222222-2222-4222-8222-222222222222";
const SYNTHETIC_ORGANIZATION_UUID = "33333333-3333-4333-8333-333333333333";
const SALT = "vibememory-fixture:";

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const HEX_ID_RE = /^[0-9a-f]{8,}$/i;
const PREFIXED_ID_RE = /^(toolu_|req_|msg_|cse_|call_|wf_|agent_)([A-Za-z0-9]+)$/;
const JOURNAL_KEY_RE = /^v2:[0-9a-f]{64}$/;

function digest(value) {
  return createHash("sha256").update(SALT + value).digest("hex");
}

/// Same shape, different value: a UUID stays a v4-looking UUID, a hex id keeps its length.
function rehash(value) {
  if (UUID_RE.test(value)) {
    const h = digest(value);
    return `${h.slice(0, 8)}-${h.slice(8, 12)}-4${h.slice(13, 16)}-8${h.slice(17, 20)}-${h.slice(20, 32)}`;
  }
  if (JOURNAL_KEY_RE.test(value)) return `v2:${digest(value)}`;
  const prefixed = PREFIXED_ID_RE.exec(value);
  if (prefixed) return prefixed[1] + digest(value).slice(0, prefixed[2].length);
  if (HEX_ID_RE.test(value)) return digest(value).slice(0, value.length);
  return null;
}

function scrubString(key, value) {
  if (key === "sessionId") return SYNTHETIC_SESSION_ID;
  if (key === "ownerAccountUuid") return SYNTHETIC_ACCOUNT_UUID;
  if (key === "ownerOrganizationUuid") return SYNTHETIC_ORGANIZATION_UUID;
  if (CWD_KEYS.has(key)) return SCRATCH_CWD;
  if (ID_KEYS.has(key)) return rehash(value) ?? SCRUBBED;
  if (KEEP.has(key)) return value;
  // An identifier can also appear as a plain value (a tool_use id inside content, a journal key).
  return rehash(value) ?? SCRUBBED;
}

function scrub(node, key) {
  if (typeof node === "string") return scrubString(key, node);
  if (Array.isArray(node)) return node.map((item) => scrub(item, key));
  if (node && typeof node === "object") {
    const out = {};
    for (const [k, v] of Object.entries(node)) out[k] = scrub(v, k);
    return out;
  }
  return node; // numbers, booleans, null keep their meaning and cost nothing to keep
}

function main(argv) {
  const args = argv.slice(2);
  const source = args.find((a) => !a.startsWith("--"));
  if (!source) {
    process.stderr.write("usage: node scrubTranscript.js <source.jsonl> [--lines a..b]\n");
    process.exit(2);
  }
  const range = args.find((a) => a.startsWith("--lines="))?.slice("--lines=".length)
    ?? (args.includes("--lines") ? args[args.indexOf("--lines") + 1] : undefined);
  const raw = readFileSync(source);
  const text = raw.toString("utf8");
  const torn = text.length > 0 && !text.endsWith("\n");
  let lines = text.split("\n");
  lines.pop(); // the piece after the last "\n": either empty or a torn tail
  const [from, to] = range ? range.split("..").map(Number) : [0, lines.length];
  lines = lines.slice(from, to);

  const out = [];
  const types = new Map();
  for (const [index, line] of lines.entries()) {
    if (line === "") { out.push(""); continue; }
    let parsed;
    try {
      parsed = JSON.parse(line);
    } catch {
      out.push(line); // not JSON: kept byte for byte, exactly as the merge does
      continue;
    }
    if (JSON.stringify(parsed) !== line) {
      process.stderr.write(`line ${from + index}: JSON.stringify round-trip differs — the writer is not JSON.stringify any more, refusing\n`);
      process.exit(1);
    }
    types.set(parsed.type, (types.get(parsed.type) ?? 0) + 1);
    out.push(JSON.stringify(scrub(parsed)));
  }
  const body = out.map((line) => line + "\n").join("");
  process.stdout.write(body);
  process.stderr.write(JSON.stringify({
    source, lines: [from, to], linesWritten: out.length, bytesIn: raw.length,
    bytesOut: Buffer.byteLength(body), tornTailDropped: torn,
    types: Object.fromEntries([...types].sort()),
  }, null, 2) + "\n");
}

main(process.argv);
