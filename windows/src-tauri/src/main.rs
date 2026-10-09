// Fedo runs without a console window: the ghost is the whole UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    fedo_lib::run()
}
