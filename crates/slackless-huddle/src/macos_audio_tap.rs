//! Zero-driver macOS 14.2+ system audio capture using CoreAudio Process Taps
//! (`CATapDescription` + `AudioHardwareCreateProcessTap` + private aggregate device).
//!
//! Requires zero third-party virtual audio drivers (no BlackHole/Loopback needed). macOS prompts
//! once for System Audio Recording permission when the tap starts; if denied or unavailable on
//! older macOS versions, callers fall back cleanly to virtual loopback devices or silence.

#![cfg(target_os = "macos")]
#![allow(non_snake_case, non_upper_case_globals)]

use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::{Arc, Mutex};

use crate::{HuddleError, Result};

type OSStatus = i32;
type AudioObjectID = u32;
type AudioDeviceID = AudioObjectID;
type AudioDeviceIOProcID = *mut c_void;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFArrayRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFAllocatorRef = *const c_void;

const kAudioObjectSystemObject: AudioObjectID = 1;
const kAudioObjectPropertyScopeGlobal: u32 = u32::from_be_bytes(*b"glob");
const kAudioObjectPropertyElementMain: u32 = 0;
const kAudioTapPropertyUID: u32 = u32::from_be_bytes(*b"tuid");
const kAudioTapPropertyFormat: u32 = u32::from_be_bytes(*b"tfmt");
const kAudioFormatFlagIsNonInterleaved: u32 = 1 << 5;
const kCFStringEncodingUTF8: u32 = 0x0800_0100;

#[repr(C)]
struct AudioObjectPropertyAddress {
    mSelector: u32,
    mScope: u32,
    mElement: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct AudioStreamBasicDescription {
    mSampleRate: f64,
    mFormatID: u32,
    mFormatFlags: u32,
    mBytesPerPacket: u32,
    mFramesPerPacket: u32,
    mBytesPerFrame: u32,
    mChannelsPerFrame: u32,
    mBitsPerChannel: u32,
    mReserved: u32,
}

#[repr(C)]
struct AudioBuffer {
    mNumberChannels: u32,
    mDataByteSize: u32,
    mData: *mut c_void,
}

#[repr(C)]
struct AudioBufferList {
    mNumberBuffers: u32,
    mBuffers: [AudioBuffer; 1],
}

type AudioDeviceIOProc = unsafe extern "C" fn(
    inDevice: AudioDeviceID,
    inNow: *const c_void,
    inInputData: *const AudioBufferList,
    inInputTime: *const c_void,
    outOutputData: *mut AudioBufferList,
    inOutputTime: *const c_void,
    inClientData: *mut c_void,
) -> OSStatus;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioHardwareCreateProcessTap(
        inDescription: *const c_void,
        outTapID: *mut AudioObjectID,
    ) -> OSStatus;
    fn AudioHardwareDestroyProcessTap(inTapID: AudioObjectID) -> OSStatus;
    fn AudioHardwareCreateAggregateDevice(
        inDescription: CFDictionaryRef,
        outDeviceID: *mut AudioDeviceID,
    ) -> OSStatus;
    fn AudioHardwareDestroyAggregateDevice(inDeviceID: AudioDeviceID) -> OSStatus;
    fn AudioObjectGetPropertyData(
        inObjectID: AudioObjectID,
        inAddress: *const AudioObjectPropertyAddress,
        inQualifierDataSize: u32,
        inQualifierData: *const c_void,
        ioDataSize: *mut u32,
        outData: *mut c_void,
    ) -> OSStatus;
    fn AudioDeviceCreateIOProcID(
        inDevice: AudioDeviceID,
        inProc: AudioDeviceIOProc,
        inClientData: *mut c_void,
        outIOProcID: *mut AudioDeviceIOProcID,
    ) -> OSStatus;
    fn AudioDeviceDestroyIOProcID(
        inDevice: AudioDeviceID,
        inIOProcID: AudioDeviceIOProcID,
    ) -> OSStatus;
    fn AudioDeviceStart(inDevice: AudioDeviceID, inProcID: AudioDeviceIOProcID) -> OSStatus;
    fn AudioDeviceStop(inDevice: AudioDeviceID, inProcID: AudioDeviceIOProcID) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFAllocatorDefault: CFAllocatorRef;
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFTypeArrayCallBacks: c_void;

