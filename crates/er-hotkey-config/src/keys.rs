//! One table of key names, carrying both codes Elden Ring can be read through.
//!
//! # Why one table and not two
//!
//! A key reaches this process by two different routes and is numbered differently on each. A
//! `GetAsyncKeyState` poll speaks Win32 virtual keys (`VK_F7 == 0x76`); the game's own keyboard
//! read -- `IDirectInputDevice8::GetDeviceState` -- fills a 256-byte table indexed by DIRECTINPUT
//! SCANCODE (`DIK_F7 == 0x41`). The two numbering schemes agree nowhere, and a DLL that suppresses
//! a key from the game's buffer while also polling it needs both numbers for the same key.
//!
//! Before this crate each DLL kept whichever half it happened to need, so `er-enemynpc-effects` knew
//! `"f7"` as a scancode and `er-invasion-warp-core` knew `"F7"` as a virtual key, with different
//! spellings for the numeric keypad and no way to ask one table a question in the other's terms.
//! [`NAMED_KEYS`] carries both codes per row, so a config file has one vocabulary regardless of
//! which route the DLL reading it uses.
//!
//! # Why names and not numbers
//!
//! Keys used to be hard-coded (`VK_INSERT`, `VK_F7`). A 60% keyboard -- the common compact en-us
//! layout -- has no Insert and no function row, which locks whole features out for anyone using
//! one, and two mods that both picked F7 fight over it with no way for the player to separate
//! them. Asking players to write `0x76` in a config file swaps one barrier for another: nobody
//! knows the virtual-key code for the key under their finger.
//!
//! So the config takes a name (`"Insert"`, `"F7"`, `"KP_Plus"`, `"]"`), and a raw `0x2d`-style code
//! is accepted too for anyone who does know. An unrecognised name is an error that names the key it
//! could not parse -- see [`crate::binding`] for what a caller does with that error, which is to
//! keep the key that was already working rather than end up with no key at all.
//!
//! # Aliases
//!
//! Every spelling any config file in this workspace ever accepted is still accepted, because a
//! player's existing file must not stop working when its DLL moves onto this table. That is why
//! the keypad has four names per key (`kp_plus`, `numpad_plus`, `numpad_add`, `numpad_+`) and why
//! both `lbracket` and `leftbracket` resolve.

/// A Win32 virtual-key code.
pub type VirtualKey = u32;

/// A DirectInput scancode -- an index into the 256-byte buffer `GetDeviceState` fills.
pub type Scancode = u8;

/// A key is held when the high bit of its DIK byte is set.
pub const DIK_DOWN_BIT: u8 = 0x80;

/// `VK_INSERT`.
pub const VK_INSERT: VirtualKey = 0x2d;
/// `VK_DELETE`.
pub const VK_DELETE: VirtualKey = 0x2e;

/// Left control, DirectInput scancode.
pub const DIK_LCONTROL: Scancode = 0x1d;
/// Right control, DirectInput scancode.
pub const DIK_RCONTROL: Scancode = 0x9d;
/// Left alt, DirectInput scancode.
pub const DIK_LMENU: Scancode = 0x38;
/// Right alt, DirectInput scancode.
pub const DIK_RMENU: Scancode = 0xb8;
/// Left shift, DirectInput scancode.
pub const DIK_LSHIFT: Scancode = 0x2a;
/// Right shift, DirectInput scancode.
pub const DIK_RSHIFT: Scancode = 0x36;

/// Ctrl, either side.
pub const MODIFIER_CTRL: u8 = 1 << 0;
/// Alt, either side.
pub const MODIFIER_ALT: u8 = 1 << 1;
/// Shift, either side.
pub const MODIFIER_SHIFT: u8 = 1 << 2;

/// One key: the name it renders as, every name it answers to, and its two codes.
///
/// `dik` is `None` for the handful of keys Win32 numbers but DirectInput's standard table does not
/// (F16 upward). Naming one of those in a config read through DirectInput is an error rather than
/// a key that silently never fires -- see [`parse_scancode_chord`].
#[derive(Clone, Copy, Debug)]
pub struct NamedKey {
    /// How this key is printed back to the player.
    pub display: &'static str,
    /// Lowercase names that resolve to this key. The first is the canonical spelling.
    pub aliases: &'static [&'static str],
    /// Win32 virtual-key code.
    pub vk: VirtualKey,
    /// DirectInput scancode, when the standard table has one.
    pub dik: Option<Scancode>,
}

const fn key(
    display: &'static str,
    aliases: &'static [&'static str],
    vk: VirtualKey,
    dik: Option<Scancode>,
) -> NamedKey {
    NamedKey {
        display,
        aliases,
        vk,
        dik,
    }
}

