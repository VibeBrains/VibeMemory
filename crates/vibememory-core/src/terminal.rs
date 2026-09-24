//! Text from outside on its way to a terminal or a journal line.

/// `text` with every control character escaped: nothing that arrived from outside — a field of
/// a request, an answer from a cabinet, a file another program wrote — can steer a terminal or
/// start a journal line of its own.
#[must_use]
pub fn printable(text: &str) -> String {
    text.chars()
        .flat_map(|character| {
            if character.is_control() {
                character.escape_default().collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect()
}
