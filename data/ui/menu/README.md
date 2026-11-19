# Main menu assets

`perimeter.rgba`: 1672 × 941, RGBA8 sRGB, row-major, no header. Generated for
this project from the user's optical-layer moodboard on 2026-11-10. This is
background scenery only; text, selection, frames and controls are native UI.
Source generation: `exec-b5b058e4-2b0b-41d2-988d-2fffea15b410.png`.

`Oswald.ttf`: unmodified variable font from
https://github.com/google/fonts/tree/main/ofl/oswald . The OFL license is
included as `OFL-Oswald.txt`. Cyrillic is included. Font and image are embedded
in the executable; runtime downloads and installed system fonts are not needed
for the main menu.

`Handjet.ttf`: unmodified variable font from
https://github.com/google/fonts/tree/main/ofl/handjet . Its Cyrillic dot-matrix
readouts are used for object titles, resource values and queue telemetry in
the compact optical controls. License: `OFL-Handjet.txt`; downloaded 2026-11-13.
Commands keep the existing narrow Oswald face. Both are embedded; no runtime
font fetch or host font installation is required for the object controls.

Frame marks and selection arrows reuse the existing `data/ui/icons/plus.rgba`
and `chevron-right.rgba` 40 × 40 icons; these are native UI overlays.
