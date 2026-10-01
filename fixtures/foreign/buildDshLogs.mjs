// Builds fixtures/foreign/dshLogs.json: logs of DeepSeek Harness made of real zstd frames, and what
// the engine must make of each.
//
// The records are synthetic; the frames are compressed by Node's own zstd, the library DSH itself
// writes its logs with, so the engine's decoder is tested against another implementation's bytes.
// The expected lines come from the translation below, written apart from the engine's: it is the
// table DSH's own wrapper used, so a session read by the engine and one handed over by the wrapper
// land as the same records.
//
// Usage: node fixtures/foreign/buildDshLogs.mjs > fixtures/foreign/dshLogs.json

import zlib from 'node:zlib';

const SESSION = 'session-11111111-1111-4111-8111-111111111111';
const CWD = '/work/Promed';
const T0 = 1790000000000;

const header = (version = 4) => ({
    type: 'session', version, id: SESSION, cwd: CWD, agentPreset: 'default',
    createdAt: T0, delegationDepth: 0, isSeeded: false,
});
const user = (seq, id, text) => ({
    type: 'user/message', seq, time: T0 + seq * 1000,
    data: { id, role: 'user', source: 'input', content: [{ type: 'text', text }] },
});
const assistant = (seq, id, text) => ({
    type: 'assistant/message', seq, time: T0 + seq * 1000 + 7,
    data: { message: { id, role: 'assistant', content: [
        { type: 'reasoning', text: 'thinking it over' },
        { type: 'text', text },
    ] } },
});
const toolCall = (seq) => ({
    type: 'tool/call', seq, time: T0 + seq * 1000,
    data: { name: 'shell', input: { command: 'ls' } },
});

const lines = (records) => records.map((record) => `${JSON.stringify(record)}\n`).join('');
const frame = (text) => zlib.zstdCompressSync(Buffer.from(text, 'utf8'));

/** A log: each element is one frame, given as records or as raw text. */
function log(frames, { cutLast = false } = {}) {
    const parts = frames.map((part) => frame(typeof part === 'string' ? part : lines(part)));
    if (cutLast) {
        const last = parts.pop();
        parts.push(last.subarray(0, Math.floor(last.length / 2)));
    }
    return Buffer.concat(parts);
}

