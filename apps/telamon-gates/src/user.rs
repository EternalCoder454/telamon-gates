//! Who is using the app: their first name, for the greeting.

use std::ffi::CStr;

/// The first word of the account's full name (the GECOS field), else the
/// login name with a capital; "" when neither can be read.
pub fn first_name() -> String {
    let (gecos, login) = account();
    let full = gecos.split(',').next().unwrap_or("").trim();
    if let Some(first) = full.split_whitespace().next() {
        return first.to_string();
    }
    let mut chars = login.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The GECOS field and the login name of the user running the app.
fn account() -> (String, String) {
    let mut buf = vec![0u8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwuid_r writes only into `pwd` and `buf` (of the length
    // given), and points `out` at `pwd` on success.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut out,
        )
    };
    if rc != 0 || out.is_null() {
        return (String::new(), std::env::var("USER").unwrap_or_default());
    }
    // SAFETY: on success both point into `buf`, NUL-terminated (or are null).
    let text = |p: *const libc::c_char| {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    (text(pwd.pw_gecos), text(pwd.pw_name))
}
