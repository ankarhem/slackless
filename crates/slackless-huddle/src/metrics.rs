//! Live process resource usage (CPU%, RSS MiB) and video pipeline telemetry (FPS, encode/decode
//! latency, bitrate, active hardware/software backend) for evaluating real-world huddle
//! performance in `slackless.log` and the Huddle Diagnostics drawer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// Inbound packet-loss percentage threshold at or above which the video stream is degraded.
pub const DEGRADED_RX_LOSS_PCT: f32 = 5.0;
/// Outbound encode latency threshold in microseconds (40 ms) indicating encoder backpressure.
pub const DEGRADED_TX_ENCODE_US: u32 = 40_000;
/// Inbound decode latency threshold in microseconds (40 ms) indicating decoder backpressure.
pub const DEGRADED_RX_DECODE_US: u32 = 40_000;

/// Windowed performance snapshot surfaced in `HuddleSessionSnapshot` and periodic `tracing` logs.
#[derive(Debug, Clone, PartialEq, Default)]
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
    /// Inbound RTP video sequence gaps observed during this sampling window.
    pub rx_seq_gaps: u64,
    /// Estimated inbound RTP video packet loss percentage (`0.0..=100.0`) during this window.
    pub rx_loss_pct: f32,
    /// Live per-track remote video decoders (one OS thread + H.264/AV1 decoder each).
    pub rx_decoders: u32,
}

