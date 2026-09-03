//! Desktop session cards: the guard that keeps their damage local and their meaning portable.
//!
//! Desktop stores one JSON descriptor per Code tab in a real local directory — it refuses
//! reparse points under its own root — so the cards travel through the outbox, where one machine
//! writes and the rest read. Two things make that more than a file copy: a card degrades on the
//! machine that cannot find its transcript ([`guard`]), and it names an absolute path of the
//! machine that wrote it ([`roots`]).

pub mod descriptor;
pub mod guard;
pub mod roots;

pub use descriptor::Descriptor;
pub use guard::{
    ExportVerdict, ImportVerdict, MachineFacts, SkipReason, WithholdReason, export_verdict,
    import_verdict, shadow_repair,
};
pub use roots::{RootError, Roots};
