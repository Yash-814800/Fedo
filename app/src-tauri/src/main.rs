// Fedo runs without a console window: the ghost is the whole UI.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // `fedo stats` reads /proc and exits: it must not start a second app.
    if std::env::args().nth(1).as_deref() == Some("stats") {
        std::process::exit(fedo_lib::stats());
    }
    fedo_lib::run()
}