    fn CFStringCreateWithCString(
        alloc: CFAllocatorRef,
        cStr: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFDictionaryCreate(
        allocator: CFAllocatorRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        numValues: isize,
        keyCallBacks: *const c_void,
        valueCallBacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFArrayCreate(
        allocator: CFAllocatorRef,
        values: *const CFTypeRef,
        numValues: isize,
        callBacks: *const c_void,
    ) -> CFArrayRef;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *const c_void;
    fn sel_registerName(str: *const c_char) -> *const c_void;
    fn objc_msgSend();
}

unsafe fn cf_str(s: &CStr) -> CFStringRef {
    unsafe { CFStringCreateWithCString(kCFAllocatorDefault, s.as_ptr(), kCFStringEncodingUTF8) }
}

/// Returns `true` when macOS 14.2+ CoreAudio Process Taps (`CATapDescription`) are available at runtime.
pub fn supports_native_system_audio_tap() -> bool {
    // SAFETY: `objc_getClass` safely returns null if `CATapDescription` does not exist.
    unsafe { !objc_getClass(c"CATapDescription".as_ptr()).is_null() }
}

struct TapCallbackCtx {
    queue: Arc<Mutex<VecDeque<i16>>>,
    non_interleaved: bool,
    channels: u32,
}

unsafe extern "C" fn tap_io_proc(
    _inDevice: AudioDeviceID,
    _inNow: *const c_void,
    inInputData: *const AudioBufferList,
    _inInputTime: *const c_void,
    _outOutputData: *mut AudioBufferList,
    _inOutputTime: *const c_void,
    inClientData: *mut c_void,
) -> OSStatus {
    if inInputData.is_null() || inClientData.is_null() {
        return 0;
    }
    // SAFETY: `inClientData` is a valid `TapCallbackCtx` alive until `AudioDeviceDestroyIOProcID`.
    let ctx = unsafe { &*(inClientData as *const TapCallbackCtx) };
    // SAFETY: CoreAudio passes a valid `AudioBufferList` containing `mNumberBuffers` entries.
    let list = unsafe { &*inInputData };
    let num_bufs = list.mNumberBuffers as usize;
    if num_bufs == 0 {
        return 0;
    }
    // SAFETY: Flexible array tail of `AudioBufferList`.
    let buffers = unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), num_bufs) };

    let mut stereo_out = Vec::<i16>::new();
    if ctx.non_interleaved && num_bufs >= 2 {
        let b0 = &buffers[0];
        let b1 = &buffers[1];
        if !b0.mData.is_null() && !b1.mData.is_null() {
            let n0 = (b0.mDataByteSize as usize) / std::mem::size_of::<f32>();
            let n1 = (b1.mDataByteSize as usize) / std::mem::size_of::<f32>();
            let frames = n0.min(n1);
            // SAFETY: CoreAudio guarantees `mData` holds `mDataByteSize` bytes of f32 PCM.
            let left = unsafe { std::slice::from_raw_parts(b0.mData as *const f32, frames) };
            let right = unsafe { std::slice::from_raw_parts(b1.mData as *const f32, frames) };
            stereo_out.reserve(frames * 2);
            for i in 0..frames {
                stereo_out.push((left[i].clamp(-1.0, 1.0) * 32767.0) as i16);
                stereo_out.push((right[i].clamp(-1.0, 1.0) * 32767.0) as i16);
            }
        }
    } else {
        let b0 = &buffers[0];
        if !b0.mData.is_null() {
            let total_f32 = (b0.mDataByteSize as usize) / std::mem::size_of::<f32>();
            let ch = (b0.mNumberChannels.max(ctx.channels).max(1)) as usize;
            // SAFETY: CoreAudio guarantees `mData` holds `mDataByteSize` bytes of f32 PCM.
            let samples = unsafe { std::slice::from_raw_parts(b0.mData as *const f32, total_f32) };
            stereo_out.reserve((total_f32 / ch) * 2);
            for frame in samples.chunks(ch) {
                let l = (frame[0].clamp(-1.0, 1.0) * 32767.0) as i16;
                let r =
                    (frame.get(1).copied().unwrap_or(frame[0]).clamp(-1.0, 1.0) * 32767.0) as i16;
                stereo_out.push(l);
                stereo_out.push(r);
            }
        }
    }

    if !stereo_out.is_empty() {
        if let Ok(mut q) = ctx.queue.try_lock() {
            if q.len() > 48_000 * 4 {
                q.drain(..48_000);
            }
            q.extend(stereo_out);
        }
    }
    0
}

/// Live macOS 14.2+ CoreAudio Process Tap streaming stereo 16-bit PCM into `queue`.
pub struct SystemAudioTap {
    tap_id: AudioObjectID,
    agg_device_id: AudioDeviceID,
    io_proc_id: AudioDeviceIOProcID,
    desc_obj: *const c_void,
    ctx: *mut TapCallbackCtx,
}

// SAFETY: CoreAudio object IDs and pointers are only torn down once in `Drop` after stopping IOProc.
unsafe impl Send for SystemAudioTap {}

