use hive_contracts::{CpuInfo, GpuInfo, GpuVendor, HardwareInfo, MemoryInfo, RuntimeResourceUsage};

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Detect installed hardware. This is a best-effort snapshot using
/// platform-specific heuristics; real GPU introspection would need
/// NVML / Metal / DirectX queries that are out of scope for the stub.
pub fn detect_hardware() -> HardwareInfo {
    HardwareInfo { cpu: detect_cpu(), memory: detect_memory(), gpus: detect_gpus() }
}

fn detect_cpu() -> CpuInfo {
    let arch = std::env::consts::ARCH.to_string();
    #[cfg(target_os = "macos")]
    let name = {
        std::process::Command::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "Unknown CPU".into())
    };
    #[cfg(target_os = "windows")]
    let name = { std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "Unknown CPU".into()) };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let name = "Unknown CPU".to_string();

    let logical = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    let physical = logical / 2; // rough heuristic

    CpuInfo { name, cores_physical: physical.max(1), cores_logical: logical, arch }
}

fn detect_memory() -> MemoryInfo {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        // Parse wmic output for total physical memory
        let total = std::process::Command::new("wmic")
            .args(["OS", "get", "TotalVisibleMemorySize", "/value"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("TotalVisibleMemorySize"))
                    .and_then(|l| l.split('=').nth(1))
                    .and_then(|v| v.trim().parse::<u64>().ok())
            })
            .unwrap_or(0)
            * 1024; // wmic returns KB, convert to bytes
        let available = std::process::Command::new("wmic")
            .args(["OS", "get", "FreePhysicalMemory", "/value"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("FreePhysicalMemory"))
                    .and_then(|l| l.split('=').nth(1))
                    .and_then(|v| v.trim().parse::<u64>().ok())
            })
            .unwrap_or(0)
            * 1024;
        MemoryInfo { total_bytes: total, available_bytes: available }
    }
    #[cfg(target_os = "macos")]
    {
        let total = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        // Parse vm_stat for free + inactive pages
        let available = std::process::Command::new("vm_stat")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| {
                let page_size: u64 = 16384; // ARM64 default; Intel is 4096
                let extract = |key: &str| -> u64 {
                    s.lines()
                        .find(|l| l.contains(key))
                        .and_then(|l| {
                            l.split(':').nth(1).map(|v| {
                                let parsed = v.trim().trim_end_matches('.').parse::<u64>();
                                if parsed.is_err() {
                                    tracing::warn!("failed to parse vm_stat value: {:?}", v.trim());
                                }
                                parsed.unwrap_or(0)
                            })
                        })
                        .unwrap_or(0)
                };
                (extract("Pages free") + extract("Pages inactive")) * page_size
            })
            .unwrap_or(0);
        MemoryInfo { total_bytes: total, available_bytes: available }
    }
    #[cfg(target_os = "linux")]
    {
        // Parse /proc/meminfo
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let extract_kb = |key: &str| -> u64 {
            meminfo
                .lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()))
                .unwrap_or(0)
        };
        MemoryInfo {
            total_bytes: extract_kb("MemTotal:") * 1024,
            available_bytes: extract_kb("MemAvailable:") * 1024,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        MemoryInfo { total_bytes: 0, available_bytes: 0 }
    }
}

fn detect_gpus() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();

    // Try nvidia-smi for NVIDIA GPUs.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    {
        let mut cmd = std::process::Command::new("nvidia-smi");
        cmd.args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"]);

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    for line in text.lines() {
                        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                        if parts.len() >= 3 {
                            let vram_mb: u64 = parts[1].parse().unwrap_or_else(|e| {
                                tracing::warn!("failed to parse GPU VRAM from nvidia-smi: {e}");
                                0
                            });
                            gpus.push(GpuInfo {
                                name: parts[0].to_string(),
                                vendor: GpuVendor::Nvidia,
                                vram_bytes: Some(vram_mb * 1024 * 1024),
                                driver_version: Some(parts[2].to_string()),
                            });
                        }
                    }
                }
            }
        }
    }

    // macOS: detect Apple Silicon GPU.
    #[cfg(target_os = "macos")]
    {
        if std::env::consts::ARCH == "aarch64" {
            gpus.push(GpuInfo {
                name: "Apple Silicon (integrated)".to_string(),
                vendor: GpuVendor::Apple,
                vram_bytes: None, // shared with system memory
                driver_version: None,
            });
        }
    }

    gpus
}

/// Get current resource usage for loaded models. In a real implementation
/// this would query the runtimes; for now returns a placeholder.
pub fn current_resource_usage() -> RuntimeResourceUsage {
    RuntimeResourceUsage::default()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_hardware_returns_valid_info() {
        let hw = detect_hardware();
        assert!(!hw.cpu.name.is_empty());
        assert!(hw.cpu.cores_logical >= 1);
        assert!(!hw.cpu.arch.is_empty());
    }

    #[test]
    fn resource_usage_defaults() {
        let usage = current_resource_usage();
        assert_eq!(usage.models_loaded, 0);
        assert_eq!(usage.ram_used_bytes, 0);
    }

    #[test]
    fn gpu_vendor_serialization() {
        let json = serde_json::to_string(&GpuVendor::Nvidia).unwrap();
        assert_eq!(json, "\"nvidia\"");
        let parsed: GpuVendor = serde_json::from_str("\"apple\"").unwrap();
        assert_eq!(parsed, GpuVendor::Apple);
    }
}
