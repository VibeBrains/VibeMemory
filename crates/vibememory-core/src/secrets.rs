//! Keys that must not be written into a memory, a rule or a skill: what a key looks like, and which kind it is.
//!
//! A memory, a rule and a skill are shared and kept in the store's history for good, and a key in one of them
//! would open an account to whoever reads the store. The scan knows the shapes of the keys people paste most often
//! and returns the kind of the first it finds. It never returns the value, so a message built from the answer
//! cannot carry the key onward.
//!
//! A shape is a prefix and a run of key characters, with a start that is not the middle of a longer word: `sk-learn`
//! is a library, `task-force-management-system` is prose, and `AKIA` inside a long identifier is neither a key. The
//! lengths are the least a real key has, kept in constants so that a vendor changing its format is one line here.

use std::fmt;

/// Which kind of key a text holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// `sk-…` of `OpenAI`.
    OpenAiKey,
    /// `sk-ant-…` of Anthropic.
    AnthropicKey,
    /// `AKIA…` or `ASIA…` of AWS.
    AwsAccessKey,
    /// `ghp_…`, `gho_…`, `ghs_…`, `ghu_…`, `ghr_…` or `github_pat_…` of `GitHub`.
    GithubToken,
    /// `xoxb-…` and the other `xox` tokens of Slack.
    SlackToken,
    /// `AIza…` of Google.
    GoogleApiKey,
    /// `sk_live_…` or `rk_live_…` of Stripe.
    StripeKey,
    /// A `-----BEGIN … PRIVATE KEY-----` header.
    PrivateKey,
    /// `vmt_<id>_<secret>` of a cabinet (`crate::token`).
    VibeMemoryToken,
}

impl SecretKind {
    /// What a person is told the text holds.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::OpenAiKey => "OpenAI API key",
            Self::AnthropicKey => "Anthropic API key",
            Self::AwsAccessKey => "AWS access key",
            Self::GithubToken => "GitHub token",
            Self::SlackToken => "Slack token",
            Self::GoogleApiKey => "Google API key",
            Self::StripeKey => "Stripe live key",
            Self::PrivateKey => "private key",
            Self::VibeMemoryToken => "VibeMemory agent token (vmt_\u{2026})",
        }
    }

    /// Stable camelCase code of the kind, for fixtures and machine-readable answers.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::OpenAiKey => "openAiKey",
            Self::AnthropicKey => "anthropicKey",
            Self::AwsAccessKey => "awsAccessKey",
            Self::GithubToken => "githubToken",
            Self::SlackToken => "slackToken",
            Self::GoogleApiKey => "googleApiKey",
            Self::StripeKey => "stripeKey",
            Self::PrivateKey => "privateKey",
            Self::VibeMemoryToken => "vibeMemoryToken",
        }
    }
}

impl fmt::Display for SecretKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Key characters after an `sk-` prefix, at least: letters, digits, `-` and `_` (`sk-proj-…` has all of them).
const OPENAI_MIN: usize = 20;

/// Key characters after `sk-ant-`, at least: real ones run to about a hundred.
const ANTHROPIC_MIN: usize = 20;

/// Characters after `AKIA` or `ASIA`, exactly: a longer run is a longer word, not a key.
const AWS_BODY: usize = 16;

/// Characters after `ghp_` and its siblings, at least.
const GITHUB_MIN: usize = 36;

/// Characters after `github_pat_`, at least: a real one has seventy-one.
const GITHUB_PAT_MIN: usize = 40;

/// Characters after `xoxb-` and its siblings, at least.
const SLACK_MIN: usize = 10;

/// Characters after `AIza`, exactly.
const GOOGLE_BODY: usize = 35;

/// Characters after `sk_live_` and `rk_live_`, at least.
const STRIPE_MIN: usize = 16;

/// The longest label between `-----BEGIN ` and the closing dashes that is looked at.
const PEM_LABEL_MAX: usize = 40;

const OPENAI_PREFIX: &[u8] = b"sk-";
const ANTHROPIC_PREFIX: &[u8] = b"sk-ant-";
const AWS_PREFIXES: [&[u8]; 2] = [b"AKIA", b"ASIA"];
const GITHUB_PREFIXES: [&[u8]; 5] = [b"ghp_", b"gho_", b"ghs_", b"ghu_", b"ghr_"];
const GITHUB_PAT_PREFIX: &[u8] = b"github_pat_";
const SLACK_PREFIX: &[u8] = b"xox";
const SLACK_SORTS: &[u8] = b"abprs";
const GOOGLE_PREFIX: &[u8] = b"AIza";
const STRIPE_PREFIXES: [&[u8]; 2] = [b"sk_live_", b"rk_live_"];
const PEM_OPEN: &[u8] = b"-----BEGIN ";
const PEM_CLOSE: &[u8] = b"-----";
const PEM_LABEL_END: &[u8] = b"PRIVATE KEY";

