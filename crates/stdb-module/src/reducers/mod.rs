//! Everything a client is allowed to ask the server to do.
//!
//! One module per area, mirroring the command messages the lightyear protocol
//! used to carry. Each reducer resolves its caller with `ctx.sender()` — there
//! is no entity to spoof, which removes the whole class of check the Bevy
//! handlers needed.

pub mod account;
pub mod api_keys;
pub mod chat;
pub mod combat;
pub mod crafting;
pub mod economy;
pub mod gathering;
pub mod items;
pub mod loot;
pub mod lifecycle;
pub mod market;
pub mod movement;
pub mod parties;
pub mod resonance;
pub mod spells;
