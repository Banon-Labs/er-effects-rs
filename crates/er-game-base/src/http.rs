//! Blocking HTTPS via WinHTTP: a get, and a JSON post.
//!
//! WinHTTP rather than a Rust TLS stack because it is what Wine actually implements: a
//! standalone probe built exactly like this completed the full handshake inside the game's
//! own Proton prefix (`WINEPREFIX=.../compatdata/1245620/pfx`), returning HTTP 200 and the
//! expected 6871-byte body with no winetricks, no ca bundle and no native override. That
//! measurement is the only reason this module exists in this shape.
//!
//! The functions are declared by hand rather than pulled from the `windows` crate's
//! `Win32_Networking_WinHttp` feature so the exact ABI in use is visible at the call site
//! and cannot drift with a crate upgrade.

use std::ffi::c_void;

/// An opaque WinHTTP handle.
type Handle = *mut c_void;

const ACCESS_TYPE_AUTOMATIC_PROXY: u32 = 4;
const FLAG_SECURE: u32 = 0x0080_0000;
const QUERY_STATUS_CODE: u32 = 19;
const QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
const HTTPS_PORT: u16 = 443;

/// Cap on a response body, so a hostile or broken endpoint cannot exhaust memory.
/// A build document is ~7 KB; a megabyte is three orders of magnitude of headroom.
const MAX_BODY_BYTES: usize = 1024 * 1024;

