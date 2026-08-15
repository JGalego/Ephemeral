// The desktop window.
//
// Everything is in the library so the commands can be unit-tested; this exists
// only to start it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ephemeral_desktop_lib::run();
}
