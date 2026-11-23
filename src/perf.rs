//! Performance / gaming helpers: engine flags, process priority, memory accounting.

use crate::storage::Settings;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Chromium flags passed to the WebView2 engine (Windows). Every webview in the profile must
/// get the exact same flags, so this is computed once at startup.
#[allow(dead_code)]
pub fn engine_args(s: &Settings) -> String {
    let mut args = vec![
        // wry's defaults (we must re-add them because we override the argument list)
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,msEdgeShopping,msEdgeCollections".to_string(),
        // Group same-site tabs into one renderer process: big RAM saving with many tabs.
        "--process-per-site".to_string(),
        format!("--renderer-process-limit={}", s.renderer_process_limit.clamp(2, 32)),
        "--no-pings".to_string(),
        "--disable-domain-reliability".to_string(),
        "--autoplay-policy=user-gesture-required".to_string(),
    ];
    if s.gpu_boost {
        // Gaming GPUs are never on Chromium's blocklist for a good reason; use them fully.
        args.push("--enable-gpu-rasterization".into());
        args.push("--enable-zero-copy".into());
        args.push("--ignore-gpu-blocklist".into());
        args.push("--enable-features=CanvasOopRasterization".into());
    }
    args.join(" ")
}

pub struct Monitor {
    sys: Option<System>,
    me: Option<Pid>,
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            sys: System::new().ok(),
            me: sysinfo::get_current_pid().ok(),
        }
    }

    fn refresh(&mut self) {
        let Some(sys) = &mut self.sys else { return };
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
    }

    /// This process plus every engine process it spawned (browser, GPU, renderers...).
    fn family(&self) -> Vec<Pid> {
        let (Some(me), Some(sys)) = (self.me, &self.sys) else {
            return Vec::new();
        };
        let mut out = vec![me];
        let mut i = 0;
        while i < out.len() {
            let parent = out[i];
            for (pid, p) in sys.processes() {
                // On Linux sysinfo also lists threads; they share their process's memory.
                if p.thread_kind().is_none() && p.parent() == Some(parent) && !out.contains(pid) {
                    out.push(*pid);
                }
            }
            i += 1;
        }
        out
    }

    /// Total resident memory (bytes) of the browser and all of its engine processes.
    pub fn memory(&mut self) -> u64 {
        self.refresh();
        let Some(sys) = &self.sys else { return 0 };
        self.family()
            .iter()
            .filter_map(|pid| sys.process(*pid).map(|p| (pid.as_u32(), p.memory())))
            .map(|(pid, rss)| private_bytes(pid).unwrap_or(rss))
            .sum()
    }

    /// Game mode: when `background` is true every browser process drops to below-normal
    /// priority and Windows "efficiency mode" (EcoQoS), so a running game gets the CPU.
    pub fn set_background(&mut self, background: bool) {
        self.refresh();
        for pid in self.family() {
            set_process_background(pid.as_u32(), background);
        }
    }
}

/// Lower our own priority before the engine starts, so every engine process inherits it.
pub fn lower_own_priority_at_startup() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS,
        };
        let _ = SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
    }
}

/// Private memory of a process (what Chrome's task manager calls its "memory footprint").
/// Summing resident sizes would count shared engine DLLs once per process.
#[cfg(windows)]
fn private_bytes(pid: u32) -> Option<u64> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        let ok = K32GetProcessMemoryInfo(h, &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS, size)
            .as_bool();
        let _ = CloseHandle(h);
        ok.then_some(c.PrivateUsage as u64)
    }
}

#[cfg(not(windows))]
fn private_bytes(_pid: u32) -> Option<u64> {
    None
}

#[cfg(windows)]
fn set_process_background(pid: u32, background: bool) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
        BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_STATE, PROCESS_SET_INFORMATION,
    };
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_SET_INFORMATION, false, pid) else {
            return;
        };
        let class = if background {
            BELOW_NORMAL_PRIORITY_CLASS
        } else {
            NORMAL_PRIORITY_CLASS
        };
        let _ = SetPriorityClass(h, class);
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            StateMask: if background {
                PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            } else {
                0
            },
        };
        let _ = SetProcessInformation(
            h,
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
        let _ = CloseHandle(h);
    }
}

#[cfg(not(windows))]
fn set_process_background(_pid: u32, _background: bool) {}
