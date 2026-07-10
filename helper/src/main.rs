#![windows_subsystem = "windows"]

mod daemon;
mod geometry;
mod native;
mod options;
mod state;
#[cfg(test)]
mod tests;

fn main() {
    // GUI-subsystem exe: a panic is otherwise invisible; the hook itself must never panic.
    std::panic::set_hook(Box::new(|info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        let msg = info.payload_as_str().unwrap_or("panic");
        let _ = std::fs::write(state::crash_path(), format!("panic at {loc}: {msg}"));
        if std::env::args_os().nth(1).is_some_and(|a| a.eq_ignore_ascii_case("daemon")) {
            // a crashed daemon must not leave a fresh heartbeat (SPEC 6.3)
            let _ = std::fs::remove_file(state::alive_path());
        }
        std::process::exit(3);
    }));
    std::process::exit(run());
}

fn run() -> i32 {
    native::enable_dpi_awareness();
    // std::env::args() panics on non-Unicode argv; every legitimate token here is ASCII
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
