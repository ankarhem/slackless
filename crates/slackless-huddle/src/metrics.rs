//! Live process resource usage (CPU%, RSS MiB) and video pipeline telemetry (FPS, encode/decode
//! latency, bitrate, active hardware/software backend) for evaluating real-world huddle
//! performance in `slackless.log` and the Huddle Diagnostics drawer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// Windowed performance snapshot surfaced in `HuddleSessionSnapshot` and periodic `tracing` logs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HuddlePerfSnapshot {
    /// Process CPU usage in tenths of a percent (e.g., `34` == `3.4%`).
    pub cpu_tenths_pct: u32,
    /// Process resident memory (RSS) in MiB.
    pub rss_mb: u32,
    /// Outbound video/screen encoder backend (e.g. `"ScreenCaptureKit + VideoToolbox (HW)"`).
    pub tx_backend: Option<String>,
    /// Outbound encoded resolution (e.g. `"1856x1200"`).
    pub tx_resolution: Option<String>,
    /// Outbound video frames per second in tenths (`295` == `29.5 fps`).
    pub tx_fps_tenths: u32,
    /// Average outbound frame capture+encode latency in microseconds.
    pub tx_encode_us: u32,
    /// Outbound video bitrate in kbps.
    pub tx_kbps: u32,
    /// Inbound remote video decoder backend (e.g. `"AV1 (rav1d)"`, `"H.264 (VideoToolbox HW)"`).
    pub rx_backend: Option<String>,
    /// Inbound decoded resolution (e.g. `"1920x1080"`).
    pub rx_resolution: Option<String>,
    /// Inbound decoded video frames per second in tenths.
    pub rx_fps_tenths: u32,
    /// Average inbound frame decode+convert latency in microseconds.
    pub rx_decode_us: u32,
    /// Inbound video bitrate in kbps.
    pub rx_kbps: u32,
}

/// Lock-free atomic counters updated by capture, encode, and decode threads.
#[derive(Default)]
pub struct HuddlePerfCounters {
    tx_backend: Mutex<Option<String>>,
    tx_width: AtomicU32,
    tx_height: AtomicU32,
    tx_frames: AtomicU64,
    tx_bytes: AtomicU64,
    tx_encode_us_sum: AtomicU64,

    rx_backend: Mutex<Option<String>>,
    rx_width: AtomicU32,
    rx_height: AtomicU32,
    rx_frames: AtomicU64,
    rx_bytes: AtomicU64,
    rx_decode_us_sum: AtomicU64,
}

impl HuddlePerfCounters {
    pub fn set_tx_backend(&self, name: &str) {
        if let Ok(mut guard) = self.tx_backend.lock() {
            *guard = Some(name.to_owned());
        }
    }

    pub fn clear_tx(&self) {
        if let Ok(mut guard) = self.tx_backend.lock() {
            *guard = None;
        }
        self.tx_width.store(0, Ordering::Relaxed);
        self.tx_height.store(0, Ordering::Relaxed);
    }

    pub fn record_tx_frame(&self, width: u32, height: u32, encoded_bytes: u64, encode_us: u64) {
        self.tx_width.store(width, Ordering::Relaxed);
        self.tx_height.store(height, Ordering::Relaxed);
        self.tx_frames.fetch_add(1, Ordering::Relaxed);
        self.tx_bytes.fetch_add(encoded_bytes, Ordering::Relaxed);
        self.tx_encode_us_sum
            .fetch_add(encode_us, Ordering::Relaxed);
    }

    pub fn set_rx_backend(&self, name: &str) {
        if let Ok(mut guard) = self.rx_backend.lock() {
            *guard = Some(name.to_owned());
        }
    }

    pub fn record_rx_packet(&self, payload_bytes: u64) {
        self.rx_bytes.fetch_add(payload_bytes, Ordering::Relaxed);
    }

    pub fn record_rx_frame(&self, width: u32, height: u32, decode_us: u64) {
        self.rx_width.store(width, Ordering::Relaxed);
        self.rx_height.store(height, Ordering::Relaxed);
        self.rx_frames.fetch_add(1, Ordering::Relaxed);
        self.rx_decode_us_sum
            .fetch_add(decode_us, Ordering::Relaxed);
    }

    /// Drain window deltas and combine with process CPU% / RSS MiB into a snapshot.
    pub fn sample(
        &self,
        elapsed_secs: f64,
        cpu_tenths_pct: u32,
        rss_mb: u32,
    ) -> HuddlePerfSnapshot {
        let dt = elapsed_secs.max(0.1);
        let tx_frames = self.tx_frames.swap(0, Ordering::Relaxed);
        let tx_bytes = self.tx_bytes.swap(0, Ordering::Relaxed);
        let tx_us = self.tx_encode_us_sum.swap(0, Ordering::Relaxed);
        let rx_frames = self.rx_frames.swap(0, Ordering::Relaxed);
        let rx_bytes = self.rx_bytes.swap(0, Ordering::Relaxed);
        let rx_us = self.rx_decode_us_sum.swap(0, Ordering::Relaxed);

        let tw = self.tx_width.load(Ordering::Relaxed);
        let th = self.tx_height.load(Ordering::Relaxed);
        let rw = self.rx_width.load(Ordering::Relaxed);
        let rh = self.rx_height.load(Ordering::Relaxed);

        HuddlePerfSnapshot {
            cpu_tenths_pct,
            rss_mb,
            tx_backend: self.tx_backend.lock().ok().and_then(|g| g.clone()),
            tx_resolution: (tw > 0 && th > 0).then(|| format!("{tw}x{th}")),
            tx_fps_tenths: ((tx_frames as f64 / dt) * 10.0).round() as u32,
            tx_encode_us: tx_us.checked_div(tx_frames).unwrap_or(0) as u32,
            tx_kbps: (((tx_bytes as f64 * 8.0) / dt) / 1000.0).round() as u32,
            rx_backend: self.rx_backend.lock().ok().and_then(|g| g.clone()),
            rx_resolution: (rw > 0 && rh > 0).then(|| format!("{rw}x{rh}")),
            rx_fps_tenths: ((rx_frames as f64 / dt) * 10.0).round() as u32,
            rx_decode_us: rx_us.checked_div(rx_frames).unwrap_or(0) as u32,
            rx_kbps: (((rx_bytes as f64 * 8.0) / dt) / 1000.0).round() as u32,
        }
    }
}