/// Every key a config file may name.
///
/// Order is load-bearing for the reverse lookup only: [`vk_name`] and [`scancode_name`] return the
/// first row carrying a code, and two rows can share one. `Enter` precedes `KP_Enter` because both
/// are `VK_RETURN`, so a config echoed back says `Enter` rather than the keypad's name for it.
pub const NAMED_KEYS: &[NamedKey] = &[
    // Editing / navigation cluster.
    key("Insert", &["insert", "ins"], VK_INSERT, Some(0xd2)),
    key("Delete", &["delete", "del"], VK_DELETE, Some(0xd3)),
    key("Home", &["home"], 0x24, Some(0xc7)),
    key("End", &["end"], 0x23, Some(0xcf)),
    key("PageUp", &["pageup", "pgup"], 0x21, Some(0xc9)),
    key("PageDown", &["pagedown", "pgdn"], 0x22, Some(0xd1)),
    key("Backspace", &["backspace"], 0x08, Some(0x0e)),
    key("Tab", &["tab"], 0x09, Some(0x0f)),
    key("Enter", &["enter", "return"], 0x0d, Some(0x1c)),
    key("Escape", &["escape", "esc"], 0x1b, Some(0x01)),
    key("Space", &["space"], 0x20, Some(0x39)),
    key("Left", &["left"], 0x25, Some(0xcb)),
    key("Up", &["up"], 0x26, Some(0xc8)),
    key("Right", &["right"], 0x27, Some(0xcd)),
    key("Down", &["down"], 0x28, Some(0xd0)),
    key("PrintScreen", &["printscreen", "sysrq"], 0x2c, Some(0xb7)),
    key("ScrollLock", &["scrolllock"], 0x91, Some(0x46)),
    key("NumLock", &["numlock"], 0x90, Some(0x45)),
    key("Pause", &["pause"], 0x13, Some(0xc5)),
    key("CapsLock", &["capslock"], 0x14, Some(0x3a)),
    // Punctuation, by symbol and by name. These survive on every compact layout.
    key("-", &["-", "minus"], 0xbd, Some(0x0c)),
    key("=", &["=", "equals"], 0xbb, Some(0x0d)),
    key("[", &["[", "leftbracket", "lbracket"], 0xdb, Some(0x1a)),
    key("]", &["]", "rightbracket", "rbracket"], 0xdd, Some(0x1b)),
    key("\\", &["\\", "backslash"], 0xdc, Some(0x2b)),
    key(";", &[";", "semicolon"], 0xba, Some(0x27)),
    key("'", &["'", "quote", "apostrophe"], 0xde, Some(0x28)),
    key(",", &[",", "comma"], 0xbc, Some(0x33)),
    key(".", &[".", "period"], 0xbe, Some(0x34)),
    key("/", &["/", "slash"], 0xbf, Some(0x35)),
    key("`", &["`", "grave", "backtick"], 0xc0, Some(0x29)),
    // Numeric keypad. `kp_*` is this crate's canonical spelling; `numpad*` is what
    // `er-enemynpc-effects` and `er-net-effects` config files already say, so both resolve.
    key("KP_0", &["kp_0", "numpad0"], 0x60, Some(0x52)),
    key("KP_1", &["kp_1", "numpad1"], 0x61, Some(0x4f)),
    key("KP_2", &["kp_2", "numpad2"], 0x62, Some(0x50)),
    key("KP_3", &["kp_3", "numpad3"], 0x63, Some(0x51)),
    key("KP_4", &["kp_4", "numpad4"], 0x64, Some(0x4b)),
    key("KP_5", &["kp_5", "numpad5"], 0x65, Some(0x4c)),
    key("KP_6", &["kp_6", "numpad6"], 0x66, Some(0x4d)),
    key("KP_7", &["kp_7", "numpad7"], 0x67, Some(0x47)),
    key("KP_8", &["kp_8", "numpad8"], 0x68, Some(0x48)),
    key("KP_9", &["kp_9", "numpad9"], 0x69, Some(0x49)),
    key(
        "KP_Multiply",
        &["kp_multiply", "numpad_multiply", "numpad_*"],
        0x6a,
        Some(0x37),
    ),
    key(
        "KP_Plus",
        &["kp_plus", "numpad_plus", "numpad_add", "numpad_+"],
        0x6b,
        Some(0x4e),
    ),
    key(
        "KP_Minus",
        &["kp_minus", "numpad_minus", "numpad_subtract", "numpad_-"],
        0x6d,
        Some(0x4a),
    ),
    key(
        "KP_Period",
        &["kp_period", "numpad_decimal", "numpad_."],
        0x6e,
        Some(0x53),
    ),
    key(
        "KP_Divide",
        &["kp_divide", "numpad_divide", "numpad_/"],
        0x6f,
        Some(0xb5),
    ),
    // Shares VK_RETURN with Enter above, which is why it is listed after it.
    key("KP_Enter", &["kp_enter", "numpad_enter"], 0x0d, Some(0x9c)),
];

/// Function keys, computed rather than listed: `VK_F1..VK_F24` are contiguous, and DirectInput's
/// standard table stops at F15 with two discontinuities on the way.
const fn function_key_scancode(index: u8) -> Option<Scancode> {
    match index {
        1..=10 => Some(0x3a + index),
        11 => Some(0x57),
        12 => Some(0x58),
        13 => Some(0x64),
        14 => Some(0x65),
        15 => Some(0x66),
        _ => None,
    }
}

