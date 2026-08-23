//! Screen-space Crowd Control bar projected above stunned, rooted or silenced entities.

pub mod components;
mod systems;

use bevy::prelude::*;

pub struct CrowdControlBarPlugin;

impl Plugin for CrowdControlBarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                systems::sync_screen_cc_bars,
                // See `RenderSync`: the projection must see this frame's camera
                // and target transforms, or the bar swims while the player walks.
                systems::update_screen_cc_bars.in_set(crate::renderer::RenderSync::Project),
            )
                .chain()
                .run_if(crate::game_state::in_gameplay),
        );
        app.add_systems(
            Update,
            systems::cleanup_screen_cc_bars.run_if(crate::game_state::not_in_gameplay),
        );
    }
}