/// Samples the current process's CPU percentage and resident memory (RSS MiB) using OS APIs.
pub struct ProcessResourceSampler {
    last_wall: Instant,
    last_cpu_us: u64,
}

impl Default for ProcessResourceSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessResourceSampler {
    pub fn new() -> Self {
        Self {
            last_wall: Instant::now(),
            last_cpu_us: current_process_cpu_us(),
        }
    }

    /// Returns `(elapsed_secs, cpu_tenths_pct, rss_mb)` since the previous call.
    pub fn sample(&mut self) -> (f64, u32, u32) {
        let now = Instant::now();
        let elapsed_secs = now.duration_since(self.last_wall).as_secs_f64();
        let cpu_us = current_process_cpu_us();
        let delta_cpu_us = cpu_us.saturating_sub(self.last_cpu_us);
        self.last_wall = now;
        self.last_cpu_us = cpu_us;

        let wall_us = (elapsed_secs * 1_000_000.0).max(1.0);
        let cpu_tenths_pct = ((delta_cpu_us as f64 / wall_us) * 1000.0).round() as u32;
        let rss_mb = current_process_rss_mb();
        (elapsed_secs, cpu_tenths_pct, rss_mb)
    }
}

#[cfg(unix)]
fn current_process_cpu_us() -> u64 {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct TimeVal {
        tv_sec: isize,
        tv_usec: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RUsage {
        ru_utime: TimeVal,
        ru_stime: TimeVal,
        _rest: [isize; 14],
    }

    unsafe extern "C" {
        fn getrusage(who: i32, usage: *mut RUsage) -> i32;
    }

    const RUSAGE_SELF: i32 = 0;
    let mut usage = std::mem::MaybeUninit::<RUsage>::zeroed();
    let rc = unsafe { getrusage(RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return 0;
    }
    let u = unsafe { usage.assume_init() };
    let user_us = (u.ru_utime.tv_sec.max(0) as u64) * 1_000_000 + u.ru_utime.tv_usec.max(0) as u64;
    let sys_us = (u.ru_stime.tv_sec.max(0) as u64) * 1_000_000 + u.ru_stime.tv_usec.max(0) as u64;
    user_us.saturating_add(sys_us)
}

#[cfg(not(unix))]
fn current_process_cpu_us() -> u64 {
    0
}

#[cfg(target_os = "macos")]
fn current_process_rss_mb() -> u32 {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct MachTimeValue {
        seconds: u32,
        microseconds: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: MachTimeValue,
        system_time: MachTimeValue,
        policy: i32,
        suspend_count: i32,
    }

    const MACH_TASK_BASIC_INFO: u32 = 20;
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn task_info(
            target_task: u32,
            flavor: u32,
            task_info_out: *mut MachTaskBasicInfo,
            task_info_out_cnt: *mut u32,
        ) -> i32;
    }

    let mut info = MachTaskBasicInfo::default();
    let mut count = (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<u32>()) as u32;
    let kr = unsafe { task_info(mach_task_self_, MACH_TASK_BASIC_INFO, &mut info, &mut count) };
    if kr == 0 {
        (info.resident_size / (1024 * 1024)) as u32
    } else {
        0
    }
}

#[cfg(target_os = "linux")]
fn current_process_rss_mb() -> u32 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    ((pages * 4096) / (1024 * 1024)) as u32
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn current_process_rss_mb() -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_sampler_reports_nonzero_rss_and_valid_window() {
        let mut sampler = ProcessResourceSampler::new();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let (dt, _cpu, rss) = sampler.sample();
        assert!(dt > 0.0);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert!(rss > 0, "process RSS should be positive");
    }

    #[test]
    fn perf_counters_compute_fps_latency_and_bitrate() {
        let c = HuddlePerfCounters::default();
        c.set_tx_backend("ScreenCaptureKit + VideoToolbox (HW)");
        c.record_tx_frame(1856, 1200, 12_500, 1_100);
        c.record_tx_frame(1856, 1200, 12_500, 1_300);
        let snap = c.sample(1.0, 25, 140);
        assert_eq!(snap.cpu_tenths_pct, 25);
        assert_eq!(snap.rss_mb, 140);
        assert_eq!(snap.tx_resolution.as_deref(), Some("1856x1200"));
        assert_eq!(snap.tx_fps_tenths, 20); // 2.0 fps
        assert_eq!(snap.tx_encode_us, 1200);
        assert_eq!(snap.tx_kbps, 200); // 25_000 bytes * 8 / 1000
    }
}
