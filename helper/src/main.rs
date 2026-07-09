#![windows_subsystem = "windows"]

mod daemon;
mod geometry;
mod native;
mod options;
mod state;
#[cfg(test)]
mod tests;

fn main() {
    // GUI-subsystem exe: a panic is otherwise invisible. Location (file:line) survives
    // strip; exit 3 matches v1's crash exit code. The hook itself must never panic.
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let msg = info.payload_as_str().unwrap_or("panic");
        let _ = std::fs::write(state::temp_path("vlc-pip-crash.txt"), format!("panic at {loc}: {msg}"));
        if daemon::owns_alive_file() {
            // a crashed daemon must not leave a fresh heartbeat: pip.lua would treat it as
            // alive for up to 15s and drop menu toggles (v1 deleted it in its finally block)
            let _ = std::fs::remove_file(state::temp_path("vlc-pip-daemon.alive"));
        }
        std::process::exit(3);
    }));
    std::process::exit(run());
}

fn run() -> i32 {
    native::enable_dpi_awareness();
    // args_os + lossy: std::env::args() would panic (= crash file) on non-Unicode argv,
    // and links extra machinery for it; every legitimate token here is ASCII anyway
    let args: Vec<String> =
        std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let mode = args.first().map_or_else(|| "toggle".to_string(), |s| s.to_ascii_lowercase());
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
        "exit" => {
            if native::exit_pip() { 0 } else { 1 }
        }
        "status" => {
            let s = native::status();
            let _ = std::fs::write(native::status_path(), &s); // the reliable channel for scripts: written FIRST
            // stdout is best-effort on a GUI-subsystem exe, and println! PANICS on a broken
            // pipe (only NULL handles are forgiven) - status must always exit 0 with the file written
            use std::io::Write;
            let _ = writeln!(std::io::stdout(), "{s}");
            0
        }
        "daemon" => daemon::run(tail), // per-gesture re-read: the daemon must see its own config writes
        "stop" => {
            if std::fs::write(state::request_path(), "stop").is_ok() { 0 } else { 1 }
        }
        _ => {
            eprintln!("unknown mode: {mode}");
            2
        }
    }
}

// one-shot (no daemon ticks): converge the minimal-look region here, sleeps are harmless
fn one_shot(ok: bool, o: &options::PipOptions) -> i32 {
    if ok && o.min && native::in_pip() {
        // min=0 makes maintain_region a no-op: skip the pure sleep
        let mut tracker = native::RegionTracker::default();
        for _ in 0..6 {
            // debounce needs ~4 ticks: measure, resize, measure, region
            std::thread::sleep(std::time::Duration::from_millis(150));
            native::maintain_region(&mut tracker, state::load(&state::state_path()));
        }
    }
    if ok { 0 } else { 1 }
}