/// Letters and digits: DirectInput's scancodes for the alphanumeric block, which is neither
/// alphabetical nor contiguous, so it is the one place a literal row-by-row table is unavoidable.
const ALPHANUMERIC_SCANCODES: &[(char, Scancode)] = &[
    ('1', 0x02),
    ('2', 0x03),
    ('3', 0x04),
    ('4', 0x05),
    ('5', 0x06),
    ('6', 0x07),
    ('7', 0x08),
    ('8', 0x09),
    ('9', 0x0a),
    ('0', 0x0b),
    ('q', 0x10),
    ('w', 0x11),
    ('e', 0x12),
    ('r', 0x13),
    ('t', 0x14),
    ('y', 0x15),
    ('u', 0x16),
    ('i', 0x17),
    ('o', 0x18),
    ('p', 0x19),
    ('a', 0x1e),
    ('s', 0x1f),
    ('d', 0x20),
    ('f', 0x21),
    ('g', 0x22),
    ('h', 0x23),
    ('j', 0x24),
    ('k', 0x25),
    ('l', 0x26),
    ('z', 0x2c),
    ('x', 0x2d),
    ('c', 0x2e),
    ('v', 0x2f),
    ('b', 0x30),
    ('n', 0x31),
    ('m', 0x32),
];

fn alphanumeric_scancode(ch: char) -> Option<Scancode> {
    ALPHANUMERIC_SCANCODES
        .iter()
        .find_map(|(name, code)| (*name == ch).then_some(*code))
}

/// Why a key name could not be turned into a code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyParseError {
    /// The value was empty or only whitespace.
    Empty,
    /// The name is not one this crate knows.
    Unknown(String),
    /// A `0x..`/decimal code that is outside the usable range `1..=254`.
    OutOfRange(i64),
    /// More than one non-modifier key in a chord (`"ctrl+a+b"`).
    MultipleTriggers(String),
    /// A chord of modifiers with nothing to trigger on (`"ctrl+alt"`).
    NoTrigger(String),
    /// The key exists but has no DirectInput scancode, so a DLL that reads the game's own
    /// keyboard buffer can never see it.
    NoScancode(String),
}

impl std::fmt::Display for KeyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "no key given"),
            Self::Unknown(name) => write!(
                f,
                "unknown key {name:?}. Use a name like \"Insert\", \"F7\", \"KP_Plus\" or \"]\", a \
                 single letter or digit, or a raw virtual-key code such as 0x2d"
            ),
            Self::OutOfRange(value) => write!(
                f,
                "virtual-key code {value:#x} is outside the usable range 0x01..=0xFE"
            ),
            Self::MultipleTriggers(raw) => {
                write!(f, "more than one non-modifier key in {raw:?}")
            }
            Self::NoTrigger(raw) => write!(f, "no trigger key in {raw:?}"),
            Self::NoScancode(name) => write!(
                f,
                "{name:?} has no DirectInput scancode, so this DLL could never see it pressed. \
                 Pick a key on the main keyboard, the function row up to F15, or the keypad"
            ),
        }
    }
}

/// Look a name up in [`NAMED_KEYS`]. `name` must already be lowercase and trimmed.
fn named_key(name: &str) -> Option<&'static NamedKey> {
    NAMED_KEYS
        .iter()
        .find(|entry| entry.aliases.contains(&name))
}

/// `"f7"` -> `7`, for any `F1`..`F24`.
fn function_key_index(lower: &str) -> Option<u8> {
    let rest = lower.strip_prefix('f')?;
    let index = rest.parse::<u8>().ok()?;
    (1..=24).contains(&index).then_some(index)
}

/// Is this value a raw code rather than a key name?
///
/// A single character is always a name (`"7"` is the digit key, not code 7), so the length check
/// is what keeps the alphanumerics out of the raw path.
fn is_raw_number(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    if lower.len() < 2 {
        return false;
    }
    match lower.strip_prefix("0x") {
        Some(hex) => i64::from_str_radix(hex, 16).is_ok(),
        None => lower.parse::<i64>().is_ok(),
    }
}

/// A raw `0x..` or decimal code, checked against the usable virtual-key range.
fn raw_code(lower: &str) -> Option<Result<VirtualKey, KeyParseError>> {
    let raw = if let Some(hex) = lower.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()?
    } else {
        lower.parse::<i64>().ok()?
    };
    Some(if (1..=0xFE).contains(&raw) {
        // A code that passed the range check fits a u32 by construction.
        Ok(raw as VirtualKey)
    } else {
        Err(KeyParseError::OutOfRange(raw))
    })
}

