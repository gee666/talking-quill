//! Fixed wire error codes and strict error messages.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Busy,
    Draining,
    Incompatible,
    Rollback,
    SecurityFault,
    InvalidState,
    NativeFailure,
    Indeterminate,
    Unavailable,
}

impl ErrorCode {
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Busy => "owner busy",
            Self::Draining => "owner draining",
            Self::Incompatible => "incompatible owner",
            Self::Rollback => "rollback latched",
            Self::SecurityFault => "security fault",
            Self::InvalidState => "invalid owner state",
            Self::NativeFailure => "native operation failed",
            Self::Indeterminate => "operation indeterminate",
            Self::Unavailable => "owner unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    code: ErrorCode,
    message: &'static str,
}

impl ErrorBody {
    #[must_use]
    pub const fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message: code.message(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }
}

impl<'de> Deserialize<'de> for ErrorBody {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            code: ErrorCode,
            message: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        if wire.message != wire.code.message() {
            return Err(de::Error::custom(SchemaError::ErrorMessage));
        }
        Ok(Self::new(wire.code))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum SchemaError {
    #[error("owner-protocol JSON does not match its strict schema")]
    Json,
    #[error("owner-protocol message discriminator is unknown")]
    UnknownMessage,
    #[error("owner-protocol method is unknown")]
    UnknownMethod,
    #[error("owner-protocol value exceeds its bound")]
    Bounds,
    #[error("owner-protocol binding snapshot is invalid")]
    Binding,
    #[error("owner-protocol release-policy digest mismatch")]
    PolicyDigest,
    #[error("owner-protocol platform key mode is invalid")]
    PlatformKey,
    #[error("owner-protocol authority ceiling does not match purpose")]
    AuthorityCeiling,
    #[error("owner-protocol response union is invalid")]
    ResponseUnion,
    #[error("owner-protocol fixed error message is invalid")]
    ErrorMessage,
    #[error("owner-protocol success result is semantically invalid")]
    InvalidSuccess,
    #[error("owner-protocol event is semantically invalid")]
    InvalidEvent,
    #[error(transparent)]
    Protocol(#[from] ProtocolSelectionError),
}
