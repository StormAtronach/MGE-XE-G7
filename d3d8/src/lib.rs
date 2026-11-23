use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::{Once, OnceLock};
use std::time::Duration;

use mge_config::ffi::FfiDocument;
use mge_config::post_values::{self, Saver};

unsafe extern "C" {
    fn mge_log_panic(message: *const c_char);
    // A line of mgeXE.log; any thread may call it (log.cpp).
    fn mge_log_line(text: *const c_char);
}

fn log_line(text: &str) {
    if let Ok(line) = CString::new(text.replace('\0', "")) {
        // SAFETY: line remains live for the duration of the call.
        unsafe { mge_log_line(line.as_ptr()) };
    }
}

/// The text of a C string, or `None` for a null pointer or a text that is not UTF-8.
///
/// # Safety
/// - `text` must be null or a valid, NUL-terminated C string pointer.
unsafe fn text_of<'a>(text: *const c_char) -> Option<&'a str> {
    if text.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(text) }.to_str().ok()
}

/// The time from the last change of a shader's values to the write of its values file.
const VALUES_DELAY: Duration = Duration::from_secs(1);

/// The thread that writes the values files, started at the first write.
fn saver() -> &'static Saver {
    static SAVER: OnceLock<Saver> = OnceLock::new();
    SAVER.get_or_init(|| Saver::new(VALUES_DELAY, log_line))
}

fn install_panic_hook() {
    static INSTALL: Once = Once::new();

    // Release builds abort, so preserve the diagnostic before Windows terminates the game.
    INSTALL.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let mut text = format!("Rust {info}");
            text.retain(|character| character != '\0');
            if let Ok(message) = CString::new(text) {
                // SAFETY: message remains live for the duration of the call.
                unsafe { mge_log_panic(message.as_ptr()) };
            }
        }));
    });
}

/// Opens the configuration document at `path`.
///
/// # Safety
/// - `path` must be a valid, NUL-terminated C string pointer or null.
/// - `out_document` must be a valid, writable pointer to receive the document handle pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_open(path: *const c_char, out_document: *mut *mut FfiDocument) -> u32 {
    install_panic_hook();
    unsafe { mge_config::ffi::open(path, out_document) as u32 }
}

/// Closes and frees a configuration document created by [`mge_config_open`].
///
/// # Safety
/// - `document` must be a valid pointer returned by [`mge_config_open`] or null.
/// - The document pointer must not be used after being passed to this function.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_close(document: *mut FfiDocument) {
    unsafe { mge_config::ffi::close(document) }
}

/// Retrieves a numeric value at key `path`.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
/// - `path` must be a valid, NUL-terminated C string pointer.
/// - `out_value` must be a valid, writable pointer for an `f64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_get_num(document: *const FfiDocument, path: *const c_char, out_value: *mut f64) -> u32 {
    unsafe { mge_config::ffi::get_number(document, path, out_value) as u32 }
}

/// Sets a numeric value at key `path`.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
/// - `path` must be a valid, NUL-terminated C string pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_set_num(document: *mut FfiDocument, path: *const c_char, value: f64) -> u32 {
    unsafe { mge_config::ffi::set_number(document, path, value) as u32 }
}

/// Retrieves a string value at key `path`.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
/// - `path` must be a valid, NUL-terminated C string pointer.
/// - `output` must point to a buffer of at least `capacity` bytes capable of receiving the string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_get_str(
    document: *const FfiDocument,
    path: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> u32 {
    unsafe { mge_config::ffi::get_string(document, path, output, capacity) as u32 }
}

/// Sets a string value at key `path`.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
/// - `path` and `value` must be valid, NUL-terminated C string pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_set_str(document: *mut FfiDocument, path: *const c_char, value: *const c_char) -> u32 {
    unsafe { mge_config::ffi::set_string(document, path, value) as u32 }
}

/// Retrieves a list of lines/strings at key `path` as newline-separated content.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
/// - `path` must be a valid, NUL-terminated C string pointer.
/// - `output` must point to a buffer of at least `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_get_lines(
    document: *const FfiDocument,
    path: *const c_char,
    output: *mut c_char,
    capacity: usize,
) -> u32 {
    unsafe { mge_config::ffi::get_lines(document, path, output, capacity) as u32 }
}

/// Reloads the configuration document from disk.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_reload(document: *mut FfiDocument) -> u32 {
    unsafe { mge_config::ffi::reload(document) as u32 }
}