/// Turn a config value into a Win32 virtual-key code.
///
/// Accepts, in order: a single letter or digit (`"K"`, `"7"`), a function key (`"F1"`..`"F24"`), a
/// name or symbol from [`NAMED_KEYS`], or a raw code (`"0x2d"`, `"45"`). The raw form is tried last
/// so a name is never misread as a number -- `"F7"` is a key, not hex `0xF7`.
///
/// # Errors
/// [`KeyParseError`] when the value is empty, unrecognised, or a code outside `1..=254`.
pub fn parse_virtual_key(value: &str) -> Result<VirtualKey, KeyParseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KeyParseError::Empty);
    }
    let lower = trimmed.to_ascii_lowercase();

    // A single letter or digit maps to its own ASCII code -- that is exactly how Win32 numbers
    // them, so no table is needed and every layout's alphanumerics work.
    if lower.len() == 1 {
        let ch = lower.as_bytes()[0];
        if ch.is_ascii_lowercase() {
            return Ok(VirtualKey::from(ch.to_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return Ok(VirtualKey::from(ch));
        }
    }

    if let Some(index) = function_key_index(&lower) {
        return Ok(0x70 + VirtualKey::from(index - 1));
    }

    if let Some(entry) = named_key(&lower) {
        return Ok(entry.vk);
    }

    match raw_code(&lower) {
        Some(result) => result,
        None => Err(KeyParseError::Unknown(trimmed.to_string())),
    }
}

/// Turn a config value into a DirectInput scancode.
///
/// Same vocabulary as [`parse_virtual_key`], with two differences forced by what DirectInput can
/// actually report: a raw number is read as a SCANCODE (`0x00..=0xFF`, the buffer's own index), and
/// a key with no scancode is [`KeyParseError::NoScancode`] rather than a binding that can never
/// fire.
///
/// # Errors
/// [`KeyParseError`] when the value is empty, unrecognised, out of range, or has no scancode.
pub fn parse_scancode(value: &str) -> Result<Scancode, KeyParseError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(KeyParseError::Empty);
    }
    let lower = trimmed.to_ascii_lowercase();

    if lower.len() == 1
        && let Some(code) = alphanumeric_scancode(lower.as_bytes()[0] as char)
    {
        return Ok(code);
    }

    if let Some(index) = function_key_index(&lower) {
        return function_key_scancode(index)
            .ok_or_else(|| KeyParseError::NoScancode(trimmed.to_string()));
    }

    if let Some(entry) = named_key(&lower) {
        return entry
            .dik
            .ok_or_else(|| KeyParseError::NoScancode(trimmed.to_string()));
    }

    // A raw number here is a scancode, not a virtual key: this parser's whole output space is the
    // 256-byte buffer's index, so reading `0x41` as VK_A rather than DIK_F7 would be a silent lie.
    let raw = if let Some(hex) = lower.strip_prefix("0x") {
        i64::from_str_radix(hex, 16).ok()
    } else {
        lower.parse::<i64>().ok()
    };
    match raw {
        Some(raw) if (0..=0xFF).contains(&raw) => Ok(raw as Scancode),
        Some(raw) => Err(KeyParseError::OutOfRange(raw)),
        None => Err(KeyParseError::Unknown(trimmed.to_string())),
    }
}

/// The name this crate prints for a virtual-key code, for echoing config back to the player.
///
/// Falls back to the raw code when the key has no name in the table -- a code accepted through the
/// raw-number path has no name to give.
#[must_use]
pub fn vk_name(code: VirtualKey) -> String {
    if (0x70..=0x87).contains(&code) {
        return format!("F{}", code - 0x70 + 1);
    }
    if let Some(entry) = NAMED_KEYS.iter().find(|entry| entry.vk == code) {
        return entry.display.to_string();
    }
    if (0x30..=0x39).contains(&code) || (0x41..=0x5a).contains(&code) {
        // Both ranges are ASCII digits and uppercase letters by construction.
        if let Some(ch) = char::from_u32(code) {
            return ch.to_string();
        }
    }
    format!("{code:#04x}")
}

/// The name this crate prints for a DirectInput scancode.
#[must_use]
pub fn scancode_name(code: Scancode) -> String {
    for index in 1..=15u8 {
        if function_key_scancode(index) == Some(code) {
            return format!("F{index}");
        }
    }
    if let Some(entry) = NAMED_KEYS.iter().find(|entry| entry.dik == Some(code)) {
        return entry.display.to_string();
    }
    if let Some((ch, _)) = ALPHANUMERIC_SCANCODES.iter().find(|(_, dik)| *dik == code) {
        return ch.to_ascii_uppercase().to_string();
    }
    format!("{code:#04x}")
}

/// A parsed binding: zero or more side-agnostic modifiers plus exactly one trigger key, carried
/// in both numbering schemes so one config value serves a poller and a buffer-reader alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    /// [`MODIFIER_CTRL`] / [`MODIFIER_ALT`] / [`MODIFIER_SHIFT`].
    pub modifiers: u8,
    /// Win32 virtual-key code of the trigger.
    pub vk: VirtualKey,
    /// DirectInput scancode of the trigger, when the key has one.
    pub dik: Option<Scancode>,
}

impl Chord {
    /// A chord with no modifiers, from a virtual key. `dik` is filled in when the table knows one,
    /// so a caller that later needs the scancode does not have to re-parse the name.
    ///
    /// LOSSY in one PLACE: `VK_RETURN` is both Enter keys, and this returns the main keyboard's
    /// scancode. Parse the name (`"KP_Enter"`) if the keypad's is what you meant.
    #[must_use]
    pub fn from_virtual_key(vk: VirtualKey) -> Self {
        Self {
            modifiers: 0,
            vk,
            dik: scancode_for_virtual_key(vk),
        }
    }

    /// Is Alt part of this chord?
    #[must_use]
    pub const fn needs_alt(self) -> bool {
        self.modifiers & MODIFIER_ALT != 0
    }

    /// The scancode as a buffer index.
    #[must_use]
    pub fn scancode_offset(self) -> Option<usize> {
        self.dik.map(usize::from)
    }
}