impl SystemAudioTap {
    /// Create a global stereo CoreAudio Process Tap and start streaming PCM into `queue`.
    pub fn start(queue: Arc<Mutex<VecDeque<i16>>>) -> Result<Self> {
        if !supports_native_system_audio_tap() {
            return Err(HuddleError::Audio(
                "CoreAudio Process Tap requires macOS 14.2+".into(),
            ));
        }

        // SAFETY: All Objective-C and CoreAudio C calls check status codes and release owned handles on error.
        unsafe {
            let tap_cls = objc_getClass(c"CATapDescription".as_ptr());
            let arr_cls = objc_getClass(c"NSArray".as_ptr());
            if tap_cls.is_null() || arr_cls.is_null() {
                return Err(HuddleError::Audio("CATapDescription unavailable".into()));
            }

            let msg_0: unsafe extern "C" fn(*const c_void, *const c_void) -> *const c_void =
                std::mem::transmute(objc_msgSend as *const ());
            let msg_1_ptr: unsafe extern "C" fn(
                *const c_void,
                *const c_void,
                *const c_void,
            ) -> *const c_void = std::mem::transmute(objc_msgSend as *const ());
            let msg_1_bool: unsafe extern "C" fn(*const c_void, *const c_void, i8) =
                std::mem::transmute(objc_msgSend as *const ());
            let msg_1_i64: unsafe extern "C" fn(*const c_void, *const c_void, i64) =
                std::mem::transmute(objc_msgSend as *const ());
            let msg_void: unsafe extern "C" fn(*const c_void, *const c_void) =
                std::mem::transmute(objc_msgSend as *const ());

            let empty_arr = msg_0(arr_cls, sel_registerName(c"array".as_ptr()));
            let alloc_desc = msg_0(tap_cls, sel_registerName(c"alloc".as_ptr()));
            let desc = msg_1_ptr(
                alloc_desc,
                sel_registerName(c"initStereoGlobalTapButExcludeProcesses:".as_ptr()),
                empty_arr,
            );
            if desc.is_null() {
                return Err(HuddleError::Audio(
                    "failed to initialize CATapDescription".into(),
                ));
            }
            // CATapUnmuted = 0 so local playback continues normally.
            msg_1_i64(desc, sel_registerName(c"setMuteBehavior:".as_ptr()), 0);
            msg_1_bool(desc, sel_registerName(c"setPrivateTap:".as_ptr()), 1);

            let mut tap_id: AudioObjectID = 0;
            let status = AudioHardwareCreateProcessTap(desc, &mut tap_id);
            if status != 0 || tap_id == 0 {
                msg_void(desc, sel_registerName(c"release".as_ptr()));
                return Err(HuddleError::Audio(format!(
                    "AudioHardwareCreateProcessTap failed (OSStatus {status})"
                )));
            }

            // Query tap UID (CFStringRef) and tap format (AudioStreamBasicDescription).
            let mut tap_uid: CFStringRef = ptr::null();
            let mut uid_size = std::mem::size_of::<CFStringRef>() as u32;
            let uid_addr = AudioObjectPropertyAddress {
                mSelector: kAudioTapPropertyUID,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            };
            let st_uid = AudioObjectGetPropertyData(
                tap_id,
                &uid_addr,
                0,
                ptr::null(),
                &mut uid_size,
                (&mut tap_uid as *mut CFStringRef).cast(),
            );
            if st_uid != 0 || tap_uid.is_null() {
                let _ = AudioHardwareDestroyProcessTap(tap_id);
                msg_void(desc, sel_registerName(c"release".as_ptr()));
                return Err(HuddleError::Audio(format!(
                    "failed to query tap UID (OSStatus {st_uid})"
                )));
            }

            let mut asbd = AudioStreamBasicDescription::default();
            let mut asbd_size = std::mem::size_of::<AudioStreamBasicDescription>() as u32;
            let fmt_addr = AudioObjectPropertyAddress {
                mSelector: kAudioTapPropertyFormat,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain,
            };
            let _ = AudioObjectGetPropertyData(
                tap_id,
                &fmt_addr,
                0,
                ptr::null(),
                &mut asbd_size,
                (&mut asbd as *mut AudioStreamBasicDescription).cast(),
            );
            let non_interleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
            let channels = asbd.mChannelsPerFrame.max(2);

            // Build private aggregate device dictionary referencing the tap.
            let k_uid = cf_str(c"uid");
            let k_name = cf_str(c"name");
            let k_private = cf_str(c"private");
            let k_stacked = cf_str(c"stacked");
            let k_taps = cf_str(c"taps");
            let k_drift = cf_str(c"drift");

            let agg_uid_str =
                std::ffi::CString::new(format!("dev.slackless.tap.{}", std::process::id()))
                    .unwrap_or_else(|_| std::ffi::CString::new("dev.slackless.tap").unwrap());
            let v_agg_uid = cf_str(&agg_uid_str);
            let v_agg_name = cf_str(c"Slackless System Audio Tap");

            let sub_tap_keys = [k_uid, k_drift];
            let sub_tap_vals = [tap_uid, kCFBooleanTrue];
            let sub_tap_dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                sub_tap_keys.as_ptr(),
                sub_tap_vals.as_ptr(),
                2,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let tap_arr_vals = [sub_tap_dict];
            let tap_arr = CFArrayCreate(
                kCFAllocatorDefault,
                tap_arr_vals.as_ptr(),
                1,
                &kCFTypeArrayCallBacks,
            );

            let agg_keys = [k_uid, k_name, k_private, k_stacked, k_taps];
            let agg_vals = [
                v_agg_uid,
                v_agg_name,
                kCFBooleanTrue,
                kCFBooleanFalse,
                tap_arr,
            ];
            let agg_dict = CFDictionaryCreate(
                kCFAllocatorDefault,
                agg_keys.as_ptr(),
                agg_vals.as_ptr(),
                5,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );

            let mut agg_device_id: AudioDeviceID = 0;
            let st_agg = AudioHardwareCreateAggregateDevice(agg_dict, &mut agg_device_id);

            for cf in [
                agg_dict,
                tap_arr,
                sub_tap_dict,
                v_agg_name,
                v_agg_uid,
                k_drift,
                k_taps,
                k_stacked,
                k_private,
                k_name,
                k_uid,
                tap_uid,
            ] {
                if !cf.is_null() {
                    CFRelease(cf);
                }
            }

            if st_agg != 0 || agg_device_id == 0 {
                let _ = AudioHardwareDestroyProcessTap(tap_id);
                msg_void(desc, sel_registerName(c"release".as_ptr()));
                return Err(HuddleError::Audio(format!(
                    "AudioHardwareCreateAggregateDevice failed (OSStatus {st_agg})"
                )));
            }

            let ctx = Box::into_raw(Box::new(TapCallbackCtx {
                queue,
                non_interleaved,
                channels,
            }));
            let mut io_proc_id: AudioDeviceIOProcID = ptr::null_mut();
            let st_proc =
                AudioDeviceCreateIOProcID(agg_device_id, tap_io_proc, ctx.cast(), &mut io_proc_id);
            if st_proc != 0 || io_proc_id.is_null() {
                drop(Box::from_raw(ctx));
                let _ = AudioHardwareDestroyAggregateDevice(agg_device_id);
                let _ = AudioHardwareDestroyProcessTap(tap_id);
                msg_void(desc, sel_registerName(c"release".as_ptr()));
                return Err(HuddleError::Audio(format!(
                    "AudioDeviceCreateIOProcID failed (OSStatus {st_proc})"
                )));
            }

            let st_start = AudioDeviceStart(agg_device_id, io_proc_id);
            if st_start != 0 {
                let _ = AudioDeviceDestroyIOProcID(agg_device_id, io_proc_id);
                drop(Box::from_raw(ctx));
                let _ = AudioHardwareDestroyAggregateDevice(agg_device_id);
                let _ = AudioHardwareDestroyProcessTap(tap_id);
                msg_void(desc, sel_registerName(c"release".as_ptr()));
                return Err(HuddleError::Audio(format!(
                    "AudioDeviceStart failed (OSStatus {st_start})"
                )));
            }

            let _ = kAudioObjectSystemObject;
            Ok(Self {
                tap_id,
                agg_device_id,
                io_proc_id,
                desc_obj: desc,
                ctx,
            })
        }
    }
}

impl Drop for SystemAudioTap {
    fn drop(&mut self) {
        // SAFETY: Stop and destroy IOProc before freeing `ctx`, then tear down aggregate device and tap.
        unsafe {
            let _ = AudioDeviceStop(self.agg_device_id, self.io_proc_id);
            let _ = AudioDeviceDestroyIOProcID(self.agg_device_id, self.io_proc_id);
            let _ = AudioHardwareDestroyAggregateDevice(self.agg_device_id);
            let _ = AudioHardwareDestroyProcessTap(self.tap_id);
            if !self.ctx.is_null() {
                drop(Box::from_raw(self.ctx));
                self.ctx = ptr::null_mut();
            }
            if !self.desc_obj.is_null() {
                let msg_void: unsafe extern "C" fn(*const c_void, *const c_void) =
                    std::mem::transmute(objc_msgSend as *const ());
                msg_void(self.desc_obj, sel_registerName(c"release".as_ptr()));
                self.desc_obj = ptr::null();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_detects_process_tap_class_without_panicking() {
        // On macOS 14.2+ this is true; on older macOS it returns false cleanly.
        let _supported = supports_native_system_audio_tap();
    }
}
