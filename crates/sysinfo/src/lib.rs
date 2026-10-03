//! Native System Info source: replaces the `deckboard-system-info` JS
//! extension, whose Boa runtime was the heaviest one loaded (~13 MB of
//! committed heap, churned by per-second systeminformation sampling).
//!
//! Push parity contract with the JS package (`index.js`):
//! - a push every 10 s (`si.observe(..., 10000, ...)`) plus one right at
//!   startup, as one `setValue` object with the four `si-*` keys;
//! - RAM "percent" value is the raw `used / total` fraction with one
//!   decimal (upstream computes it that way; the client scales it);
//! - CPU temperature has no usable native source here, so it is `null`
//!   (systeminformation returns no real value on Windows either, and no
//!   board in the wild consumes this key).

use std::time::Duration;

use tokio::sync::mpsc as tokio_mpsc;

/// Tile inputs this source serves: (value, icon, fontIcon, color, mode).
pub fn input_declarations() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    vec![
        ("si-cpu", "headphones", "fas", "#8E44AD", "graph"),
        ("si-ram", "headphones", "fas", "#8E44AD", "graph"),
    ]
}

/// Does this action kind belong to the native system-info source?
pub fn is_sysinfo_action(kind: &str) -> bool {
    kind == "si-cpu" || kind == "si-ram"
}

/// The JS extension's `execute(action, args) {}` is empty; keep the same
/// no-op so tile presses succeed without changing anything.
pub fn execute(kind: &str) {
    tracing::debug!(kind, "system-info action accepted (no-op, JS parity)");
}

/// Spawn the sampling loop; values arrive as the `setValue` object (all
/// four keys, exactly like the JS package pushed).
pub fn spawn_push() -> tokio_mpsc::UnboundedReceiver<serde_json::Value> {
    let (tx, rx) = tokio_mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("sysinfo-push".into())
        .spawn(move || push_loop(&tx))
        .expect("spawn sysinfo thread");
    rx
}

fn push_loop(tx: &tokio_mpsc::UnboundedSender<serde_json::Value>) {
    let brand = cpu_brand();
    let interval = Duration::from_secs(10);
    let mut times = cpu_times();
    // first push has no delta yet; the JS observe loop warmed up the same way
    let mut load_pct = 0.0;
    loop {
        // a failed send means the receiver is gone (server shutting
        // down): end the thread instead of sampling into the void forever
        if !send(tx, &brand, load_pct, &mem_info()) {
            tracing::debug!("sysinfo push channel closed, stopping the sampling loop");
            return;
        }
        std::thread::sleep(interval);
        let next = cpu_times();
        load_pct = cpu_load_pct(times, next);
        times = next;
    }
}

fn send(
    tx: &tokio_mpsc::UnboundedSender<serde_json::Value>,
    brand: &str,
    load_pct: f64,
    mem: &(u64, u64),
) -> bool {
    let (total, avail) = *mem;
    let used = total.saturating_sub(avail) as f64;
    let payload = serde_json::json!({
        "si-load-cpu": {
            "title": "CPU Load",
            "description": brand,
            "value": format!("{load_pct:.1}"),
            "suffix": "%",
        },
        "si-temperature-cpu": {
            "title": "CPU Temperature",
            "description": brand,
            "value": serde_json::Value::Null,
            "suffix": "°C",
        },
        "si-load-p-ram": {
            "title": "RAM Usage",
            "value": format!("{:.1}", if total == 0 { 0.0 } else { used / total as f64 }),
            "suffix": "%",
        },
        "si-load-gb-ram": {
            "title": "RAM Usage",
            "value": format!("{:.1}", used * 1e-9),
            "suffix": "GB",
        },
    });
    if tx.send(payload).is_err() {
        // receiver dropped: tell the loop to stop
        return false;
    }
    tracing::debug!(target: "pulpit_sysinfo", load = load_pct, "pushed system-info values");
    true
}

// ------------------------------------------------------------ CPU sampling