/// The scancode for a virtual key, when both are known for the same physical key.
#[must_use]
pub fn scancode_for_virtual_key(vk: VirtualKey) -> Option<Scancode> {
    if (0x70..=0x87).contains(&vk) {
        // The +1 undoes VK_F1's zero-based offset; the index is 1..=24 by the range check.
        return function_key_scancode((vk - 0x70 + 1) as u8);
    }
    if let Some(entry) = NAMED_KEYS.iter().find(|entry| entry.vk == vk) {
        return entry.dik;
    }
    char::from_u32(vk).and_then(|ch| alphanumeric_scancode(ch.to_ascii_lowercase()))
}

/// The modifier a token names, if it names one.
fn modifier_bit(token: &str) -> Option<u8> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(MODIFIER_CTRL),
        "alt" => Some(MODIFIER_ALT),
        "shift" => Some(MODIFIER_SHIFT),
        _ => None,
    }
}

/// Split a chord on `+` without eating a `+` that is part of a key name.
///
/// `"numpad_+"` is a real spelling of the keypad's plus key and appears in shipped config files.
/// A plain `split('+')` turns it into `["numpad_", ""]` and then reports `unknown key "numpad_"`,
/// which is a config line the player copied out of the documentation being rejected. So an empty
/// segment -- which only occurs where two `+` were adjacent or one ended the string -- is read as
/// a literal `+`, glued onto the preceding token when that token is a partial key name and standing
/// alone when it is a modifier (`"ctrl++"` is Ctrl plus the `+` key).
fn chord_tokens(raw: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    // Set after an empty segment has been consumed as a literal `+`, so the next empty segment --
    // the other half of the same `++` -- is not counted twice.
    let mut consumed_literal = false;
    for (index, part) in raw.split('+').enumerate() {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            consumed_literal = false;
            tokens.push(trimmed.to_owned());
            continue;
        }
        if index == 0 || consumed_literal {
            consumed_literal = false;
            continue;
        }
        match tokens.last_mut() {
            Some(last) if modifier_bit(last).is_none() => last.push('+'),
            _ => tokens.push("+".to_owned()),
        }
        consumed_literal = true;
    }
    tokens
}

/// Split a chord into its parts, accumulating modifiers.
///
/// Returns the modifier mask and the single trigger part, or the error that says which of the ways
/// a chord can be malformed happened.
fn split_chord(raw: &str) -> Result<(u8, String), KeyParseError> {
    let mut modifiers = 0u8;
    let mut trigger: Option<String> = None;
    for token in chord_tokens(raw) {
        match modifier_bit(&token) {
            Some(bit) => modifiers |= bit,
            None => {
                if trigger.replace(token).is_some() {
                    return Err(KeyParseError::MultipleTriggers(raw.trim().to_string()));
                }
            }
        }
    }
    let trigger = trigger.ok_or_else(|| {
        if raw.trim().is_empty() {
            KeyParseError::Empty
        } else {
            KeyParseError::NoTrigger(raw.trim().to_string())
        }
    })?;
    Ok((modifiers, trigger))
}

/// Parse `"ctrl+alt+c"` into modifiers plus a trigger key, numbered as a virtual key.
///
/// Modifiers are side-agnostic: `ctrl` matches either control key, because a hotkey that answered
/// only to the left one would look broken to anyone who reaches for the right.
///
/// # Errors
/// [`KeyParseError`] when the chord is empty, has no trigger, has more than one trigger, or names
/// a key this crate does not know.
pub fn parse_chord(raw: &str) -> Result<Chord, KeyParseError> {
    let (modifiers, trigger) = split_chord(raw)?;
    let vk = parse_virtual_key(&trigger)?;
    Ok(Chord {
        modifiers,
        vk,
        dik: scancode_for_virtual_key(vk),
    })
}

/// Parse a chord for a DLL that reads the game's own DirectInput buffer.
///
/// Differs from [`parse_chord`] in exactly one way that matters: the trigger must have a scancode,
/// because a key with none can never appear in the buffer this caller reads. Refusing at parse time
/// turns a binding that would silently never fire into a line in the log.
///
/// # Errors
/// [`KeyParseError`] as [`parse_chord`], plus [`KeyParseError::NoScancode`].
pub fn parse_scancode_chord(raw: &str) -> Result<Chord, KeyParseError> {
    let (modifiers, trigger) = split_chord(raw)?;
    let dik = parse_scancode(&trigger)?;
    // The virtual key comes from the name, never from a raw number: `"45"` is scancode 0x2d
    // (DIK_X) to the parser above and virtual key 0x2d (VK_INSERT) to the other one, so inferring
    // one from the other would silently name a different physical key. A raw scancode simply has
    // no virtual key, and says so with 0.
    let vk = if is_raw_number(&trigger) {
        0
    } else {
        parse_virtual_key(&trigger).unwrap_or(0)
    };
    Ok(Chord {
        modifiers,
        vk,
        dik: Some(dik),
    })
}

