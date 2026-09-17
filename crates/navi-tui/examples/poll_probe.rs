//! Probe: does `crossterm::event::poll` honour its timeout in a PTY?
//!
//! Run inside a PTY (e.g. `script -qec "cargo run -p navi-tui --example poll_probe" /dev/null`):
//! ten 50 ms polls must finish in ~500 ms. If this hangs, a TUI loop that only
//! polls the terminal cannot react to control messages.

fn main() {
    let start = std::time::Instant::now();
    for index in 0..10 {
        let ready = crossterm::event::poll(std::time::Duration::from_millis(50)).unwrap_or(false);
        println!("{index}: ready={ready} elapsed={:?}", start.elapsed());
    }
    println!("total {:?}", start.elapsed());
}