/// Total (idle, kernel, user) CPU times in 100 ns ticks since boot.
/// Same kernel32 read the JS host's os shim is built on.
#[cfg(windows)]
fn cpu_times() -> (u64, u64, u64) {
    #[repr(C)]
    #[derive(Default)]
    struct Filetime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(idle: *mut Filetime, kernel: *mut Filetime, user: *mut Filetime) -> i32;
    }
    let (mut idle, mut kernel, mut user) = (
        Filetime::default(),
        Filetime::default(),
        Filetime::default(),
    );
    // SAFETY: three distinct out-parameters of the documented struct size
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
    if ok == 0 {
        return (0, 0, 0);
    }
    let raw = |f: &Filetime| ((f.high as u64) << 32) | f.low as u64;
    (raw(&idle), raw(&kernel), raw(&user))
}

#[cfg(not(windows))]
fn cpu_times() -> (u64, u64, u64) {
    (0, 0, 0)
}

/// (total, available) physical memory in bytes. Mirrors the JS host's
/// GlobalMemoryStatusEx read; systeminformation computes
/// `mem.used = total - available` from the same source.
#[cfg(windows)]
fn mem_info() -> (u64, u64) {
    // must mirror MEMORYSTATUSEX exactly: 64 bytes, dwLength = 64, or the
    // API rejects the call
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page: u64,
        avail_page: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_virtual_extended: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buf: *mut MemoryStatus) -> i32;
    }
    let mut buf = MemoryStatus {
        length: std::mem::size_of::<MemoryStatus>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page: 0,
        avail_page: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_virtual_extended: 0,
    };
    debug_assert_eq!(buf.length, 64);
    // SAFETY: buf is initialized with the expected dwLength
    let ok = unsafe { GlobalMemoryStatusEx(&mut buf) };
    if ok == 0 {
        return (0, 0);
    }
    (buf.total_phys, buf.avail_phys)
}

#[cfg(not(windows))]
fn mem_info() -> (u64, u64) {
    (0, 0)
}

/// Busy percentage between two time samples. The JS stack derived the same
/// number from these counters via the os shim: node's shape folds all
/// kernel time (idle included) into `sys`, so busy = user + kernel - idle
/// and total = user + kernel.
fn cpu_load_pct(prev: (u64, u64, u64), next: (u64, u64, u64)) -> f64 {
    let delta = |a: u64, b: u64| b.saturating_sub(a);
    let idle = delta(prev.0, next.0);
    let kernel = delta(prev.1, next.1);
    let user = delta(prev.2, next.2);
    let total = user + kernel;
    if total == 0 {
        return 0.0;
    }
    100.0 * (total - idle) as f64 / total as f64
}

/// `si.cpu().brand` equivalent: the processor name from the registry (the
/// same string WMI serves systeminformation on Windows).
#[cfg(windows)]
fn cpu_brand() -> String {
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueA(
            key: usize,
            sub: *const u8,
            value: *const u8,
            flags: u32,
            r#type: *mut u32,
            data: *mut u8,
            len: *mut u32,
        ) -> i32;
    }
    const HKEY_LOCAL_MACHINE: usize = 0x8000_0002;
    const RRF_RT_REG_SZ: u32 = 0x0000_0002;
    let sub = b"HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0\0";
    let name = b"ProcessorNameString\0";
    let mut buf = [0u8; 256];
    let mut len = buf.len() as u32;
    // SAFETY: all pointers reference initialized buffers of the passed size
    let ok = unsafe {
        RegGetValueA(
            HKEY_LOCAL_MACHINE,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if ok == 0 {
        let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        let text = String::from_utf8_lossy(&buf[..end]).trim().to_string();
        if !text.is_empty() {
            return text;
        }
    }
    "CPU".into()
}

#[cfg(not(windows))]
fn cpu_brand() -> String {
    "CPU".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_channel_ends_the_push_loop() {
        // the receiver is dropped immediately: send must report the
        // closed channel so push_loop exits instead of sampling forever
        let (tx, rx) = tokio_mpsc::unbounded_channel::<serde_json::Value>();
        drop(rx);
        assert!(!send(&tx, "CPU", 1.0, &(8, 4)));
        // an open channel keeps the loop alive
        let (tx, _rx) = tokio_mpsc::unbounded_channel::<serde_json::Value>();
        assert!(send(&tx, "CPU", 1.0, &(8, 4)));
    }
}
