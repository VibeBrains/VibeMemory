//! The binary. Everything it does lives in the library next to it.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    clippy::disallowed_macros
)]

fn main() {
    println!("vibememory {}", env!("CARGO_PKG_VERSION"));
}
