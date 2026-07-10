#![windows_subsystem = "windows"]

mod daemon;
mod geometry;
mod native;
mod options;
mod state;
#[cfg(test)]
mod tests;

fn main() {
    // A GUI-subsystem panic is otherwise invisible. Location survives stripping, and
    // the hook itself must never panic.
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let msg = info.payload_as_str().unwrap_or("panic");
        let _ = std::fs::write(state::crash_path(), format!("panic at {loc}: {msg}"));
        if daemon::owns_alive_file() {
            // A fresh heartbeat from a crashed daemon would make pip.lua drop toggles.
            let _ = std::fs::remove_file(state::alive_path());
        }
        std::process::exit(3);
    }));
    std::process::exit(run());
}

fn run() -> i32 {
    native::enable_dpi_awareness();
    // args_os + lossy: std::env::args() panics on non-Unicode argv; every legitimate
    // token here is ASCII anyway
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let mode = args
        .first()
        .map_or_else(|| "toggle".to_string(), |s| s.to_ascii_lowercase());
    let tail = args.get(1..).unwrap_or(&[]);
    match mode.as_str() {
        "toggle" => {
            let o = options::effective(tail);
            one_shot(native::toggle(&o), &o)
        }
        "enter" => {
            let o = options::effective(tail);
            one_shot(native::enter(native::find_player(), &o), &o)
        }
        "exit" => i32::from(!native::exit_pip()),
        "status" => {
            let s = native::status();
            let _ = std::fs::write(state::status_path(), &s); // the reliable channel for scripts: written FIRST
            // stdout is best-effort on a GUI-subsystem exe, and println! PANICS on a broken
            // pipe (only NULL handles are forgiven) - status must always exit 0 with the file written
            use std::io::Write;
            let _ = writeln!(std::io::stdout(), "{s}");
            0
        }
        "daemon" => daemon::run(tail), // per-gesture re-read: the daemon must see its own config writes
        "stop" => i32::from(std::fs::write(state::request_path(), "stop").is_err()),
        _ => {
            eprintln!("unknown mode: {mode}");
            2
        }
    }
}

// One-shot commands converge the minimal look without relying on daemon ticks.
fn one_shot(ok: bool, o: &options::PipOptions) -> i32 {
    if ok && o.min && native::in_pip() {
        let mut tracker = native::RegionTracker::default();
        for _ in 0..6 {
            // debounce needs ~4 ticks: measure, resize, measure, region
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = native::maintain_region(&mut tracker, state::load(&state::state_path()));
        }
    }
    i32::from(!ok)
}
