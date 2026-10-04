//! M8 discovery (plan 014): the desktop advertises itself over mDNS so
//! a tablet can find it without typing an address - the "both devices
//! declare that they are open" half of the Bluetooth-style pairing.
//! Browsing stays on the tablet (Android NsdManager); the desktop only
//! announces. Pure-Rust stack (`mdns-sd`) - no system mDNS service on
//! Windows, no async runtime. Failure to advertise is never fatal:
//! manual pairing keeps working.

use mdns_sd::{ServiceDaemon, ServiceInfo};

/// The advertised service: hold the handle for the process lifetime -
/// dropping it (or stopping the daemon) unregisters the announcement.
pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
}

/// Service type the tablets browse for. Version and protocol ride in
/// TXT so a browser can filter before resolving.
pub const SERVICE_TYPE: &str = "_pulpit._tcp.local.";

pub fn advertise(port: u16, version: &str) -> Result<Discovery, String> {
    let daemon = ServiceDaemon::new().map_err(|e| format!("mDNS daemon: {e}"))?;
    let host = hostname::get()
        .map(|h| h.to_string_lossy().trim_end_matches('.').to_string())
        .unwrap_or_else(|_| "pulpit".to_string());
    let instance = format!("Pulpit on {host}");
    let fullname = format!("{instance}.{SERVICE_TYPE}");
    let props = [
        ("proto", "v2"),
        ("version", version),
        ("host", host.as_str()),
    ];
    // addr_auto: the daemon announces every local interface address, so
    // the tablet reaches the right NIC without us picking one.
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &format!("{host}.local."),
        "",
        port,
        &props[..],
    )
    .map_err(|e| format!("mDNS service info: {e}"))?
    .enable_addr_auto();
    daemon
        .register(info)
        .map_err(|e| format!("mDNS register: {e}"))?;
    tracing::info!(service = %fullname, port, "mDNS advertisement up");
    Ok(Discovery { daemon, fullname })
}

impl Discovery {
    /// Graceful unregister (best-effort; process death also clears the
    /// announcement once the TTL lapses). Both hosts currently
    /// `mem::forget` the handle and rely on the TTL; kept for a host that
    /// wants to withdraw the announcement before exit.
    pub fn stop(self) {
        let _ = self.daemon.unregister(&self.fullname);
    }
}
