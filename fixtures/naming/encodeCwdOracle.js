// Oracle for the `computed` cases in encodeCwd.json.
//
// Exact project-directory encoding of Claude Code CLI 2.1.232 and 2.1.255, transcribed from
// `strings` of both binaries on 2026-09-02 (2.1.232: yTo / xT / xAy / Ynt, Pre = 200;
// 2.1.255: k / PA / be / t4, rL = 200 — same bodies, different minified names):
//
//   yTo(e) = e.replace(/[^a-zA-Z0-9]/g, "-")
//   xT(e)  = { t = yTo(e); return t.length <= 200 ? t : `${t.slice(0, 200)}-${xAy(e)}` }
//   xAy(e) = Math.abs(Ynt(e)).toString(36)
//   Ynt(e) = { t = 0; for (r = 0; r < e.length; r++) t = (t << 5) - t + e.charCodeAt(r) | 0; return t }
//
// The CLI feeds NFC(realpath(cwd)) into xT (measured 2026-09-02: an NFD directory name arrives
// in the SessionStart stdin already in NFC and the directory is `…-caf-`). realpath is the
// caller's job; NFC is applied here so the oracle matches `vibememory_core::naming::encode_cwd`.
//
// Usage: node encodeCwdOracle.js '<cwd>' ['<cwd>' ...]   → one slug per line, same order.

const MAX_LEN = 200;

function hash(s) {
  let t = 0;
  for (let r = 0; r < s.length; r++) t = (t << 5) - t + s.charCodeAt(r) | 0;
  return t;
}

function encode(cwd) {
  const nfc = cwd.normalize("NFC");
  const slug = nfc.replace(/[^a-zA-Z0-9]/g, "-");
  if (slug.length <= MAX_LEN) return slug;
  return `${slug.slice(0, MAX_LEN)}-${Math.abs(hash(nfc)).toString(36)}`;
}

for (const cwd of process.argv.slice(2)) process.stdout.write(encode(cwd) + "\n");