/// Saves the configuration document to disk.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_save(document: *mut FfiDocument) -> u32 {
    unsafe { mge_config::ffi::save(document) as u32 }
}

/// Checks if the document was newly created/missing defaults.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_needs_creation(document: *const FfiDocument) -> u32 {
    unsafe { mge_config::ffi::needs_creation(document) }
}

/// Copies the last error message into `output`.
///
/// # Safety
/// - `document` must be a valid pointer to an open [`FfiDocument`].
/// - `output` must point to a buffer of at least `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_config_last_error(document: *const FfiDocument, output: *mut c_char, capacity: usize) -> u32 {
    unsafe { mge_config::ffi::last_error(document, output, capacity) as u32 }
}

/// The number of the settings of the port of the post shader `shader`
/// ([`post_values::PORTS`]); 0 for a shader whose port has none.
///
/// # Safety
/// - `shader` must be null or a valid, NUL-terminated C string pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_setting_count(shader: *const c_char) -> u32 {
    install_panic_hook();
    unsafe { text_of(shader) }.map_or(0, |shader| post_values::settings_of(shader).len() as u32)
}

/// The setting `index` of the port of `shader`: its name into `name` (at most `capacity`
/// bytes with the end zero) and its default into `default`. Returns 1, or 0 for no such
/// setting or a name that does not fit; then the outputs do not change.
///
/// # Safety
/// - `shader` must be null or a valid, NUL-terminated C string pointer.
/// - `name` must point to a buffer of at least `capacity` bytes.
/// - `default` must be a valid, writable pointer for an `f32`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_setting(
    shader: *const c_char,
    index: u32,
    name: *mut c_char,
    capacity: usize,
    default: *mut f32,
) -> u32 {
    install_panic_hook();
    let Some(shader) = (unsafe { text_of(shader) }) else {
        return 0;
    };
    let Some(setting) = post_values::settings_of(shader).get(index as usize) else {
        return 0;
    };
    let bytes = setting.name.as_bytes();
    if bytes.len() >= capacity {
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), name.cast::<u8>(), bytes.len());
        *name.add(bytes.len()) = 0;
        *default = setting.default[0];
    }
    1
}

/// Reads the values file of `shader` in `folder` (`<shader>.values.toml`). `values` and
/// `given` have `count` places, in the order of the settings: a value of the file goes into
/// its place of `values`, and its byte of `given` becomes 1. Each part of the file that is
/// not used gives a line of the log. Returns the number of values given.
///
/// # Safety
/// - `folder` and `shader` must be null or valid, NUL-terminated C string pointers.
/// - `values` and `given` must point to `count` writable places each.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_values_read(
    folder: *const c_char,
    shader: *const c_char,
    values: *mut f32,
    given: *mut u8,
    count: u32,
) -> u32 {
    install_panic_hook();
    let (Some(folder), Some(shader)) = (unsafe { text_of(folder) }, unsafe { text_of(shader) }) else {
        return 0;
    };
    let settings = post_values::settings_of(shader);
    let read = post_values::read_values(&post_values::values_path(Path::new(folder), shader), settings);
    for warning in &read.warnings {
        log_line(&format!("!! Post shader values: {warning}"));
    }
    let mut number = 0;
    for (index, value) in read.values.iter().enumerate().take(count as usize) {
        if let Some(value) = value {
            unsafe {
                *values.add(index) = *value;
                *given.add(index) = 1;
            }
            number += 1;
        }
    }
    number
}

/// Queues a write of the values file of `shader` in `folder` with `values` (`count` floats,
/// in the order of the settings). The thread of the values files writes it about 1 s after
/// the last change: never on the caller's thread.
///
/// # Safety
/// - `folder` and `shader` must be null or valid, NUL-terminated C string pointers.
/// - `values` must point to `count` readable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_values_queue(
    folder: *const c_char,
    shader: *const c_char,
    values: *const f32,
    count: u32,
) -> u32 {
    install_panic_hook();
    let (Some(folder), Some(shader)) = (unsafe { text_of(folder) }, unsafe { text_of(shader) }) else {
        return 0;
    };
    let settings = post_values::settings_of(shader);
    if settings.is_empty() || count as usize != settings.len() {
        return 0;
    }
    let values = unsafe { std::slice::from_raw_parts(values, count as usize) };
    let text = post_values::values_text(shader, settings, values);
    saver().queue(post_values::values_path(Path::new(folder), shader), text);
    1
}
