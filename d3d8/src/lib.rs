use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::{Once, OnceLock};
use std::time::Duration;

use mge_config::ffi::FfiDocument;
use mge_config::post_header::{self, Header};
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

/// The header of a post shader file, read by mge-config's `post_header`: the one parser of
/// the `//!` lines, which the helper uses too. d3d8.dll lists its variables to the API of
/// MGE XE.
pub struct PostHeader {
    header: Header,
    /// The variables as the API lists them: `mgeflags` first when the file has the key, then
    /// the variables of the file.
    variables: Vec<(CString, c_char, Vec<f32>, bool)>,
}

/// What a header gives besides its variables.
#[repr(C)]
pub struct PostHeaderFacts {
    pub priority_adjust: i64,
    pub variable_count: u32,
    pub requires_hdr: u8,
    pub disable_sunglare: u8,
}

/// Copies `bytes` and a zero into `output` (`capacity` bytes). False when they do not fit;
/// then `output` does not change.
///
/// # Safety
/// - `output` must point to a buffer of at least `capacity` bytes.
unsafe fn copy_text(output: *mut c_char, capacity: usize, bytes: &[u8]) -> bool {
    if output.is_null() || bytes.len() >= capacity {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), output.cast::<u8>(), bytes.len());
        *output.add(bytes.len()) = 0;
    }
    true
}

/// Reads the header of the post shader file at `path`. Returns 1 with the header in `out`,
/// which [`mge_post_header_free`] frees; or 0 with the reason in `error` (at most `capacity`
/// bytes with the end zero, cut to fit) for a file that cannot be read or a header that the
/// renderer does not take.
///
/// # Safety
/// - `path` must be null or a valid, NUL-terminated C string pointer.
/// - `out` must be a valid, writable pointer.
/// - `error` must point to a buffer of at least `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_header_read(
    path: *const c_char,
    out: *mut *mut PostHeader,
    error: *mut c_char,
    capacity: usize,
) -> u32 {
    install_panic_hook();
    let fail = |reason: &str| {
        let mut bytes = reason.replace('\0', "").into_bytes();
        bytes.truncate(capacity.saturating_sub(1));
        unsafe { copy_text(error, capacity, &bytes) };
        0
    };
    let Some(path) = (unsafe { text_of(path) }) else {
        return fail("the path is not a text");
    };
    let text = match std::fs::read(path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return fail("the file cannot be read"),
    };
    let header = match post_header::parse(&text) {
        Ok(header) => header,
        Err(reason) => return fail(&reason),
    };
    let mut variables = Vec::new();
    if let Some(flags) = header.mgeflags {
        variables.push((c"mgeflags".to_owned(), b'i' as c_char, vec![flags as f32], false));
    }
    for variable in &header.variables {
        let Ok(name) = CString::new(variable.name.as_str()) else {
            return fail("a variable name has a zero byte");
        };
        let (letter, values) = variable.api_shape();
        variables.push((name, letter as c_char, values, header.is_setting(&variable.name)));
    }
    unsafe { *out = Box::into_raw(Box::new(PostHeader { header, variables })) };
    1
}

/// Frees a header of [`mge_post_header_read`].
///
/// # Safety
/// - `header` must be null or a header of [`mge_post_header_read`] not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_header_free(header: *mut PostHeader) {
    if !header.is_null() {
        drop(unsafe { Box::from_raw(header) });
    }
}

/// The facts of a header into `out`.
///
/// # Safety
/// - `header` must be a header of [`mge_post_header_read`]; `out` a valid, writable pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_header_facts(header: *const PostHeader, out: *mut PostHeaderFacts) {
    let header = unsafe { &*header };
    unsafe {
        *out = PostHeaderFacts {
            priority_adjust: header.header.priority_adjust,
            variable_count: header.variables.len() as u32,
            requires_hdr: u8::from(header.header.requires_hdr),
            disable_sunglare: u8::from(header.header.disable_sunglare),
        }
    };
}

/// The category of a header into `output` (empty for `none`). Returns 1, or 0 when it does
/// not fit.
///
/// # Safety
/// - `header` must be a header of [`mge_post_header_read`].
/// - `output` must point to a buffer of at least `capacity` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_header_category(header: *const PostHeader, output: *mut c_char, capacity: usize) -> u32 {
    let header = unsafe { &*header };
    u32::from(unsafe { copy_text(output, capacity, header.header.category.as_bytes()) })
}

