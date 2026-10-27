//! The C side of the probe of the wgpu renderer (`mge_probe`, the crate under `crates/probe`,
//! where the logic and its tests are): the read of the cached answer in `DllMain`, and the
//! check of the key with the probe when it changed, when the plugin or the game first asks.

use std::ffi::{c_char, CStr};

/// A C string as a Rust string, or `fallback` for a null pointer.
///
/// # Safety
/// `text` is a NUL-terminated string that lives through the call, or null.
unsafe fn text_of(text: *const c_char, fallback: &str) -> String {
    if text.is_null() {
        fallback.to_string()
    } else {
        // SAFETY: the caller gives a NUL-terminated string.
        unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned()
    }
}

/// Copies `text` into `out` (at most `capacity` bytes with the end).
///
/// # Safety
/// `out` points at `capacity` writable bytes, or is null.
unsafe fn copy_out(text: &str, out: *mut c_char, capacity: usize) {
    if out.is_null() || capacity == 0 {
        return;
    }
    let bytes = text.as_bytes();
    let length = bytes.len().min(capacity - 1);
    // SAFETY: `out` has `capacity` bytes; at most `capacity - 1` are copied and one zero ends
    // the text.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), length);
        *out.add(length) = 0;
    }
}

/// The cached answer of the probe, for `DllMain` (a file read only): 0 none, 1 usable, 2 no
/// cache or a file that is not one. The reason of the cached answer goes into `reason`.
///
/// # Safety
/// - `path` is a NUL-terminated string that lives through the call, or null.
/// - `reason` points at `capacity` writable bytes, or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_probe_cached(path: *const c_char, reason: *mut c_char, capacity: usize) -> u32 {
    // SAFETY: the caller's pointers, as documented.
    let path = unsafe { text_of(path, "mgeXE-renderer-probe.txt") };
    match mge_probe::read_cache(&path) {
        Some(cached) => {
            // SAFETY: as documented.
            unsafe { copy_out(&cached.answer.reason, reason, capacity) };
            u32::from(cached.answer.usable)
        }
        None => 2,
    }
}

/// The answer for this machine (`mge_probe::check`): the cache's for the same `key`, else
/// `numidium.exe --probe`'s (`exe`, the refusal code `exit_gpu`, the limit `seconds`), which
/// then goes into the cache at `path`. Returns 0 a probe's none, 1 a probe's usable, 2 a
/// cached none, 3 a cached usable; the reason goes into `reason`.
///
/// # Safety
/// - `exe`, `path` and `key` are NUL-terminated strings that live through the call, or null.
/// - `reason` points at `capacity` writable bytes, or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_probe_check(exe: *const c_char, path: *const c_char, key: *const c_char, exit_gpu: i32, seconds: u32, reason: *mut c_char, capacity: usize) -> u32 {
    // SAFETY: the caller's pointers, as documented.
    let (exe, path, key) = unsafe { (text_of(exe, "numidium.exe"), text_of(path, "mgeXE-renderer-probe.txt"), text_of(key, "")) };
    let (answer, source) = mge_probe::check(&exe, &path, &key, exit_gpu, u64::from(seconds));
    // SAFETY: as documented.
    unsafe { copy_out(&answer.reason, reason, capacity) };
    let cached = match source {
        mge_probe::Source::Cache => 2,
        mge_probe::Source::Probe => 0,
    };
    cached + u32::from(answer.usable)
}
