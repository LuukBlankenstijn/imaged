//! Routed top-level views. Skeleton implementations; the real UIs are built per
//! view against the server functions in `crate::api` and the DTOs in
//! `crate::model`.

pub(crate) mod groups;
mod hosts;
mod images;
mod tasks;

pub use groups::Groups;
pub use hosts::Hosts;
pub use images::Images;
pub use tasks::Tasks;
