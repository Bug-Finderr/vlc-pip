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
        let _ = std::fs::write(state::crash_path(), format!("panic at {loc}: {msg}"));
        if daemon::owns_alive_file() {
            // a crashed daemon must not leave a fresh heartbeat: pip.lua would treat it as
            // alive for up to 15s and drop menu toggles (v1 deleted it in its finally block)
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
    let args: Vec<String> =
        std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let mode = args.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let tail = args.get(1..).unwrap_or(&[]);
    match mode.as_str() {
        "exit" => {
            if native::exit_pip() { 0 } else { 1 }
        }
        "status" => {
            let _ = std::fs::write(state::status_path(), native::status());
            0
        }
        "daemon" => daemon::run(tail),
        _ => {
            eprintln!("unknown mode: {mode}");
            2
        }
    }
}
