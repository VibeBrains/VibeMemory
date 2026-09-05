//! The manifest hash against NIST's own vectors: a home-grown SHA-256 that is wrong is worse than
//! none, because it would certify a truncated transcript as whole.

#![allow(clippy::panic, clippy::unwrap_used, clippy::expect_used)]

use vibememory_cli::sha256::hex;

#[test]
fn nist_vectors_match_exactly() {
    // FIPS 180-4 examples and the empty message.
    assert_eq!(
        hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn a_million_a_is_the_long_vector() {
    // The one that catches a padding bug at a block boundary.
    let input = vec![b'a'; 1_000_000];
    assert_eq!(
        hex(&input),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn lengths_around_the_block_boundary_are_all_different() {
    // 55, 56, 63, 64, 65 bytes exercise every padding branch; a bug would make two collide.
    let mut seen = std::collections::BTreeSet::new();
    for len in [0usize, 1, 55, 56, 57, 63, 64, 65, 119, 120, 128] {
        assert!(seen.insert(hex(&vec![b'x'; len])), "collision at {len}");
    }
}
