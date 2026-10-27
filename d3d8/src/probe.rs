//! The C side of the probe of the wgpu renderer (`mge_probe`, the crate under `crates/probe`,
//! where the logic and its tests are): the call of d3d8.dll before it chooses its path.

use std::ffi::{c_char, CStr};

/// The probe for the C++ side: `exe` is the path of numidium.exe, `exit_gpu` the code of a
/// refusal (`MGE_PROBE_EXIT_GPU`), `seconds` the limit; the reason, or the backend to use,
/// goes into `reason` (at most `capacity` bytes with the end). Returns 1 when the GPU is
/// usable, 0 when not.
///
/// # Safety
/// - `exe` is a NUL-terminated string that lives through the call, or null.
/// - `reason` points at `capacity` writable bytes, or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_probe_wgpu(exe: *const c_char, exit_gpu: i32, seconds: u32, reason: *mut c_char, capacity: usize) -> u32 {
    // SAFETY: the caller gives a NUL-terminated string or null.
    let exe = if exe.is_null() { "numidium.exe".to_string() } else { unsafe { CStr::from_ptr(exe) }.to_string_lossy().into_owned() };
    let answer = mge_probe::run(&exe, exit_gpu, u64::from(seconds));
    if !reason.is_null() && capacity > 0 {
        let bytes = answer.reason.as_bytes();
        let length = bytes.len().min(capacity - 1);
        // SAFETY: `reason` has `capacity` bytes; at most `capacity - 1` are copied and one
        // zero ends the text.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), reason.cast::<u8>(), length);
            *reason.add(length) = 0;
        }
    }
    u32::from(answer.usable)
}
