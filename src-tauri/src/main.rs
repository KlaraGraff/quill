// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // These subcommands run before Tauri initializes. `mcp` serves AI
    // clients over stdio; `pdfium-smoke` lets release CI exercise the
    // signed bundle's real dynamic-library policy. All other arguments
    // fall through to the normal app launch.
    let mut args = std::env::args();
    let _exe = args.next();
    match args.next().as_deref() {
        Some("mcp") => {
            quill_lib::mcp_stdio_main();
            return;
        }
        Some("pdfium-smoke") => {
            if let Err(error) = quill_lib::pdfium_smoke_test() {
                eprintln!("quill pdfium-smoke: {error}");
                std::process::exit(1);
            }
            println!("quill pdfium-smoke: ok");
            return;
        }
        _ => {}
    }
    quill_lib::run()
}
