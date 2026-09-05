//! Protocol negotiation, platform tags, and shared state enums.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Observe,
    Capture,
    Maintenance,
}

impl Purpose {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Observe => 1,
            Self::Capture => 2,
            Self::Maintenance => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorityCeiling {
    Observer,
    Capture,
    Maintenance,
}

impl AuthorityCeiling {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Observer => 1,
            Self::Capture => 2,
            Self::Maintenance => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
}

impl Platform {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::Windows => 1,
            Self::Macos => 2,
        }
    }

    pub(crate) const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Windows),
            2 => Some(Self::Macos),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Architecture {
    #[serde(rename = "x64")]
    X64,
    #[serde(rename = "arm64")]
    Arm64,
}

impl Architecture {
    pub(crate) const fn tag(self) -> u8 {
        match self {
            Self::X64 => 1,
            Self::Arm64 => 2,
        }
    }

    pub(crate) const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::X64),
            2 => Some(Self::Arm64),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerReportedState {
    Starting,
    IdleNeutral,
    LeaseDisabled,
    LeaseEnabled,
    LeaseDraining,
    OrphanCancelling,
    OrphanDraining,
    MaintenanceDraining,
    DegradedDraining,
    MaintenanceReady,
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    Starting,
    Healthy,
    RollbackLatched,
    Degraded,
    StoppingNative,
    FlushingResponse,
    Exiting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
    NotRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionMode {
    Off,
    Recording,
    CancelOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolHeader {
    pub major: u16,
    pub minor: u16,
    #[serde(rename = "compatibilityEpoch")]
    pub compatibility_epoch: u32,
    #[serde(rename = "supportedFeatureBits")]
    pub supported_feature_bits: FeatureBits,
    #[serde(rename = "requiredFeatureBits")]
    pub required_feature_bits: FeatureBits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedProtocol {
    pub major: u16,
    pub minor: u16,
    #[serde(rename = "compatibilityEpoch")]
    pub compatibility_epoch: u32,
    #[serde(rename = "featureBits")]
    pub feature_bits: FeatureBits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ProtocolSelectionError {
    #[error("invalid owner-protocol header")]
    InvalidHeader,
    #[error("owner-protocol major mismatch")]
    MajorMismatch,
    #[error("owner-protocol compatibility epoch mismatch")]
    EpochMismatch,
    #[error("owner-protocol required features are unavailable")]
    MissingRequiredFeature,
    #[error("owner-protocol has unknown required features")]
    UnknownRequiredFeature,
}

impl ProtocolHeader {
    pub fn validate(self) -> Result<(), ProtocolSelectionError> {
        let supported = self.supported_feature_bits.get();
        let required = self.required_feature_bits.get();
        if self.major != PROTOCOL_MAJOR
            || self.compatibility_epoch != COMPATIBILITY_EPOCH
            || supported & BASE_V1 == 0
            || required & BASE_V1 == 0
            || required & !supported != 0
        {
            return Err(ProtocolSelectionError::InvalidHeader);
        }
        if required & !BASE_V1 != 0 {
            return Err(ProtocolSelectionError::UnknownRequiredFeature);
        }
        Ok(())
    }

    pub fn negotiate(self, owner: Self) -> Result<SelectedProtocol, ProtocolSelectionError> {
        self.validate()?;
        owner.validate()?;
        if self.major != owner.major {
            return Err(ProtocolSelectionError::MajorMismatch);
        }
        if self.compatibility_epoch != owner.compatibility_epoch {
            return Err(ProtocolSelectionError::EpochMismatch);
        }
        let selected = self.supported_feature_bits.get() & owner.supported_feature_bits.get();
        if selected & (self.required_feature_bits.get() | owner.required_feature_bits.get())
            != (self.required_feature_bits.get() | owner.required_feature_bits.get())
        {
            return Err(ProtocolSelectionError::MissingRequiredFeature);
        }
        Ok(SelectedProtocol {
            major: self.major,
            minor: self.minor.min(owner.minor),
            compatibility_epoch: self.compatibility_epoch,
            feature_bits: FeatureBits::new(selected),
        })
    }
}
