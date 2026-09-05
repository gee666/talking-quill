//! Release-policy self binding and exact or one-hop maintenance pairing.
use super::*;

pub(super) fn validate_policy_self_fields(
    hello: &Hello,
    challenge: &Challenge,
) -> Result<
    (
        crate::release_policy::ReleasePolicy,
        crate::release_policy::ReleasePolicy,
    ),
    AuthenticationError,
> {
    let client = hello
        .client_release_policy
        .decode()
        .map_err(|_| AuthenticationError::Policy)?;
    let owner = challenge
        .owner_release_policy
        .decode()
        .map_err(|_| AuthenticationError::Policy)?;
    if client.platform != hello.platform
        || client.architecture != hello.architecture
        || client.gateway_protocol != hello.protocol
        || !client
            .release_build_digest
            .constant_time_eq(&hello.release_build_digest)
        || !client
            .gateway_sha256
            .constant_time_eq(&hello.executable_sha256)
        || !client
            .gateway_signer_policy_digest
            .constant_time_eq(&hello.signer_policy_digest)
        || owner.platform != challenge.platform
        || owner.architecture != challenge.architecture
        || owner.owner_protocol != challenge.owner_protocol
        || !owner
            .release_build_digest
            .constant_time_eq(&challenge.release_build_digest)
        || !owner
            .owner_sha256
            .constant_time_eq(&challenge.executable_sha256)
        || !owner
            .owner_signer_policy_digest
            .constant_time_eq(&challenge.signer_policy_digest)
    {
        return Err(AuthenticationError::Policy);
    }
    Ok((client, owner))
}

pub(super) fn validate_policy_pair(
    purpose: Purpose,
    client: &crate::release_policy::ReleasePolicy,
    owner: &crate::release_policy::ReleasePolicy,
) -> Result<(), AuthenticationError> {
    let exact = client == owner;
    let client_names_owner_predecessor = client.predecessor.as_ref().is_some_and(|previous| {
        previous.platform == owner.platform
            && previous.architecture == owner.architecture
            && previous.release_build_digest == owner.release_build_digest
            && previous.gateway_sha256 == owner.gateway_sha256
            && previous.owner_sha256 == owner.owner_sha256
    });
    let owner_names_client_predecessor = owner.predecessor.as_ref().is_some_and(|previous| {
        previous.platform == client.platform
            && previous.architecture == client.architecture
            && previous.release_build_digest == client.release_build_digest
            && previous.gateway_sha256 == client.gateway_sha256
            && previous.owner_sha256 == client.owner_sha256
    });
    let compatible = match purpose {
        Purpose::Observe | Purpose::Capture => exact,
        Purpose::Maintenance => {
            exact || client_names_owner_predecessor || owner_names_client_predecessor
        }
    };
    if compatible {
        Ok(())
    } else {
        Err(AuthenticationError::PolicyPair)
    }
}
