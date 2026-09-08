//! Storing a build on the planner and getting a short link back.
//!
//! # Why this exists, when the `?i=` link needs no network at all
//!
//! The self-contained form carries the whole document in the URL, and a real inventory does not
//! fit in one. Measured on a live character: 910 armaments (278 copies of one of them), 87 KB of
//! JSON, a **22,663-character** link. The planner's own share links run two to five thousand, and
//! the browser never even sends one that long. So a build past the safe length is stored instead:
//! one `POST /inventories`, and the link becomes `?b=<14 hex>`.
//!
//! # This writes to somebody else's free service, so it is rationed
//!
//! The API mints an anonymous account per caller. This module makes exactly one and keeps it in a
//! file beside the game, so a player who presses the row a hundred times is a hundred builds under
//! one account rather than a hundred accounts. The user agent identifies this mod by name. Nothing
//! polls, nothing retries in a loop, and a build small enough for `?i=` never touches the network.
//!
//! Verified against the live API before it was written (`POST /signup` -> 201, `POST /inventories`
//! with a 96 KB body -> 201, `GET /inventories/<id>` -> the same 910 armaments back). The request
//! shape is the site's own: `{id, data, gameId: 1, version}` with `Authorization: Basic <session>`
//! and `User: <user id>`, all read out of the planner bundle's `createInventory`/`makeRequest`.

use er_build_export::BuildExportDoc;

use crate::{API_HOST, USER_AGENT, log_line};

/// Where the anonymous account lives, beside the game executable with the logs.
const SESSION_FILE_NAME: &str = "er-build-planner-session.json";

/// `POST /signup` -- mints the anonymous account. Body is an empty object.
const SIGNUP_PATH: &str = "/signup";
/// `POST /inventories` -- stores a build and answers with its share id.
const INVENTORIES_PATH: &str = "/inventories";
/// The planner's own `gameId` for Elden Ring.
const GAME_ID: u32 = 1;

/// Longest `?i=` link this is willing to hand a browser.
///
/// Not a guess: a 2,863-character link opened fine, an 8,111-character one did not, and the
/// planner's own links top out around five thousand. Four thousand sits under the shortest
/// measured failure with room to spare, and everything past it is stored instead.
pub const MAX_SELF_CONTAINED_URL_CHARS: usize = 4000;

/// Why an upload could not happen.
#[derive(Debug)]
pub enum UploadError {
    /// The account could not be created.
    Signup(String),
    /// The build could not be stored.
    Store(String),
    /// The service answered with something this cannot read.
    Answer(String),
}

impl core::fmt::Display for UploadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            UploadError::Signup(why) => write!(f, "could not create a planner account: {why}"),
            UploadError::Store(why) => write!(f, "could not store the build: {why}"),
            UploadError::Answer(why) => write!(f, "the planner answered unexpectedly: {why}"),
        }
    }
}

/// An anonymous planner account: the two ids every authenticated request carries.
#[derive(Clone, Debug)]
struct Session {
    session_id: String,
    user_id: String,
}

impl Session {
    /// The authorization headers, in the shape the site sends them.
    ///
    /// `Basic <uuid>` is not base64 of `user:password` -- it is literally the word `Basic`, a
    /// space, and the session uuid, which is what the planner's `makeRequestInternal` builds.
    fn headers(&self) -> String {
        format!(
            "Authorization: Basic {}\r\nUser: {}",
            self.session_id, self.user_id
        )
    }
}

/// Where the session file lives.
fn session_path() -> Option<std::path::PathBuf> {
    Some(er_game_base::log::game_directory_path()?.join(SESSION_FILE_NAME))
}

/// The account this installation already has, if any.
fn stored_session() -> Option<Session> {
    let contents = std::fs::read_to_string(session_path()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let session_id = value.get("session")?.as_str()?.to_owned();
    let user_id = value.get("user")?.as_str()?.to_owned();
    (!session_id.is_empty() && !user_id.is_empty()).then_some(Session {
        session_id,
        user_id,
    })
}

/// Remember the account, so the next link reuses it instead of minting another.
fn store_session(session: &Session) {
    let Some(path) = session_path() else {
        return;
    };
    let contents = serde_json::json!({
        "session": session.session_id,
        "user": session.user_id,
        "note": "Anonymous er-build-planner account, made by er-quickload so shared builds have \
                 an owner. Delete this file to be given a new one.",
    });
    if let Err(err) = std::fs::write(&path, contents.to_string()) {
        log_line(&format!(
            "[build-export] could not remember the planner account in '{}': {err} -- the next \
             upload will create another one",
            path.display()
        ));
    }
}

/// The account, reused if there is one and created exactly once if there is not.
fn session() -> Result<Session, UploadError> {
    if let Some(session) = stored_session() {
        log_line("[build-export] reusing this installation's planner account");
        return Ok(session);
    }
    log_line(&format!(
        "[build-export] no planner account yet; POST https://{API_HOST}{SIGNUP_PATH}"
    ));
    let body = er_game_base::http::post_json(API_HOST, SIGNUP_PATH, USER_AGENT, "", "{}")
        .map_err(|err| UploadError::Signup(err.to_string()))?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|err| UploadError::Answer(err.to_string()))?;
    let session_id = value
        .get("session")
        .and_then(|session| session.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| UploadError::Answer("signup carried no session id".to_owned()))?;
    let user_id = value
        .get("user")
        .and_then(|user| user.get("id"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| UploadError::Answer("signup carried no user id".to_owned()))?;
    let session = Session {
        session_id: session_id.to_owned(),
        user_id: user_id.to_owned(),
    };
    store_session(&session);
    Ok(session)
}

/// Store `doc` on the planner and return its share id.
///
/// Blocking, and network-bound: worker thread only.
pub fn store(doc: &BuildExportDoc) -> Result<String, UploadError> {
    let session = session()?;
    // `id` must be a string and an empty one asks for a new build -- a null is refused outright
    // (`400 "id" must be a string`, measured).
    let request = serde_json::json!({
        "id": "",
        "data": doc,
        "gameId": GAME_ID,
        "version": doc.version,
    });
    let body = request.to_string();
    log_line(&format!(
        "[build-export] POST https://{API_HOST}{INVENTORIES_PATH} ({} bytes)",
        body.len()
    ));
    let answer = er_game_base::http::post_json(
        API_HOST,
        INVENTORIES_PATH,
        USER_AGENT,
        &session.headers(),
        &body,
    )
    .map_err(|err| UploadError::Store(err.to_string()))?;
    let value: serde_json::Value =
        serde_json::from_str(&answer).map_err(|err| UploadError::Answer(err.to_string()))?;
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| UploadError::Answer("the stored build carried no id".to_owned()))?;
    Ok(id.to_owned())
}
