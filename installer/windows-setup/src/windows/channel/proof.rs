//! Peer bindings and setup authentication transcript proofs.
use super::*;

pub(in super::super) fn peer_binding(image: &[u8; 32], claims: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(image);
    hash.update(claims);
    hash.finalize().into()
}

pub(in super::super) fn ephemeral_secret() -> Result<SecretKey> {
    for _ in 0..16 {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
        if let Ok(secret) = SecretKey::from_slice(&bytes) {
            return Ok(secret);
        }
    }
    Err(fail(
        EXIT_FAILURE,
        "Cannot generate an ephemeral setup key.",
    ))
}

#[allow(clippy::too_many_arguments)]
pub(in super::super) fn authenticated_proof(
    shared: &[u8],
    nonce: &[u8; 32],
    controller: u32,
    worker: u32,
    image: &[u8; 32],
    controller_public: &[u8],
    worker_public: &[u8],
    role: &[u8],
    frame: &[u8],
) -> [u8; 32] {
    let mut mac =
        Hmac::<Sha256V10>::new_from_slice(shared).expect("P-256 secret is a valid HMAC key");
    mac.update(b"TalkingQuill/setup-pipe/p256/v3");
    mac.update(nonce);
    mac.update(&controller.to_le_bytes());
    mac.update(&worker.to_le_bytes());
    mac.update(image);
    mac.update(controller_public);
    mac.update(worker_public);
    mac.update(role);
    mac.update(frame);
    mac.finalize().into_bytes().into()
}

#[cfg(test)]
pub(in super::super) fn channel_proof(
    nonce: &[u8; 32],
    controller: u32,
    worker: u32,
    image: &[u8; 32],
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"TalkingQuill/setup-pipe/v2");
    hash.update(nonce);
    hash.update(controller.to_le_bytes());
    hash.update(worker.to_le_bytes());
    hash.update(image);
    hash.finalize().into()
}
