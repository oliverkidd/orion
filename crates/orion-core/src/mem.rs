//! Small memory probes shared by the daemon and the TUI client. All of them
//! shell out (macOS has no /proc), so none of them belongs on an event
//! loop: the daemon runs its sweep on the blocking pool, and the TUI reads
//! its own RSS there too — the footer's readout asks every five seconds.

/// Resident set size of one process, bytes.
pub fn process_rss_bytes(pid: u32) -> Option<u64> {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    text.trim().parse::<u64>().ok().map(|kb| kb * 1024)
}

/// Physical memory installed on this machine, bytes.
pub fn system_total_bytes() -> Option<u64> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()?;
    String::from_utf8(out.stdout).ok()?.trim().parse().ok()
}
