//! Exact admitted stale registry descriptors and schema-two fixture bytes.
use super::*;

pub(in super::super) const STALE_REGISTRY_HARDENED_SDDL: &str =
    "O:BAG:BAD:P(A;CI;KA;;;SY)(A;CI;KA;;;BA)";
pub(in super::super) const STALE_REGISTRY_LEGACY_SDDL: &str = "O:S-1-5-21-1333774511-1103852894-3119617217-1001G:S-1-5-21-1333774511-1103852894-3119617217-513D:AI(A;CIID;KR;;;BU)(A;CIID;KA;;;BA)(A;CIID;KA;;;SY)(A;ID;KA;;;S-1-5-21-1333774511-1103852894-3119617217-1001)(A;CIIOID;KA;;;CO)(A;CIID;KR;;;AC)(A;CIID;KR;;;S-1-15-3-1024-1065365936-1281604716-3511738428-1654721687-432734479-3232135806-4053264122-3456934681)";
pub(in super::super) const REGISTRY_DESCRIPTOR_INFORMATION: u32 =
    OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;

pub(in super::super) const SYNTHETIC_SCHEMA2_GENERATION: &str = "78bd88811b14faf1e11ba59620088aa0";
pub(in super::super) const SYNTHETIC_SCHEMA2_PENDING: &str =
    ".relaunch-pending-f3d466fe9027728be142ba84d6258074";
pub(in super::super) const SYNTHETIC_SCHEMA2_SHA256: &str =
    "abb2d6183c58b6ec52e28f6befbe43d949d2eeaf1998122921118272da8f3bad";
pub(in super::super) const SYNTHETIC_SCHEMA2_BYTES: &[u8] = br#"{"schemaVersion":2,"generation":"78bd88811b14faf1e11ba59620088aa0","userSid":"S-1-5-21-1333774511-1103852894-3119617217-1001","logonSid":"S-1-5-21-1333774511-1103852894-3119617217-1001","request":"--windows-update-bootstrap-v2=dGVzdA==","nonce":"11111111111111111111111111111111","sourceVersion":"0.0.69","targetVersion":"0.0.70","phase":"armed","completedVersion":null,"predecessor":{"version":"0.0.69","platform":"win32","architecture":"x64","sourceCommit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","sourceTree":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","releaseBuildDigest":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","roles":[]}}"#;
