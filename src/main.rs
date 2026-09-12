#![allow(unsafe_op_in_unsafe_fn)]

mod core;
mod live_tests;
mod ui;
mod uia;

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--validate-fixture") {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        let path = args
            .get(2)
            .map(std::path::Path::new)
            .unwrap_or_else(|| std::path::Path::new("work/fixture"));
        if let Err(e) = live_tests::run(path) {
            eprintln!("Live validation failed: {e}");
            std::process::exit(1);
        }
    } else if args.get(1).is_some_and(|s| s == "--probe") {
        unsafe {
            let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
                windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            );
        }
        uia::probe(args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10));
    } else if let Err(e) = ui::run(if args.get(1).is_some_and(|s| s == "--ui-smoke") {
        Some(
            args.get(2)
                .map(std::path::Path::new)
                .unwrap_or_else(|| std::path::Path::new("work/fixture")),
        )
    } else {
        None
    }) {
        eprintln!("Scan Lab: {e}");
        std::process::exit(1);
    }
}
