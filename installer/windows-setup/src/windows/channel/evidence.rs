//! Authenticated setup receipt publication and evidence signing.
use super::*;

pub(in super::super) struct AuthenticationReceiptInput<'a> {
    pub(in super::super) controller: u32,
    pub(in super::super) worker: u32,
    pub(in super::super) package_sha256: &'a [u8; 32],
    pub(in super::super) peer_binding: &'a [u8; 32],
    pub(in super::super) transcript_sha256: &'a [u8; 32],
    pub(in super::super) nonce: &'a [u8; 32],
    pub(in super::super) controller_public: &'a [u8],
    pub(in super::super) worker_public: &'a [u8; 65],
    pub(in super::super) request: &'a [u8],
    pub(in super::super) worker_proof: &'a [u8; 32],
    pub(in super::super) controller_proof: &'a [u8; 32],
}

pub(in super::super) fn publish_authentication_receipt(
    input: &AuthenticationReceiptInput<'_>,
    worker_process: std::os::windows::io::RawHandle,
) -> Result<()> {
    let AuthenticationReceiptInput {
        controller,
        worker,
        package_sha256,
        peer_binding,
        transcript_sha256,
        nonce,
        controller_public,
        worker_public,
        request,
        worker_proof,
        controller_proof,
    } = *input;
    let name = wide(OsStr::new(&format!(
        r"\\.\pipe\TalkingQuill.Setup.Receipt.{controller}"
    )));
    let raw = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            2048,
            2048,
            1_000,
            ptr::null(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(fail(EXIT_FAILURE, "Cannot create setup evidence pipe."));
    }
    let pipe = unsafe { OwnedHandle::from_raw_handle(raw) };
    let deadline = Instant::now() + Duration::from_secs(1);
    pipe_connect(pipe.as_raw_handle(), worker_process, deadline)?;
    let challenge = pipe_read::<32>(pipe.as_raw_handle(), Some(worker_process), deadline)?;
    let signing_key = evidence_signing_key()?;
    let evidence_public = signing_key.verifying_key().to_encoded_point(false);
    let mut signed = Vec::new();
    signed.extend_from_slice(b"TalkingQuill/setup-evidence-signature/v1");
    signed.extend_from_slice(&challenge);
    signed.extend_from_slice(&controller.to_le_bytes());
    signed.extend_from_slice(&worker.to_le_bytes());
    signed.extend_from_slice(package_sha256);
    signed.extend_from_slice(peer_binding);
    signed.extend_from_slice(transcript_sha256);
    signed.extend_from_slice(nonce);
    signed.extend_from_slice(controller_public);
    signed.extend_from_slice(worker_public);
    signed.extend_from_slice(request);
    signed.extend_from_slice(worker_proof);
    signed.extend_from_slice(controller_proof);
    let signature: Signature = signing_key.sign(&signed);
    let receipt = format!(
        "{{\"schemaVersion\":2,\"protocol\":\"P-256-ECDH/HMAC-SHA256-v1\",\"controllerPid\":{controller},\"workerPid\":{worker},\"packageSha256\":\"{}\",\"peerBinding\":\"{}\",\"transcriptSha256\":\"{}\",\"observerChallenge\":\"{}\",\"nonce\":\"{}\",\"controllerPublicKey\":\"{}\",\"workerPublicKey\":\"{}\",\"request\":\"{}\",\"workerProof\":\"{}\",\"controllerProof\":\"{}\",\"evidencePublicKey\":\"{}\",\"evidenceSignature\":\"{}\"}}",
        hex_hash(package_sha256),
        hex_hash(peer_binding),
        hex_hash(transcript_sha256),
        hex_hash(&challenge),
        hex_hash(nonce),
        hex_bytes(controller_public),
        hex_bytes(worker_public),
        hex_bytes(request),
        hex_hash(worker_proof),
        hex_hash(controller_proof),
        hex_bytes(evidence_public.as_bytes()),
        hex_bytes(&signature.to_bytes()),
    );
    pipe_write(
        pipe.as_raw_handle(),
        receipt.as_bytes(),
        Some(worker_process),
        deadline,
    )
}

pub(in super::super) fn evidence_signing_key() -> Result<SigningKey> {
    for _ in 0..16 {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
        if let Ok(key) = SigningKey::from_bytes((&bytes).into()) {
            return Ok(key);
        }
    }
    Err(fail(
        EXIT_FAILURE,
        "Cannot create setup evidence signing key.",
    ))
}