/** The translation of DSH's wrapper, with ids kept unique the way the engine keeps them. */
function translate(records) {
    const used = new Set();
    const out = [];
    records.slice(1).forEach((record, index) => {
        if (typeof record.type !== 'string' || !record.type || !Number.isFinite(record.time)) {
            return;
        }
        const timestamp = new Date(Math.trunc(record.time)).toISOString();
        const fallback = `${SESSION}-${record.seq ?? `i${index}`}`;
        let line;
        let own = null;
        if (record.type === 'user/message') {
            line = { type: 'user', timestamp, message: { role: 'user', content: record.data?.content ?? '' } };
            own = record.data?.id || null;
        } else if (record.type === 'assistant/message') {
            line = { type: 'assistant', timestamp, message: { role: 'assistant', content: record.data?.message?.content ?? [] } };
            own = record.data?.message?.id || null;
        } else {
            line = { type: record.type, timestamp, data: record.data ?? null };
        }
        const uuid = [own, fallback].find((candidate) => candidate && !used.has(candidate)) ?? `${fallback}-${index}`;
        used.add(uuid);
        out.push(JSON.parse(JSON.stringify({ ...line, uuid }).replace(/vmt_[0-9a-hjkmnp-tv-z]{8}_[^\s"'\\]*/g, '<token>')));
    });
    return out;
}

const conversation = [header(), user(1, 'u-1', 'list the files'), assistant(2, 'a-1', 'here they are'), toolCall(3)];

const cases = [
    {
        id: 'framesJoined',
        note: 'A log is many frames; their records join in order, and the header gives the id and the directory',
        log: log([conversation.slice(0, 3), conversation.slice(3)]),
        expect: { session: SESSION, cwd: CWD, lines: translate(conversation) },
    },
    {
        id: 'lastFrameCut',
        note: 'The last frame of a live log is still being written: it is dropped, and the next look takes it whole',
        log: log([conversation, [user(4, 'u-2', 'and the hidden ones')]], { cutLast: true }),
        expect: { session: SESSION, cwd: CWD, lines: translate(conversation) },
    },
    {
        id: 'lastLineCut',
        note: 'A frame that ends in the middle of a line: the cut line is dropped',
        log: log([lines(conversation) + JSON.stringify(user(4, 'u-2', 'cut')).slice(0, 40)]),
        expect: { session: SESSION, cwd: CWD, lines: translate(conversation) },
    },
    {
        id: 'tokenCutOut',
        note: 'A token of ours caught in the log is cut out: a file holding one would stay on this machine',
        log: log([[header(), user(1, 'u-1', 'connect with vmt_abcdefgh_1111111111 please')]]),
        expect: { session: SESSION, cwd: CWD, lines: translate([header(), user(1, 'u-1', 'connect with vmt_abcdefgh_1111111111 please')]) },
    },
    {
        id: 'repeatedIdAndNoSeq',
        note: 'Two messages with one id, and a record without seq: every line still gets an id of its own',
        log: log([[header(), assistant(1, 'a-1', 'one'), assistant(2, 'a-1', 'two'), { type: 'tool/result', time: T0 + 9000, data: { ok: true } }]]),
        expect: {
            session: SESSION, cwd: CWD,
            lines: translate([header(), assistant(1, 'a-1', 'one'), assistant(2, 'a-1', 'two'), { type: 'tool/result', time: T0 + 9000, data: { ok: true } }]),
        },
    },
    {
        id: 'recordWithoutTime',
        note: 'A record without a time cannot be placed by the merge and is left out',
        log: log([[header(), { type: 'session/meta', seq: 1, data: {} }, user(2, 'u-1', 'hello')]]),
        expect: { session: SESSION, cwd: CWD, lines: translate([header(), { type: 'session/meta', seq: 1, data: {} }, user(2, 'u-1', 'hello')]) },
    },
    {
        id: 'unknownVersion',
        note: 'A version of the format this engine was not written for is refused by name, never guessed at',
        log: log([[header(5), user(1, 'u-1', 'hello')]]),
        expect: { problem: 'unknownVersion', version: 5 },
    },
    {
        id: 'emptyLog',
        note: 'A session that has just begun has written nothing: not a failure, nothing to hand over yet',
        log: Buffer.alloc(0),
        expect: { problem: 'empty' },
    },
    {
        id: 'headerOnly',
        note: 'Only the header: still nothing to hand over',
        log: log([[header()]]),
        expect: { problem: 'empty' },
    },
    {
        id: 'noHeader',
        note: 'A log whose first record is not the header is not a log of this format',
        log: log([[user(1, 'u-1', 'hello')]]),
        expect: { problem: 'noHeader' },
    },
    {
        id: 'damagedLine',
        note: 'A line that is not JSON before the last one: the log is damaged, not merely being written',
        log: log([`${JSON.stringify(header())}\n{not json\n${JSON.stringify(user(2, 'u-1', 'hello'))}\n`]),
        expect: { problem: 'record', line: 2 },
    },
];

const logNames = [
    { name: 'session.v4.jsonl.zstd', expect: true },
    { name: 'session.v5.jsonl.zstd', expect: true },
    { name: 'session.v12.jsonl.zstd', expect: true },
    { name: 'session.lock', expect: false },
    { name: 'session.v.jsonl.zstd', expect: false },
    { name: 'session.vx.jsonl.zstd', expect: false },
    { name: 'session.v4.jsonl', expect: false },
];

process.stdout.write(`${JSON.stringify({
    description: 'Logs of DeepSeek Harness as zstd frames compressed by Node, and the session the engine makes of each. Built by fixtures/foreign/buildDshLogs.mjs; format: docs/manuals/foreignSessionSpec.md',
    cases: cases.map(({ log: bytes, ...rest }) => ({ ...rest, log: bytes.toString('base64') })),
    logNames,
}, null, 2)}\n`);