/// The kind of the first key `text` holds, `None` when it holds none.
///
/// The value is not returned, and not to be reconstructed from the answer.
#[must_use]
pub fn find(text: &str) -> Option<SecretKind> {
    let bytes = text.as_bytes();
    (0..bytes.len())
        .find_map(|at| shaped_at(bytes, at))
        .or_else(|| crate::token::holds_token(bytes).then_some(SecretKind::VibeMemoryToken))
}

/// The kind of the first key in any of `texts`.
#[must_use]
pub fn find_in<'a>(texts: impl IntoIterator<Item = &'a str>) -> Option<SecretKind> {
    texts.into_iter().find_map(find)
}

/// The key that starts at `at`, if one does.
fn shaped_at(bytes: &[u8], at: usize) -> Option<SecretKind> {
    let rest = bytes.get(at..)?;
    if private_key(rest) {
        return Some(SecretKind::PrivateKey);
    }
    // A key is not the tail of a longer word
    let inside_word = at
        .checked_sub(1)
        .and_then(|before| bytes.get(before))
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_');
    if inside_word {
        return None;
    }
    prefixed(rest)
}

/// Letters, digits, `-` and `_`.
fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

/// Letters and digits.
fn is_base62(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

/// Uppercase letters and digits.
fn is_upper_or_digit(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}

/// Letters, digits and `-`.
fn is_slack_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

/// How many bytes from the start of `bytes` are `accept`ed.
fn run(bytes: &[u8], accept: fn(u8) -> bool) -> usize {
    bytes.iter().take_while(|byte| accept(**byte)).count()
}

/// The part of `rest` after `prefix`, when it starts with it.
fn after<'a>(rest: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    rest.strip_prefix(prefix)
}

/// The kind of a key with a known prefix that `rest` starts with.
fn prefixed(rest: &[u8]) -> Option<SecretKind> {
    if after(rest, ANTHROPIC_PREFIX).is_some_and(|body| run(body, is_key_char) >= ANTHROPIC_MIN) {
        return Some(SecretKind::AnthropicKey);
    }
    // Real keys are random: a hyphenated phrase of twenty letters (`sk-learn-compatible-…`) has no digit in it
    if after(rest, OPENAI_PREFIX).is_some_and(|body| {
        let length = run(body, is_key_char);
        length >= OPENAI_MIN
            && body
                .get(..length)
                .is_some_and(|key| key.iter().any(u8::is_ascii_digit))
    }) {
        return Some(SecretKind::OpenAiKey);
    }
    if AWS_PREFIXES
        .iter()
        .filter_map(|prefix| after(rest, prefix))
        .any(|body| run(body, is_upper_or_digit) == AWS_BODY && !continues_word(body, AWS_BODY))
    {
        return Some(SecretKind::AwsAccessKey);
    }
    let github = GITHUB_PREFIXES
        .iter()
        .filter_map(|prefix| after(rest, prefix))
        .any(|body| run(body, is_base62) >= GITHUB_MIN);
    let github_pat =
        after(rest, GITHUB_PAT_PREFIX).is_some_and(|body| run(body, is_key_char) >= GITHUB_PAT_MIN);
    if github || github_pat {
        return Some(SecretKind::GithubToken);
    }
    if slack(rest) {
        return Some(SecretKind::SlackToken);
    }
    if after(rest, GOOGLE_PREFIX).is_some_and(|body| run(body, is_key_char) == GOOGLE_BODY) {
        return Some(SecretKind::GoogleApiKey);
    }
    if STRIPE_PREFIXES
        .iter()
        .filter_map(|prefix| after(rest, prefix))
        .any(|body| run(body, is_base62) >= STRIPE_MIN)
    {
        return Some(SecretKind::StripeKey);
    }
    None
}

/// Whether the byte after the first `length` bytes of `body` goes on with a word.
fn continues_word(body: &[u8], length: usize) -> bool {
    body.get(length)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

/// Whether `rest` starts with `xoxb-…` or one of its siblings.
fn slack(rest: &[u8]) -> bool {
    let Some(body) = after(rest, SLACK_PREFIX) else {
        return false;
    };
    let sort_and_dash = (body.first(), body.get(1));
    matches!(sort_and_dash, (Some(sort), Some(b'-')) if SLACK_SORTS.contains(sort))
        && body
            .get(2..)
            .is_some_and(|key| run(key, is_slack_char) >= SLACK_MIN)
}

/// Whether `rest` starts with the header of a PEM private key: `-----BEGIN RSA PRIVATE KEY-----` and its kin.
fn private_key(rest: &[u8]) -> bool {
    let Some(after_open) = after(rest, PEM_OPEN) else {
        return false;
    };
    let window = after_open
        .get(..PEM_LABEL_MAX.min(after_open.len()))
        .unwrap_or_default();
    window
        .windows(PEM_CLOSE.len())
        .position(|candidate| candidate == PEM_CLOSE)
        .and_then(|end| window.get(..end))
        .is_some_and(|label| !label.contains(&b'\n') && label.ends_with(PEM_LABEL_END))
}
