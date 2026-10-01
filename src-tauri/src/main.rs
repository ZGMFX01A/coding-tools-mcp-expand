// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    match coding_tools_mcp_desktop_lib::cli::run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(true) => return,
        Ok(false) => {},
        Err(message) => { eprintln!("{message}"); std::process::exit(2); }
    }
    coding_tools_mcp_desktop_lib::run()
}
