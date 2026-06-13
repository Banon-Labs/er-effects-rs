use std::fmt;

const DEFAULT_MAX_HOLD_FRAMES: u16 = 30;
const MIN_ACTION_FRAMES: u16 = 1;

/// Whitelisted logical inputs that the automation layer is allowed to emit.
///
/// This crate intentionally models controller/menu intent instead of exposing
/// arbitrary key or mouse injection. Backends map these buttons to a concrete
/// injection mechanism and must release all held buttons when dropped or when a
/// sequence fails.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SafeButton {
    Confirm,
    Cancel,
    Start,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    LeftBumper,
    RightBumper,
}

impl SafeButton {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Confirm => "confirm",
            Self::Cancel => "cancel",
            Self::Start => "start",
            Self::DpadUp => "dpad_up",
            Self::DpadDown => "dpad_down",
            Self::DpadLeft => "dpad_left",
            Self::DpadRight => "dpad_right",
            Self::LeftBumper => "left_bumper",
            Self::RightBumper => "right_bumper",
        }
    }
}

/// A bounded input action. Durations are expressed in game frames so callers do
/// not depend on host sleeps, pointer focus, or wall-clock mouse polling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafeInputAction {
    Tap { button: SafeButton, frames: u16 },
    Hold { button: SafeButton, frames: u16 },
    Release { button: SafeButton },
    ReleaseAll,
}

impl SafeInputAction {
    pub fn tap(
        button: SafeButton,
        frames: u16,
        config: SafeInputConfig,
    ) -> Result<Self, SafeInputError> {
        config.validate_frames(frames)?;
        Ok(Self::Tap { button, frames })
    }

    pub fn hold(
        button: SafeButton,
        frames: u16,
        config: SafeInputConfig,
    ) -> Result<Self, SafeInputError> {
        config.validate_frames(frames)?;
        Ok(Self::Hold { button, frames })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SafeInputConfig {
    pub max_hold_frames: u16,
}

impl Default for SafeInputConfig {
    fn default() -> Self {
        Self {
            max_hold_frames: DEFAULT_MAX_HOLD_FRAMES,
        }
    }
}

impl SafeInputConfig {
    pub fn validate_frames(self, frames: u16) -> Result<(), SafeInputError> {
        if frames < MIN_ACTION_FRAMES {
            return Err(SafeInputError::ZeroFrameAction);
        }
        if frames > self.max_hold_frames {
            return Err(SafeInputError::FramesExceedLimit {
                frames,
                max: self.max_hold_frames,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum SafeInputError {
    ZeroFrameAction,
    FramesExceedLimit { frames: u16, max: u16 },
    Backend(String),
}

impl fmt::Display for SafeInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroFrameAction => write!(formatter, "input action must last at least one frame"),
            Self::FramesExceedLimit { frames, max } => write!(
                formatter,
                "input action frame count {frames} exceeds safety limit {max}"
            ),
            Self::Backend(message) => write!(formatter, "input backend failed: {message}"),
        }
    }
}

impl std::error::Error for SafeInputError {}

pub trait SafeInputBackend {
    fn apply(&mut self, action: SafeInputAction) -> Result<(), SafeInputError>;
}

/// Safe facade around an input backend. It validates bounded actions and offers
/// no mouse movement or arbitrary key injection API.
pub struct SafeInputController<B> {
    backend: B,
    config: SafeInputConfig,
}

impl<B> SafeInputController<B>
where
    B: SafeInputBackend,
{
    pub fn new(backend: B, config: SafeInputConfig) -> Self {
        Self { backend, config }
    }

    pub fn tap(&mut self, button: SafeButton, frames: u16) -> Result<(), SafeInputError> {
        self.backend
            .apply(SafeInputAction::tap(button, frames, self.config)?)
    }

    pub fn hold(&mut self, button: SafeButton, frames: u16) -> Result<(), SafeInputError> {
        self.backend
            .apply(SafeInputAction::hold(button, frames, self.config)?)
    }

    pub fn release(&mut self, button: SafeButton) -> Result<(), SafeInputError> {
        self.backend.apply(SafeInputAction::Release { button })
    }

    pub fn release_all(&mut self) -> Result<(), SafeInputError> {
        self.backend.apply(SafeInputAction::ReleaseAll)
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

/// Deterministic backend used by tests and trace-only integrations.
#[derive(Default, Debug)]
pub struct RecordingBackend {
    pub actions: Vec<SafeInputAction>,
}

impl SafeInputBackend for RecordingBackend {
    fn apply(&mut self, action: SafeInputAction) -> Result<(), SafeInputError> {
        self.actions.push(action);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MAX_HOLD_FRAMES: u16 = 3;
    const TEST_VALID_TAP_FRAMES: u16 = 2;
    const TEST_TOO_MANY_HOLD_FRAMES: u16 = 4;
    const TEST_ZERO_FRAMES: u16 = 0;

    #[test]
    fn controller_allows_only_bounded_actions() {
        let backend = RecordingBackend::default();
        let mut controller = SafeInputController::new(
            backend,
            SafeInputConfig {
                max_hold_frames: TEST_MAX_HOLD_FRAMES,
            },
        );

        controller
            .tap(SafeButton::Confirm, TEST_VALID_TAP_FRAMES)
            .unwrap();
        let error = controller
            .hold(SafeButton::DpadDown, TEST_TOO_MANY_HOLD_FRAMES)
            .unwrap_err();
        controller.release_all().unwrap();

        assert_eq!(
            error,
            SafeInputError::FramesExceedLimit {
                frames: TEST_TOO_MANY_HOLD_FRAMES,
                max: TEST_MAX_HOLD_FRAMES,
            }
        );
        assert_eq!(
            controller.into_backend().actions,
            vec![
                SafeInputAction::Tap {
                    button: SafeButton::Confirm,
                    frames: TEST_VALID_TAP_FRAMES,
                },
                SafeInputAction::ReleaseAll,
            ]
        );
    }

    #[test]
    fn zero_frame_tap_is_rejected() {
        let error = SafeInputAction::tap(
            SafeButton::Confirm,
            TEST_ZERO_FRAMES,
            SafeInputConfig::default(),
        )
        .unwrap_err();
        assert_eq!(error, SafeInputError::ZeroFrameAction);
    }
}
