//! Merging the files two machines share. Each shared format has its own driver with its own
//! rules and its own report: transcripts here, `memory/*.md` (keep-both) beside it later. Pure:
//! bytes in, bytes and a report out; the caller does the I/O.

pub mod jsonl;