#[link(name = "winhttp")]
unsafe extern "system" {
    fn WinHttpOpen(
        agent: *const u16,
        access: u32,
        proxy: *const u16,
        bypass: *const u16,
        flags: u32,
    ) -> Handle;
    fn WinHttpConnect(session: Handle, server: *const u16, port: u16, reserved: u32) -> Handle;
    fn WinHttpOpenRequest(
        connect: Handle,
        verb: *const u16,
        object: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept: *const *const u16,
        flags: u32,
    ) -> Handle;
    fn WinHttpSendRequest(
        req: Handle,
        headers: *const u16,
        headers_len: u32,
        optional: *const c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(req: Handle, reserved: *mut c_void) -> i32;
    fn WinHttpQueryHeaders(
        req: Handle,
        info_level: u32,
        name: *const u16,
        buffer: *mut c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> i32;
    fn WinHttpReadData(req: Handle, buffer: *mut c_void, to_read: u32, read: *mut u32) -> i32;
    fn WinHttpCloseHandle(h: Handle) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

/// What went wrong, precisely enough to act on without a debugger.
#[derive(Debug)]
pub enum HttpError {
    /// A WinHTTP call failed; carries which one and the OS error.
    Win32 { step: &'static str, code: u32 },
    /// The server answered, but not with success.
    Status(u32),
    /// The body exceeded [`MAX_BODY_BYTES`].
    TooLarge,
    /// The body was not UTF-8.
    NotUtf8,
}

impl core::fmt::Display for HttpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            HttpError::Win32 { step, code } => write!(f, "{step} failed (win32 error {code})"),
            HttpError::Status(code) => write!(f, "server returned HTTP {code}"),
            HttpError::TooLarge => write!(f, "response exceeded {MAX_BODY_BYTES} bytes"),
            HttpError::NotUtf8 => write!(f, "response was not valid UTF-8"),
        }
    }
}

/// Owns a WinHTTP handle so every early return closes it.
struct Owned(Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // Safety: the handle came from WinHTTP and is closed exactly once.
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// Encode as a NUL-terminated UTF-16 string.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(core::iter::once(0)).collect()
}

/// Get `https://{host}{path}` and return the body.
///
/// Blocking. Must not be called from `DllMain` (loader lock) or from a frame callback --
/// run it on a worker thread.
///
/// # Errors
///
/// Returns [`HttpError`] identifying the step that failed.
/// [`get`] with an explicit body cap.
///
/// The default cap was chosen for a build-planner share document, which is a few kilobytes. The
/// second caller -- the build watermark's release lookup -- reads a GitHub releases page where
/// every release carries ~43 asset records, and that response is megabytes. Rather than raise
/// the default for everyone (and lose the protection the cap exists to give), the limit is the
/// caller's to state.
pub fn get_with_limit(
    host: &str,
    path: &str,
    user_agent: &str,
    max_body_bytes: usize,
) -> Result<String, HttpError> {
    get_inner(host, path, user_agent, max_body_bytes)
}

pub fn get(host: &str, path: &str, user_agent: &str) -> Result<String, HttpError> {
    get_inner(host, path, user_agent, MAX_BODY_BYTES)
}

/// Post `body` as JSON to `https://{host}{path}` and return the response body.
///
/// `headers` is the extra request-header block, CRLF-separated and without a trailing CRLF --
/// `Content-Type` is added here, so a caller supplies only what is its own (an authorization
/// header, say). Pass an empty string for none.
///
/// Blocking, with the same thread rules as [`get`]: never from `DllMain`, never from a frame.
///
/// # Errors
///
/// Returns [`HttpError`] identifying the step that failed.
pub fn post_json(
    host: &str,
    path: &str,
    user_agent: &str,
    headers: &str,
    body: &str,
) -> Result<String, HttpError> {
    request(
        host,
        path,
        user_agent,
        "POST",
        headers,
        Some(body),
        MAX_BODY_BYTES,
    )
}

fn get_inner(
    host: &str,
    path: &str,
    user_agent: &str,
    max_body_bytes: usize,
) -> Result<String, HttpError> {
    request(host, path, user_agent, "GET", "", None, max_body_bytes)
}

/// One request, whatever the verb.
///
/// The get and the post differ in three places -- the verb, the header block, and whether a body
/// is sent -- so they share a body rather than being copied apart, which is how the two would end
/// up disagreeing about which status codes count as success.
fn request(
    host: &str,
    path: &str,
    user_agent: &str,
    verb: &str,
    headers: &str,
    body: Option<&str>,
    max_body_bytes: usize,
) -> Result<String, HttpError> {
    let agent = wide(user_agent);
    let host_w = wide(host);
    let path_w = wide(path);
    let verb = wide(verb);
    // The header block WinHTTP is handed. Content-Type is ours to state: every body this sends is
    // JSON, and a server that is told nothing guesses.
    let header_block = if body.is_some() {
        let mut block = String::from("Content-Type: application/json");
        if !headers.is_empty() {
            block.push_str("\r\n");
            block.push_str(headers);
        }
        block
    } else {
        headers.to_owned()
    };
    let header_w = wide(&header_block);
    let header_len = if header_block.is_empty() {
        0
    } else {
        header_block.encode_utf16().count() as u32
    };
    let body_bytes = body.unwrap_or("").as_bytes();

    // Safety: every pointer below outlives the call that uses it, and each handle is
    // wrapped in `Owned` before any fallible step can return.
    unsafe {
        let session = Owned(WinHttpOpen(
            agent.as_ptr(),
            ACCESS_TYPE_AUTOMATIC_PROXY,
            core::ptr::null(),
            core::ptr::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(HttpError::Win32 {
                step: "WinHttpOpen",
                code: GetLastError(),
            });
        }

        let conn = Owned(WinHttpConnect(session.0, host_w.as_ptr(), HTTPS_PORT, 0));
        if conn.0.is_null() {
            return Err(HttpError::Win32 {
                step: "WinHttpConnect",
                code: GetLastError(),
            });
        }

        let req = Owned(WinHttpOpenRequest(
            conn.0,
            verb.as_ptr(),
            path_w.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            FLAG_SECURE,
        ));
        if req.0.is_null() {
            return Err(HttpError::Win32 {
                step: "WinHttpOpenRequest",
                code: GetLastError(),
            });
        }

        // The TLS handshake happens here; a Wine TLS failure surfaces at this call.
        let header_ptr = if header_len == 0 {
            core::ptr::null()
        } else {
            header_w.as_ptr()
        };
        let (body_ptr, body_len) = if body_bytes.is_empty() {
            (core::ptr::null(), 0)
        } else {
            (
                body_bytes.as_ptr().cast::<c_void>(),
                body_bytes.len() as u32,
            )
        };
        if WinHttpSendRequest(
            req.0, header_ptr, header_len, body_ptr, body_len, body_len, 0,
        ) == 0
        {
            return Err(HttpError::Win32 {
                step: "WinHttpSendRequest",
                code: GetLastError(),
            });
        }
        if WinHttpReceiveResponse(req.0, core::ptr::null_mut()) == 0 {
            return Err(HttpError::Win32 {
                step: "WinHttpReceiveResponse",
                code: GetLastError(),
            });
        }

        let mut status: u32 = 0;
        let mut status_len: u32 = 4;
        if WinHttpQueryHeaders(
            req.0,
            QUERY_STATUS_CODE | QUERY_FLAG_NUMBER,
            core::ptr::null(),
            (&raw mut status).cast::<c_void>(),
            &raw mut status_len,
            core::ptr::null_mut(),
        ) == 0
        {
            return Err(HttpError::Win32 {
                step: "WinHttpQueryHeaders",
                code: GetLastError(),
            });
        }
        // Any 2xx. `POST /inventories` answers 201, and treating that as a failure would report
        // an upload that in fact stored the build.
        if !(200..300).contains(&status) {
            return Err(HttpError::Status(status));
        }

        let mut body = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let mut read: u32 = 0;
            if WinHttpReadData(
                req.0,
                chunk.as_mut_ptr().cast::<c_void>(),
                chunk.len() as u32,
                &raw mut read,
            ) == 0
            {
                return Err(HttpError::Win32 {
                    step: "WinHttpReadData",
                    code: GetLastError(),
                });
            }
            if read == 0 {
                break;
            }
            if body.len() + read as usize > max_body_bytes {
                return Err(HttpError::TooLarge);
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }

        String::from_utf8(body).map_err(|_| HttpError::NotUtf8)
    }
}
