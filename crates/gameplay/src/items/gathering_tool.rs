//! Subcategory of a gathering tool. Resources list the kinds that grant
//! bonuses on that node; any equipped item of a listed kind applies.

use serde::{Deserialize, Serialize};

/// Kind of gathering tool. Match key between an item and a resource node.
///
/// Oak trees list [`GatheringToolKind::Axe`]; copper veins list
/// [`GatheringToolKind::Hammer`]. A later iron axe with the same kind works
/// on oak without touching the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GatheringToolKind {
    Axe,
    Hammer,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_distinct() {
        assert_ne!(GatheringToolKind::Axe, GatheringToolKind::Hammer);
    }
}
