//! System diagnostics for dualink.
//!
//! Provides startup health checks and a detailed `--diagnose` report.
//! macOS-specific: checks accessibility permission, VirtualHID, Secure Input,
//! and OS version.

/// Run lightweight startup checks and log warnings for anything abnormal.
pub fn log_startup_checks() {
    #[cfg(target_os = "macos")]
    macos::log_startup_checks();
}

/// Print a full diagnostic report to stdout (for `--diagnose`).
pub fn print_full_report(port: u16) {
    #[cfg(target_os = "macos")]
    macos::print_full_report(port);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = port;
        println!("Diagnostics are currently only available on macOS.");
    }
}

// ---------------------------------------------------------------------------
// macOS implementation
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use std::process::Command;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn IsSecureEventInputEnabled() -> u8;
    }

    fn is_accessibility_trusted() -> bool {
        unsafe { AXIsProcessTrusted() != 0 }
    }

    fn is_secure_input_enabled() -> bool {
        unsafe { IsSecureEventInputEnabled() != 0 }
    }

    fn macos_version() -> String {
        Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            })
            .unwrap_or_else(|| "unknown".into())
    }

    fn macos_build() -> String {
        Command::new("sw_vers")
            .arg("-buildVersion")
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            })
            .unwrap_or_else(|| "unknown".into())
    }

    const VHID_SOCKET_DIR: &str = "/Library/Application Support/org.pqrs/tmp/rootonly/vhidd_server";

    fn is_vhid_daemon_present() -> bool {
        std::path::Path::new(VHID_SOCKET_DIR).exists()
    }

    fn is_vhid_driver_loaded() -> bool {
        Command::new("systemextensionsctl")
            .arg("list")
            .output()
            .ok()
            .map(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                // Match both activated and enabled states
                stdout.contains("org.pqrs.Karabiner-DriverKit-VirtualHIDDevice")
                    && (stdout.contains("[activated enabled]")
                        || stdout.contains("[activated waiting for user]"))
            })
            .unwrap_or(false)
    }

    fn check_port_available(port: u16) -> bool {
        std::net::UdpSocket::bind(("0.0.0.0", port)).is_ok()
    }

    // -----------------------------------------------------------------------
    // Startup checks (log only)
    // -----------------------------------------------------------------------

    pub fn log_startup_checks() {
        let version = macos_version();
        log::info!("macOS {version} (build {})", macos_build());

        if !is_accessibility_trusted() {
            log::warn!(
                "Accessibility permission NOT granted — input capture will fail. \
                 Open System Settings → Privacy & Security → Accessibility."
            );
        }

        if is_secure_input_enabled() {
            log::warn!(
                "Secure Input is active — some key events may not be captured. \
                 A password field or secure app may be focused."
            );
        }

        // Warn about known problematic macOS versions
        if version.starts_with("26.") {
            log::info!("macOS Tahoe detected — using hardened CGEventTap with auto-recovery");
        }
    }

    // -----------------------------------------------------------------------
    // Full diagnostic report (stdout)
    // -----------------------------------------------------------------------

    pub fn print_full_report(port: u16) {
        println!("╔══════════════════════════════════════════╗");
        println!("║     Dualink System Diagnostics           ║");
        println!("╚══════════════════════════════════════════╝");
        println!();

        // --- OS ---
        let version = macos_version();
        let build = macos_build();
        println!("  macOS version:  {version} ({build})");

        if version.starts_with("26.") {
            println!("  ⚠  macOS Tahoe — CGEventTap may be intermittently disabled by system");
        }
        println!();

        // --- Accessibility ---
        let ax = is_accessibility_trusted();
        println!(
            "  Accessibility:  {}",
            if ax { "✓ granted" } else { "✗ NOT granted" }
        );
        if !ax {
            println!("     → Open System Settings → Privacy & Security → Accessibility");
            println!("     → Add and enable the dualink binary");
        }
        println!();

        // --- Secure Input ---
        let si = is_secure_input_enabled();
        println!(
            "  Secure Input:   {}",
            if si {
                "⚠ ACTIVE — some key events will not be captured"
            } else {
                "✓ inactive"
            }
        );
        if si {
            println!("     → Close any focused password field or secure application");
            println!("     → Common culprits: Terminal (Secure Keyboard Entry), 1Password, iTerm2");
        }
        println!();

        // --- VirtualHID ---
        let vhid_daemon = is_vhid_daemon_present();
        let vhid_driver = is_vhid_driver_loaded();
        println!("  VirtualHID daemon:  {}", status_str(vhid_daemon));
        println!("  VirtualHID driver:  {}", status_str(vhid_driver));
        if !vhid_daemon && !vhid_driver {
            println!("     → Karabiner VirtualHID not installed");
            println!("     → dualink will use CGEventPost (fn/Globe key not supported)");
            println!("     → Install: https://karabiner-elements.pqrs.org/");
        } else if vhid_daemon && !vhid_driver {
            println!(
                "     → Driver not activated — check System Settings → General → Login Items & Extensions"
            );
        }
        println!();

        // --- Network ---
        let port_ok = check_port_available(port);
        println!(
            "  UDP port {port}:    {}",
            if port_ok {
                "✓ available"
            } else {
                "✗ in use (another dualink instance or conflicting service?)"
            }
        );
        println!();

        // --- Summary ---
        let issues = [!ax, si, !port_ok];
        let issue_count = issues.iter().filter(|&&b| b).count();
        if issue_count == 0 {
            println!("  All checks passed.");
        } else {
            println!("  {issue_count} issue(s) found — see above for details.");
        }
    }

    fn status_str(ok: bool) -> &'static str {
        if ok { "✓ available" } else { "✗ not found" }
    }
}
