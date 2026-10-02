//! Shared command-deck palette: navy metal, pale labels, amber selection.
use bevy::prelude::*;

pub const PANEL: Color = Color::srgba(0.025, 0.041, 0.070, 0.97);
pub const FRAME: Color = Color::srgb(0.25, 0.34, 0.45);
pub const BUTTON: Color = Color::srgb(0.075, 0.105, 0.16);
pub const HOVER: Color = Color::srgb(0.15, 0.24, 0.34);
pub const ACTIVE: Color = Color::srgb(0.065, 0.23, 0.36);
pub const PRESSED: Color = Color::srgb(0.20, 0.34, 0.46);
pub const TEXT: Color = Color::srgb(0.87, 0.92, 0.96);
pub const MUTED: Color = Color::srgb(0.53, 0.67, 0.79);
pub const ACCENT: Color = Color::srgb(0.96, 0.80, 0.46);

pub fn button_background(interaction: Interaction, active: bool) -> Color {
    match interaction {
        Interaction::Pressed => PRESSED,
        Interaction::Hovered => HOVER,
        Interaction::None if active => ACTIVE,
        Interaction::None => BUTTON,
    }
}
