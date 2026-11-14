//! The engine's one error type, [`Error`], and what kind of failure it is,
//! [`ErrorKind`].

use std::fmt;

/// What went wrong, for matching on. More kinds can arrive in a minor
/// release, so a `match` needs a `_` arm.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let err = ctx.gfx.try_load_texture("no/such/file.png").unwrap_err();
///     assert_eq!(err.kind(), ErrorKind::NotFound);
/// });
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// No such file in `assets/`, or no such named thing (an animation,
    /// a component, a resource).
    NotFound,
    /// The file exists but couldn't be read.
    Io,
    /// The bytes aren't what they should be: not an image, a font, a sound
    /// or a model this engine can read.
    Decode,
    /// A shader didn't compile; the message has the compiler's log.
    Shader,
    /// The GPU or the build lacks something this needs.
    Unsupported,
    /// A handle that this game didn't make, or that was freed.
    BadHandle,
    /// JSON that doesn't parse or doesn't fit the type.
    Json,
    /// More than the engine allows (too many of something, too big).
    Limit,
    /// An argument outside what the call accepts, such as a roughness of 2
    /// or mesh data whose parts don't fit together.
    Invalid,
}

/// A failure from a `try_` call: its [`kind`](Self::kind) to match on, the
/// asset [`path`](Self::path) when there is one, and a message for people.
///
/// `Display` starts with `ponko:`, so a log line and the panic of the
/// matching non-`try_` call read the same. It implements
/// [`std::error::Error`], so `?` turns it into a `Box<dyn Error>`:
///
/// ```
/// # use ponko::prelude::*;
/// fn load_optional(ctx: &mut Ctx) -> Result<Texture, Box<dyn std::error::Error>> {
///     let texture = ctx.gfx.try_load_texture("maybe.png")?;
///     Ok(texture)
/// }
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let err = load_optional(ctx).unwrap_err();
///     assert!(err.to_string().starts_with("ponko:"));
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: ErrorKind,
    path: Option<String>,
    message: String,
}

impl Error {
    /// An error of `kind` with a message for people. Useful for plugins
    /// and helpers that want to fail like the engine does.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let err = Error::new(ErrorKind::Invalid, "speed must be positive");
    /// assert_eq!(err.to_string(), "ponko: speed must be positive");
    /// ```
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            path: None,
            message: message.into(),
        }
    }

    /// The same error, naming the asset it's about.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let err = Error::new(ErrorKind::Decode, "\"a.png\" is not a valid image").with_path("a.png");
    /// assert_eq!(err.path(), Some("a.png"));
    /// ```
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// What kind of failure this is.
    #[must_use]
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The asset path (as passed to the call) when the error is about one.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    /// The message for people, without the `ponko:` prefix.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The file `path` isn't in `assets/`; `message` is what
    /// `assets::resolve` said.
    pub(crate) fn not_found(path: &str, message: String) -> Self {
        Self::new(ErrorKind::NotFound, message).with_path(path)
    }

    /// The bytes of `path` didn't decode.
    pub(crate) fn decode(path: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Decode, message).with_path(path)
    }

    /// The same error with `context: ` in front of its message.
    #[must_use]
    pub(crate) fn context(mut self, context: impl fmt::Display) -> Self {
        self.message = format!("{context}: {}", self.message);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ponko: {}", self.message)
    }
}

impl std::error::Error for Error {}

/// For the older calls that return `Result<_, String>`: the message,
/// without the `ponko:` prefix (their callers add it).
impl From<Error> for String {
    fn from(e: Error) -> Self {
        e.message
    }
}

impl From<crate::reflect::ReflectError> for Error {
    fn from(e: crate::reflect::ReflectError) -> Self {
        use crate::reflect::ReflectError as R;
        let kind = match &e {
            R::UnknownComponent(_) | R::MissingComponent { .. } | R::UnknownResource(_) | R::MissingResource(_) => {
                ErrorKind::NotFound
            }
            R::NoSuchEntity(_) => ErrorKind::BadHandle,
            R::BadValue { .. } | R::Migration { .. } => ErrorKind::Json,
            R::NewerSave { .. } => ErrorKind::Unsupported,
        };
        Self::new(kind, e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new(ErrorKind::Json, e.to_string())
    }
}

#[cfg(feature = "navmesh")]
impl From<crate::nav::NavBakeError> for Error {
    fn from(e: crate::nav::NavBakeError) -> Self {
        Self::new(ErrorKind::Invalid, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_has_the_engine_prefix_once() {
        let e = Error::new(ErrorKind::Io, "disk on fire");
        assert_eq!(e.to_string(), "ponko: disk on fire");
        assert_eq!(String::from(e), "disk on fire");
    }

    #[test]
    fn reflect_errors_map_to_kinds() {
        let e: Error = crate::reflect::ReflectError::UnknownComponent("Nope".into()).into();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        let e: Error = crate::reflect::ReflectError::BadValue {
            component: "A".into(),
            message: "x".into(),
        }
        .into();
        assert_eq!(e.kind(), ErrorKind::Json);
    }
}