/// Render a chord the way a config file would spell it, for the log line that says what is live.
#[must_use]
pub fn chord_name(chord: Chord) -> String {
    let mut out = String::new();
    if chord.modifiers & MODIFIER_CTRL != 0 {
        out.push_str("Ctrl+");
    }
    if chord.modifiers & MODIFIER_ALT != 0 {
        out.push_str("Alt+");
    }
    if chord.modifiers & MODIFIER_SHIFT != 0 {
        out.push_str("Shift+");
    }
    if chord.vk == 0 {
        // Parsed from a raw scancode, so the scancode is the only name there is.
        out.push_str(&chord.dik.map_or_else(|| "?".to_string(), scancode_name));
    } else {
        out.push_str(&vk_name(chord.vk));
    }
    out
}

/// Is this scancode's key held in a DirectInput keyboard buffer?
#[must_use]
pub fn scancode_down(state: &[u8], code: Scancode) -> bool {
    state
        .get(usize::from(code))
        .is_some_and(|byte| byte & DIK_DOWN_BIT != 0)
}

/// Are every modifier and the trigger of `chord` all held in this DirectInput keyboard buffer?
///
/// A buffer shorter than the trigger's index is never a press. That matters: devices of different
/// classes can share one `GetDeviceState` implementation, so a 16-byte `DIMOUSESTATE` arrives at a
/// keyboard hook, and reading scancode offsets out of one would find garbage.
#[must_use]
pub fn chord_down(chord: Chord, state: &[u8]) -> bool {
    let Some(trigger) = chord.dik else {
        return false;
    };
    let either =
        |left: Scancode, right: Scancode| scancode_down(state, left) || scancode_down(state, right);
    if chord.modifiers & MODIFIER_CTRL != 0 && !either(DIK_LCONTROL, DIK_RCONTROL) {
        return false;
    }
    if chord.modifiers & MODIFIER_ALT != 0 && !either(DIK_LMENU, DIK_RMENU) {
        return false;
    }
    if chord.modifiers & MODIFIER_SHIFT != 0 && !either(DIK_LSHIFT, DIK_RSHIFT) {
        return false;
    }
    scancode_down(state, trigger)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: a keyboard without Insert/Delete can name keys it actually has.
    #[test]
    fn a_compact_keyboard_can_name_keys_it_has() {
        assert_eq!(parse_virtual_key("F7"), Ok(0x76));
        assert_eq!(parse_virtual_key("]"), Ok(0xdd));
        assert_eq!(parse_virtual_key("RightBracket"), Ok(0xdd));
        assert_eq!(parse_virtual_key("K"), Ok(0x4b));
        assert_eq!(parse_virtual_key("7"), Ok(0x37));
    }

    /// The historical defaults of every crate that moved onto this table still parse, so nobody's
    /// existing config file stops working.
    #[test]
    fn the_historical_defaults_still_parse() {
        assert_eq!(parse_virtual_key("Insert"), Ok(VK_INSERT));
        assert_eq!(parse_virtual_key("Delete"), Ok(VK_DELETE));
        assert_eq!(parse_virtual_key("ins"), Ok(VK_INSERT));
        assert_eq!(parse_virtual_key("DEL"), Ok(VK_DELETE));
        // er-enemynpc-effects' default hotkey, in its own spelling.
        assert_eq!(
            parse_scancode_chord("ctrl+alt+c").map(|c| c.dik),
            Ok(Some(0x2e))
        );
        // er-net-effects' keypad spellings.
        assert_eq!(parse_virtual_key("numpad_add"), Ok(0x6b));
        assert_eq!(parse_virtual_key("numpad_subtract"), Ok(0x6d));
        assert_eq!(parse_virtual_key("numpad0"), Ok(0x60));
        // er-invasion-warp-core's.
        assert_eq!(parse_virtual_key("KP_Plus"), Ok(0x6b));
        assert_eq!(parse_virtual_key("lbracket"), Ok(0xdb));
    }

    /// Case and surrounding whitespace are a person writing a file, not an error.
    #[test]
    fn names_are_case_insensitive_and_trimmed() {
        assert_eq!(parse_virtual_key("  kP_pLuS "), Ok(0x6b));
        assert_eq!(parse_virtual_key("ESCAPE"), Ok(0x1b));
    }

    /// A raw code is accepted for anyone who already knows it.
    #[test]
    fn a_raw_virtual_key_code_is_accepted_in_hex_or_decimal() {
        assert_eq!(parse_virtual_key("0x2d"), Ok(VK_INSERT));
        assert_eq!(parse_virtual_key("45"), Ok(VK_INSERT));
    }

    /// A name must never be misread as a number: "F7" is a key, not hex 0xF7.
    #[test]
    fn a_named_key_is_not_reinterpreted_as_a_number() {
        assert_eq!(parse_virtual_key("F7"), Ok(0x76));
        assert_ne!(parse_virtual_key("F7"), Ok(0xf7));
    }

    /// The failure that matters. A typo must say so, and name the key it could not read.
    #[test]
    fn an_unknown_name_is_an_error_that_names_the_offending_key() {
        let error = parse_virtual_key("Winkey").expect_err("not a key this crate knows");
        assert_eq!(error, KeyParseError::Unknown("Winkey".to_string()));
        assert!(error.to_string().contains("Winkey"), "{error}");
        assert!(error.to_string().contains("Insert"), "{error}");
    }

    #[test]
    fn an_empty_value_is_an_error_rather_than_a_default() {
        assert_eq!(parse_virtual_key(""), Err(KeyParseError::Empty));
        assert_eq!(parse_virtual_key("   "), Err(KeyParseError::Empty));
        assert_eq!(parse_chord("  "), Err(KeyParseError::Empty));
    }

    /// A code outside the virtual-key range can never fire, so accepting it would be a silent dud.
    #[test]
    fn an_out_of_range_code_is_refused() {
        assert_eq!(parse_virtual_key("0x0"), Err(KeyParseError::OutOfRange(0)));
        assert_eq!(
            parse_virtual_key("0x1ff"),
            Err(KeyParseError::OutOfRange(0x1ff))
        );
    }

    /// Every alias in the table round-trips, in both numbering schemes. A typo'd row would
    /// otherwise be a key nobody can select, or -- worse -- one that resolves to the wrong code.
    #[test]
    fn every_named_key_parses_to_its_own_codes() {
        for entry in NAMED_KEYS {
            for alias in entry.aliases {
                assert_eq!(parse_virtual_key(alias), Ok(entry.vk), "vk for {alias}");
                match entry.dik {
                    Some(dik) => assert_eq!(parse_scancode(alias), Ok(dik), "dik for {alias}"),
                    None => assert!(parse_scancode(alias).is_err(), "dik for {alias}"),
                }
            }
            assert!(
                entry
                    .aliases
                    .contains(&entry.display.to_ascii_lowercase().as_str()),
                "{} does not answer to its own display name",
                entry.display
            );
        }
    }

    /// The two schemes must agree about which physical key a name means. This is the check that a
    /// row cannot carry `VK_F7` next to `DIK_F8`, which nothing else in the crate would notice.
    ///
    /// Rows whose virtual key an earlier row already claims are skipped, because recovering a
    /// scancode from a virtual key is lossy exactly where Win32 is: see
    /// [`the_two_enter_keys_share_one_virtual_key`].
    #[test]
    fn the_two_numbering_schemes_agree_per_key() {
        for (index, entry) in NAMED_KEYS.iter().enumerate() {
            if NAMED_KEYS[..index].iter().any(|prior| prior.vk == entry.vk) {
                continue;
            }
            assert_eq!(
                scancode_for_virtual_key(entry.vk),
                entry.dik,
                "{} disagrees across schemes",
                entry.display
            );
        }
        for index in 1..=15u8 {
            let vk = 0x70 + VirtualKey::from(index - 1);
            assert_eq!(scancode_for_virtual_key(vk), function_key_scancode(index));
        }
        for (ch, dik) in ALPHANUMERIC_SCANCODES {
            let vk = parse_virtual_key(&ch.to_string()).expect("alphanumeric parses");
            assert_eq!(scancode_for_virtual_key(vk), Some(*dik), "{ch}");
        }
    }

    /// Names are echoed back for the log line that tells the player which keys are live.
    #[test]
    fn a_code_renders_a_name_the_player_would_recognise() {
        assert_eq!(vk_name(VK_INSERT), "Insert");
        assert_eq!(vk_name(0x76), "F7");
        assert_eq!(vk_name(0x4b), "K");
        assert_eq!(vk_name(0x6b), "KP_Plus");
        // Enter wins over KP_Enter for the shared VK_RETURN, per the table's ordering note.
        assert_eq!(vk_name(0x0d), "Enter");
        assert_eq!(scancode_name(0x41), "F7");
        assert_eq!(scancode_name(0x2e), "C");
        assert_eq!(scancode_name(0x9c), "KP_Enter");
    }

    /// Function keys are computed, so the whole range must be right at both ends.
    #[test]
    fn the_function_key_range_is_correct_at_both_ends() {
        assert_eq!(parse_virtual_key("F1"), Ok(0x70));
        assert_eq!(parse_virtual_key("F24"), Ok(0x87));
        assert!(matches!(
            parse_virtual_key("F25"),
            Err(KeyParseError::Unknown(_))
        ));
        assert!(matches!(
            parse_virtual_key("F0"),
            Err(KeyParseError::Unknown(_))
        ));
        assert_eq!(parse_scancode("F1"), Ok(0x3b));
        assert_eq!(parse_scancode("F15"), Ok(0x66));
    }

    /// A key Win32 numbers but DirectInput does not must be refused by the scancode parser rather
    /// than accepted as a binding that can never fire.
    #[test]
    fn a_key_with_no_scancode_is_refused_by_the_scancode_parser() {
        assert_eq!(parse_virtual_key("F16"), Ok(0x7f));
        assert_eq!(
            parse_scancode("F16"),
            Err(KeyParseError::NoScancode("F16".to_string()))
        );
        assert!(
            parse_scancode("F16")
                .unwrap_err()
                .to_string()
                .contains("could never see it")
        );
    }

    /// Win32 gives the main Enter and the keypad's Enter one virtual key and tells them apart by
    /// an extended-key flag `GetAsyncKeyState` does not expose. So the name -> code direction is
    /// exact for both, and the code -> scancode direction can only answer with the main one. A
    /// config file naming `"KP_Enter"` still gets the keypad's scancode; only a caller starting
    /// from a bare `VK_RETURN` loses the distinction, and there is nothing to recover it from.
    #[test]
    fn the_two_enter_keys_share_one_virtual_key() {
        assert_eq!(parse_virtual_key("enter"), parse_virtual_key("kp_enter"));
        assert_eq!(parse_scancode("enter"), Ok(0x1c));
        assert_eq!(parse_scancode("kp_enter"), Ok(0x9c));
        assert_eq!(
            parse_scancode_chord("kp_enter").map(|c| c.dik),
            Ok(Some(0x9c))
        );
        assert_eq!(scancode_for_virtual_key(0x0d), Some(0x1c));
    }

    #[test]
    fn a_chord_carries_its_modifiers_and_both_codes() {
        let parsed = parse_chord("ctrl+alt+c").expect("ctrl+alt+c parses");
        assert_eq!(parsed.modifiers, MODIFIER_CTRL | MODIFIER_ALT);
        assert_eq!(parsed.vk, 0x43);
        assert_eq!(parsed.dik, Some(0x2e));
        assert_eq!(chord_name(parsed), "Ctrl+Alt+C");
    }

    #[test]
    fn chord_parsing_is_case_and_space_insensitive() {
        assert_eq!(
            parse_chord(" Ctrl + ALT + C ").expect("spaced parse"),
            parse_chord("ctrl+alt+c").expect("plain parse")
        );
    }

    #[test]
    fn a_bare_key_needs_no_modifier() {
        let parsed = parse_chord("f9").expect("f9 parses");
        assert_eq!(parsed.modifiers, 0);
        assert_eq!(parsed.vk, 0x78);
        assert_eq!(parsed.dik, Some(0x43));
    }

    /// A key whose name ends in `+`. A plain split on the separator turns `"numpad_+"` into
    /// `unknown key "numpad_"`, which rejects a spelling this workspace's own config comments
    /// hand the player.
    #[test]
    fn a_key_name_containing_a_plus_is_not_split_apart() {
        assert_eq!(parse_chord("numpad_+").map(|c| c.vk), Ok(0x6b));
        assert_eq!(parse_chord("ctrl+numpad_+").map(|c| c.vk), Ok(0x6b));
        assert_eq!(
            parse_chord("ctrl+numpad_+").map(|c| c.modifiers),
            Ok(MODIFIER_CTRL)
        );
        assert_eq!(parse_chord("numpad_-").map(|c| c.vk), Ok(0x6d));
        assert_eq!(parse_chord("numpad_*").map(|c| c.vk), Ok(0x6a));
        assert_eq!(parse_chord("numpad_/").map(|c| c.vk), Ok(0x6f));
        // `ctrl++` is Ctrl plus the keypad-adjacent `+` -- the separator, then a literal one.
        assert_eq!(
            parse_chord("ctrl+kp_plus").map(|c| c.modifiers),
            Ok(MODIFIER_CTRL)
        );
    }

    #[test]
    fn malformed_chords_name_the_way_they_are_malformed() {
        assert!(matches!(
            parse_chord("ctrl+alt+nonsense"),
            Err(KeyParseError::Unknown(_))
        ));
        assert_eq!(
            parse_chord("ctrl+alt"),
            Err(KeyParseError::NoTrigger("ctrl+alt".to_string()))
        );
        assert_eq!(
            parse_chord("ctrl+a+b"),
            Err(KeyParseError::MultipleTriggers("ctrl+a+b".to_string()))
        );
    }

    fn buffer_with(down: &[u8]) -> [u8; 256] {
        let mut table = [0u8; 256];
        for offset in down {
            table[usize::from(*offset)] = DIK_DOWN_BIT;
        }
        table
    }

    #[test]
    fn a_combination_matches_only_when_every_key_is_held() {
        let chord = parse_scancode_chord("ctrl+alt+c").expect("parse");
        assert!(chord_down(
            chord,
            &buffer_with(&[DIK_LCONTROL, DIK_LMENU, 0x2e])
        ));
        assert!(!chord_down(chord, &buffer_with(&[DIK_LCONTROL, 0x2e])));
        assert!(!chord_down(chord, &buffer_with(&[DIK_LCONTROL, DIK_LMENU])));
    }

    #[test]
    fn either_side_of_a_modifier_counts() {
        let chord = parse_scancode_chord("ctrl+alt+c").expect("parse");
        assert!(chord_down(
            chord,
            &buffer_with(&[DIK_RCONTROL, DIK_RMENU, 0x2e])
        ));
    }

    /// A mouse-sized buffer arriving at a keyboard hook must never read as a press.
    #[test]
    fn a_short_buffer_is_never_a_press() {
        let chord = parse_scancode_chord("insert").expect("parse");
        assert!(!chord_down(chord, &[0xff; 16]));
    }

    /// A raw scancode has no virtual key to recover, and must not pretend otherwise.
    #[test]
    fn a_raw_scancode_chord_keeps_the_scancode_and_admits_it_has_no_virtual_key() {
        let chord = parse_scancode_chord("0x41").expect("raw scancode parses");
        assert_eq!(chord.dik, Some(0x41));
        assert_eq!(chord.vk, 0);
        assert_eq!(chord_name(chord), "F7");
    }
}