/// The variable `index` of a header: its name into `name` (at most `capacity` bytes with the
/// end zero), its type letter of the API, the number of its values, and 1 in `setting` for a
/// setting (kept in the values file). Returns 1, or 0 for no such variable or a name that
/// does not fit; then the outputs do not change.
///
/// # Safety
/// - `header` must be a header of [`mge_post_header_read`].
/// - `name` must point to a buffer of at least `capacity` bytes.
/// - `letter`, `value_count` and `setting` must be valid, writable pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_header_variable(
    header: *const PostHeader,
    index: u32,
    name: *mut c_char,
    capacity: usize,
    letter: *mut c_char,
    value_count: *mut u32,
    setting: *mut u8,
) -> u32 {
    let header = unsafe { &*header };
    let Some((text, kind, values, is_setting)) = header.variables.get(index as usize) else {
        return 0;
    };
    if !unsafe { copy_text(name, capacity, text.as_bytes()) } {
        return 0;
    }
    unsafe {
        *letter = *kind;
        *value_count = values.len() as u32;
        *setting = u8::from(*is_setting);
    }
    1
}

/// Copies at most `count` values of the variable `index` of a header into `values`. Returns
/// the number copied.
///
/// # Safety
/// - `header` must be a header of [`mge_post_header_read`].
/// - `values` must point to `count` writable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_header_values(header: *const PostHeader, index: u32, values: *mut f32, count: u32) -> u32 {
    let header = unsafe { &*header };
    let Some((_, _, own, _)) = header.variables.get(index as usize) else {
        return 0;
    };
    let number = own.len().min(count as usize);
    unsafe { std::ptr::copy_nonoverlapping(own.as_ptr(), values, number) };
    number as u32
}

/// The `count` names of `names`, or `None` for a null pointer or a name that is not a text.
///
/// # Safety
/// - `names` must be null or point to `count` valid, NUL-terminated C string pointers.
unsafe fn names_of<'a>(names: *const *const c_char, count: u32) -> Option<Vec<&'a str>> {
    if names.is_null() {
        return None;
    }
    (0..count as usize)
        .map(|index| unsafe { text_of(*names.add(index)) })
        .collect()
}

/// Reads the values file of `shader` in `folder` (`<shader>.values.toml`) for the `count`
/// settings `names`. `values` and `given` have `count` places, in the order of `names`: a
/// value of the file goes into its place of `values`, and its byte of `given` becomes 1.
/// Each part of the file that is not used gives a line of the log. Returns the number of
/// values given.
///
/// # Safety
/// - `folder` and `shader` must be null or valid, NUL-terminated C string pointers.
/// - `names` must be null or point to `count` valid, NUL-terminated C string pointers.
/// - `values` and `given` must point to `count` writable places each.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_values_read(
    folder: *const c_char,
    shader: *const c_char,
    names: *const *const c_char,
    count: u32,
    values: *mut f32,
    given: *mut u8,
) -> u32 {
    install_panic_hook();
    let (Some(folder), Some(shader), Some(names)) = (unsafe { text_of(folder) }, unsafe { text_of(shader) }, unsafe {
        names_of(names, count)
    }) else {
        return 0;
    };
    let read = post_values::read_values(&post_values::values_path(Path::new(folder), shader), &names);
    for warning in &read.warnings {
        log_line(&format!("!! Post shader values: {warning}"));
    }
    let mut number = 0;
    for (index, value) in read.values.iter().enumerate() {
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

/// Queues a write of the values file of `shader` in `folder` with the `count` settings
/// `names` and their `values`. The thread of the values files writes it about 1 s after the
/// last change: never on the caller's thread. Returns 1, or 0 for no settings.
///
/// # Safety
/// - `folder` and `shader` must be null or valid, NUL-terminated C string pointers.
/// - `names` must be null or point to `count` valid, NUL-terminated C string pointers.
/// - `values` must point to `count` readable floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mge_post_values_queue(
    folder: *const c_char,
    shader: *const c_char,
    names: *const *const c_char,
    count: u32,
    values: *const f32,
) -> u32 {
    install_panic_hook();
    let (Some(folder), Some(shader), Some(names)) = (unsafe { text_of(folder) }, unsafe { text_of(shader) }, unsafe {
        names_of(names, count)
    }) else {
        return 0;
    };
    if names.is_empty() || values.is_null() {
        return 0;
    }
    let values = unsafe { std::slice::from_raw_parts(values, count as usize) };
    let text = post_values::values_text(shader, &names, values);
    saver().queue(post_values::values_path(Path::new(folder), shader), text);
    1
}