impl HuddlePerfSnapshot {
    /// Returns `true` when packet loss (`>= 5%`) or encode/decode latency (`>= 40 ms`) suggests
    /// the media pipeline is degraded and may benefit from auto-degrade / quality reduction.
    pub fn is_degraded(&self) -> bool {
        self.rx_loss_pct >= DEGRADED_RX_LOSS_PCT
            || self.tx_encode_us >= DEGRADED_TX_ENCODE_US
            || self.rx_decode_us >= DEGRADED_RX_DECODE_US
    }
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
    rx_packets: AtomicU64,
    rx_seq_gaps: AtomicU64,
    rx_frames: AtomicU64,
    rx_bytes: AtomicU64,
    rx_decode_us_sum: AtomicU64,
    rx_decoders: AtomicUsize,
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
        self.rx_packets.fetch_add(1, Ordering::Relaxed);
        self.rx_bytes.fetch_add(payload_bytes, Ordering::Relaxed);
    }

    pub fn record_rx_seq_gap(&self, n: u64) {
        self.rx_seq_gaps.fetch_add(n, Ordering::Relaxed);
    }

    /// Track how many per-track remote video decoders are live (idle ones are swept).
    pub fn set_rx_decoders(&self, count: usize) {
        self.rx_decoders.store(count, Ordering::Relaxed);
    }

    /// Number of live per-track remote video decoders.
    pub fn rx_decoders(&self) -> usize {
        self.rx_decoders.load(Ordering::Relaxed)
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
        let rx_packets = self.rx_packets.swap(0, Ordering::Relaxed);
        let rx_seq_gaps = self.rx_seq_gaps.swap(0, Ordering::Relaxed);
        let rx_frames = self.rx_frames.swap(0, Ordering::Relaxed);
        let rx_bytes = self.rx_bytes.swap(0, Ordering::Relaxed);
        let rx_us = self.rx_decode_us_sum.swap(0, Ordering::Relaxed);

        let tw = self.tx_width.load(Ordering::Relaxed);
        let th = self.tx_height.load(Ordering::Relaxed);
        let rw = self.rx_width.load(Ordering::Relaxed);
        let rh = self.rx_height.load(Ordering::Relaxed);

        let expected_rx = rx_packets.saturating_add(rx_seq_gaps);
        let rx_loss_pct = if expected_rx == 0 {
            0.0
        } else {
            ((rx_seq_gaps as f64 / expected_rx as f64) * 100.0).clamp(0.0, 100.0) as f32
        };

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
            rx_seq_gaps,
            rx_loss_pct,
            rx_decoders: self.rx_decoders.load(Ordering::Relaxed) as u32,
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
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` is a writable `rusage` for the duration of the call, and `RUSAGE_SELF`
    // asks only about this process.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return 0;
    }
    // SAFETY: `getrusage` returned 0, so it filled `usage` (which was zeroed to begin with).
    let usage = unsafe { usage.assume_init() };
    let micros = |tv: libc::timeval| {
        let secs = u64::try_from(tv.tv_sec).unwrap_or(0);
        secs.saturating_mul(1_000_000)
            .saturating_add(u64::try_from(tv.tv_usec).unwrap_or(0))
    };
    micros(usage.ru_utime).saturating_add(micros(usage.ru_stime))
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
    // SAFETY: `MachTaskBasicInfo` mirrors `mach_task_basic_info` field for field, `count` is its
    // size in 32-bit words as `task_info` expects, and `mach_task_self_` is this task's port,
    // which libSystem sets at process start.
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
        #[cfg(unix)]
        assert!(
            current_process_cpu_us() > 0,
            "getrusage should report CPU time"
        );
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

    #[test]
    fn perf_counters_reset_deltas_handle_clear_tx_and_track_rx() {
        let c = HuddlePerfCounters::default();
        c.set_tx_backend("TX-HW");
        c.record_tx_frame(1280, 720, 5_000, 800);
        c.set_rx_backend("AV1 (rav1d SW)");
        c.record_rx_packet(2_500);
        c.record_rx_packet(2_500);
        c.record_rx_frame(1920, 1080, 2_000);
        c.record_rx_frame(1920, 1080, 4_000);

        let s1 = c.sample(2.0, 15, 200);
        assert_eq!(s1.tx_backend.as_deref(), Some("TX-HW"));
        assert_eq!(s1.tx_resolution.as_deref(), Some("1280x720"));
        assert_eq!(s1.tx_fps_tenths, 5); // 0.5 fps
        assert_eq!(s1.tx_kbps, 20); // 5000*8 / 2 / 1000
        assert_eq!(s1.rx_backend.as_deref(), Some("AV1 (rav1d SW)"));
        assert_eq!(s1.rx_resolution.as_deref(), Some("1920x1080"));
        assert_eq!(s1.rx_fps_tenths, 10); // 1.0 fps
        assert_eq!(s1.rx_decode_us, 3000);
        assert_eq!(s1.rx_kbps, 20);

        // Second sample without new frames resets delta metrics to 0 while preserving backend/res.
        let s2 = c.sample(0.01, 0, 200); // dt clamps to 0.1
        assert_eq!(s2.tx_fps_tenths, 0);
        assert_eq!(s2.tx_encode_us, 0);
        assert_eq!(s2.tx_kbps, 0);
        assert_eq!(s2.rx_fps_tenths, 0);
        assert_eq!(s2.rx_decode_us, 0);
        assert_eq!(s2.rx_kbps, 0);
        assert_eq!(s2.tx_resolution.as_deref(), Some("1280x720"));
        assert_eq!(s2.rx_resolution.as_deref(), Some("1920x1080"));

        // clear_tx clears TX backend & resolution without touching RX.
        c.clear_tx();
        let s3 = c.sample(1.0, 0, 200);
        assert!(s3.tx_backend.is_none());
        assert!(s3.tx_resolution.is_none());
        assert_eq!(s3.rx_backend.as_deref(), Some("AV1 (rav1d SW)"));
        assert_eq!(s3.rx_resolution.as_deref(), Some("1920x1080"));
    }

    #[test]
    fn perf_counters_seq_gaps_loss_pct_and_degraded_thresholds() {
        let c = HuddlePerfCounters::default();

        // 1. Zero packets and zero gaps -> 0.0% loss, not degraded.
        let s_empty = c.sample(1.0, 10, 120);
        assert_eq!(s_empty.rx_seq_gaps, 0);
        assert_eq!(s_empty.rx_loss_pct, 0.0);
        assert!(!s_empty.is_degraded());

        // 2. Partial loss below 5% threshold: 96 rx packets + 4 gaps = 4.0% loss -> healthy.
        for _ in 0..96 {
            c.record_rx_packet(1000);
        }
        c.record_rx_seq_gap(4);
        let s_low_loss = c.sample(1.0, 10, 120);
        assert_eq!(s_low_loss.rx_seq_gaps, 4);
        assert!((s_low_loss.rx_loss_pct - 4.0).abs() < 0.01);
        assert!(!s_low_loss.is_degraded());

        // 3. Partial loss at 5% threshold: 95 rx packets + 5 gaps = 5.0% loss -> degraded!
        for _ in 0..95 {
            c.record_rx_packet(1000);
        }
        c.record_rx_seq_gap(2);
        c.record_rx_seq_gap(3);
        let s_threshold = c.sample(1.0, 10, 120);
        assert_eq!(s_threshold.rx_seq_gaps, 5);
        assert!((s_threshold.rx_loss_pct - 5.0).abs() < 0.01);
        assert!(s_threshold.is_degraded());

        // 4. Saturation: gaps without any received packets -> 100.0% loss, finite & clamped.
        c.record_rx_seq_gap(12);
        let s_sat = c.sample(1.0, 10, 120);
        assert_eq!(s_sat.rx_seq_gaps, 12);
        assert_eq!(s_sat.rx_loss_pct, 100.0);
        assert!(s_sat.rx_loss_pct.is_finite());
        assert!(s_sat.is_degraded());

        // 5. Window reset: next sample clears rx_seq_gaps and rx_loss_pct back to 0.
        let s_reset = c.sample(1.0, 10, 120);
        assert_eq!(s_reset.rx_seq_gaps, 0);
        assert_eq!(s_reset.rx_loss_pct, 0.0);
        assert!(!s_reset.is_degraded());

        // 6. Latency degradation checks (tx_encode_us and rx_decode_us >= 40ms).
        let snap_enc_slow = HuddlePerfSnapshot {
            tx_encode_us: DEGRADED_TX_ENCODE_US,
            ..Default::default()
        };
        assert!(snap_enc_slow.is_degraded());

        let snap_dec_slow = HuddlePerfSnapshot {
            rx_decode_us: DEGRADED_RX_DECODE_US,
            ..Default::default()
        };
        assert!(snap_dec_slow.is_degraded());

        let snap_fast = HuddlePerfSnapshot {
            tx_encode_us: DEGRADED_TX_ENCODE_US - 1,
            rx_decode_us: DEGRADED_RX_DECODE_US - 1,
            rx_loss_pct: DEGRADED_RX_LOSS_PCT - 0.1,
            ..Default::default()
        };
        assert!(!snap_fast.is_degraded());
    }
}
