use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use cpal::{Stream, StreamConfig};

use super::dsp::compute_rms_interleaved;
use crate::{HuddleError, Result};

/// One enumerated audio device (mic or speaker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// Available input and output audio devices on the current host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioDevices {
    pub inputs: Vec<AudioDeviceInfo>,
    pub outputs: Vec<AudioDeviceInfo>,
}

/// Human-readable device name via `cpal 0.18` `DeviceTrait::description`.
pub(crate) fn cpal_device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_owned())
}

/// Enumerate all host microphones and speakers.
pub fn list_audio_devices() -> AudioDevices {
    let host = cpal::default_host();
    let default_in = host
        .default_input_device()
        .and_then(|d| cpal_device_name(&d));
    let default_out = host
        .default_output_device()
        .and_then(|d| cpal_device_name(&d));

    let inputs = host
        .input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|d| cpal_device_name(&d))
        .map(|name| AudioDeviceInfo {
            is_default: default_in.as_deref() == Some(name.as_str()),
            name,
        })
        .collect();

    let outputs = host
        .output_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|d| cpal_device_name(&d))
        .map(|name| AudioDeviceInfo {
            is_default: default_out.as_deref() == Some(name.as_str()),
            name,
        })
        .collect();

    AudioDevices { inputs, outputs }
}

pub(super) fn current_default_input_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .and_then(|d| cpal_device_name(&d))
}

pub(super) fn current_default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| cpal_device_name(&d))
}

pub(super) fn select_input_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = name.filter(|s| !s.is_empty())
        && let Ok(mut devices) = host.input_devices()
        && let Some(found) = devices.find(|d| cpal_device_name(d).as_deref() == Some(target))
    {
        return Ok(found);
    }
    host.default_input_device()
        .ok_or_else(|| HuddleError::Audio("no input microphone device found".to_owned()))
}

pub(super) fn select_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(target) = name.filter(|s| !s.is_empty())
        && let Ok(mut devices) = host.output_devices()
        && let Some(found) = devices.find(|d| cpal_device_name(d).as_deref() == Some(target))
    {
        return Ok(found);
    }
    host.default_output_device()
        .ok_or_else(|| HuddleError::Audio("no output speaker device found".to_owned()))
}

/// Play a short two-note harmonic chime on `output_device` (or the default speaker) for `F-COLLAB-040`.
pub fn play_speaker_test_tone(output_device: Option<&str>) -> Result<()> {
    let device_name = output_device.map(ToOwned::to_owned);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

    std::thread::Builder::new()
        .name("slackless-speaker-test".to_owned())
        .spawn(move || {
            let start = (|| -> Result<Stream> {
                let device = select_output_device(device_name.as_deref())?;
                let supported = device
                    .default_output_config()
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;
                let rate = supported.sample_rate() as f32;
                let channels = supported.channels() as usize;
                let total_frames = (rate * 0.45) as usize;
                let frame_ix = Arc::new(AtomicU32::new(0));
                let ix_cb = Arc::clone(&frame_ix);

                let config: StreamConfig = supported.into();
                let stream = device
                    .build_output_stream(
                        config,
                        move |data: &mut [f32], _| {
                            let mut ix = ix_cb.load(Ordering::Relaxed) as usize;
                            for frame in data.chunks_mut(channels) {
                                let sample = if ix < total_frames {
                                    let t = ix as f32 / rate;
                                    let freq = if t < 0.22 { 523.25 } else { 659.25 }; // C5 -> E5
                                    let env = (1.0 - (t / 0.45)).clamp(0.0, 1.0) * 0.22;
                                    (2.0 * std::f32::consts::PI * freq * t).sin() * env
                                } else {
                                    0.0
                                };
                                for ch in frame.iter_mut() {
                                    *ch = sample;
                                }
                                ix += 1;
                            }
                            ix_cb.store(ix as u32, Ordering::Relaxed);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;

                stream
                    .play()
                    .map_err(|e| HuddleError::Audio(e.to_string()))?;
                Ok(stream)
            })();

            match start {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    std::thread::sleep(Duration::from_millis(520));
                    drop(stream);
                }
                Err(err) => {
                    let _ = ready_tx.send(Err(err));
                }
            }
        })
        .map_err(|e| HuddleError::Audio(e.to_string()))?;

    ready_rx
        .recv()
        .map_err(|e| HuddleError::Audio(e.to_string()))?
}

/// Lightweight live microphone level probe for Preferences -> Audio & video (`F-COLLAB-048`).
pub struct MicLevelProbe {
    running: Arc<AtomicBool>,
    rms_bits: Arc<AtomicU32>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl MicLevelProbe {
    pub fn start(input_device: Option<&str>) -> Result<Self> {
        let device_name = input_device.map(ToOwned::to_owned);
        let running = Arc::new(AtomicBool::new(true));
        let rms_bits = Arc::new(AtomicU32::new(0));
        let run_flag = Arc::clone(&running);
        let rms_cb = Arc::clone(&rms_bits);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<()>>(1);

        let worker = std::thread::Builder::new()
            .name("slackless-mic-probe".to_owned())
            .spawn(move || {
                let start = (|| -> Result<Stream> {
                    let device = select_input_device(device_name.as_deref())?;
                    let supported = device
                        .default_input_config()
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    let channels = supported.channels() as usize;
                    let config: StreamConfig = supported.into();

                    let stream = device
                        .build_input_stream(
                            config,
                            move |data: &[f32], _| {
                                let rms = compute_rms_interleaved(data, channels);
                                rms_cb.store(rms.to_bits(), Ordering::Relaxed);
                            },
                            |_| {},
                            None,
                        )
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    stream
                        .play()
                        .map_err(|e| HuddleError::Audio(e.to_string()))?;
                    Ok(stream)
                })();

                match start {
                    Ok(stream) => {
                        let _ = ready_tx.send(Ok(()));
                        while run_flag.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(25));
                        }
                        drop(stream);
                    }
                    Err(err) => {
                        let _ = ready_tx.send(Err(err));
                    }
                }
            })
            .map_err(|e| HuddleError::Audio(e.to_string()))?;

        ready_rx
            .recv()
            .map_err(|e| HuddleError::Audio(e.to_string()))??;

        Ok(Self {
            running,
            rms_bits,
            worker: Some(worker),
        })
    }

    /// Current microphone RMS level normalized to `0.0..=1.0`.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.rms_bits.load(Ordering::Relaxed))
    }
}

impl Drop for MicLevelProbe {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
