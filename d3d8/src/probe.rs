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
/// answer to use (no cache, a file that is not one, or a "no answer" line: the probe asks
/// again). The reason of the cached line goes into `reason`.
///
/// # Safety
/// - `path` is a NUL-terminated string that lives through the call, or null.
/// - `reason` points at `capacity` writable bytes, or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_probe_cached(path: *const c_char, reason: *mut c_char, capacity: usize) -> u32 {
    // SAFETY: the caller's pointers, as documented.
    let path = unsafe { text_of(path, "mgeXE-renderer-probe.txt") };
    let Some(cached) = mge_probe::read_cache(&path) else {
        return 2;
    };
    // SAFETY: as documented.
    unsafe { copy_out(&cached.reason, reason, capacity) };
    match cached.said {
        mge_probe::Said::None => 0,
        mge_probe::Said::Usable => 1,
        mge_probe::Said::NoAnswer => 2,
    }
}

/// The verdict for this machine (`mge_probe::check`): the cache's for the same `key`, else
/// `numidium.exe --probe`'s (`exe`, the refusal code `exit_gpu`, the limit `seconds`), whose
/// line then goes into the cache at `path`. Returns the number of `mge_probe::Verdict`
/// (`MgeProbeVerdict` of mge_probe_ffi.h); the reason goes into `reason`, and the probe's own
/// lines for mgeXE.log into `lines`, one per line end (empty when no probe ran).
///
/// # Safety
/// - `exe`, `path` and `key` are NUL-terminated strings that live through the call, or null.
/// - `reason` points at `capacity` writable bytes, or is null.
/// - `lines` points at `lines_capacity` writable bytes, or is null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_probe_check(
    exe: *const c_char,
    path: *const c_char,
    key: *const c_char,
    exit_gpu: i32,
    seconds: u32,
    reason: *mut c_char,
    capacity: usize,
    lines: *mut c_char,
    lines_capacity: usize,
) -> u32 {
    // SAFETY: the caller's pointers, as documented.
    let (exe, path, key) = unsafe { (text_of(exe, "numidium.exe"), text_of(path, "mgeXE-renderer-probe.txt"), text_of(key, "")) };
    let outcome = mge_probe::check(&exe, &path, &key, exit_gpu, u64::from(seconds));
    // SAFETY: as documented.
    unsafe {
        copy_out(&outcome.reason, reason, capacity);
        copy_out(&outcome.lines.join("\n"), lines, lines_capacity);
    }
    outcome.verdict as u32
}
