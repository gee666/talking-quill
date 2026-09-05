//! Same-image setup peer authentication and lifecycle messages.
use super::*;

pub(super) struct ControllerChannel {
    pub(super) handle: OwnedHandle,
    pub(super) action: Action,
    pub(super) silent: bool,
    pub(super) lifecycle_parent: u32,
}

pub(super) fn duplicate_delegated_process_handle(
    value: u64,
    expected_process: u32,
) -> Result<OwnedHandle> {
    let raw = value as usize as *mut c_void;
    let mut flags = 0;
    if value == 0
        || value >= usize::MAX.saturating_sub(15) as u64
        || unsafe { GetHandleInformation(raw, &mut flags) } == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Worker delegated an invalid process handle.",
        ));
    }
    let mut duplicate = ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            raw,
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
        || duplicate.is_null()
        || duplicate == INVALID_HANDLE_VALUE
    {
        return Err(fail(
            EXIT_REJECTED,
            "Worker process handle cannot be retained safely.",
        ));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    if unsafe { GetProcessId(process.as_raw_handle()) } != expected_process {
        return Err(fail(
            EXIT_REJECTED,
            "Worker delegated a handle for the wrong process.",
        ));
    }
    Ok(process)
}

impl ControllerChannel {
    pub(super) fn create(action: Action, silent: bool, lifecycle_parent: u32) -> Result<Self> {
        let pid = std::process::id();
        let name = wide(OsStr::new(&format!(r"\\.\pipe\TalkingQuill.Setup.{pid}")));
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                256,
                256,
                30_000,
                ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(fail(
                EXIT_FAILURE,
                "Cannot create the protected setup pipe.",
            ));
        }
        Ok(Self {
            handle: unsafe { OwnedHandle::from_raw_handle(handle) },
            action,
            silent,
            lifecycle_parent,
        })
    }

    pub(super) fn wait_relocated_status(&self, process: &OwnedHandle, image: &Path) -> Result<i32> {
        let operation = pipe_read::<13>(
            self.handle.as_raw_handle(),
            Some(process.as_raw_handle()),
            Instant::now() + Duration::from_secs(30),
        )?;
        if operation == *b"TQ-ARM-DELETE" {
            let deletion = arm_mapped_image_deletion(image);
            pipe_write(
                self.handle.as_raw_handle(),
                &[u8::from(deletion.is_ok())],
                Some(process.as_raw_handle()),
                Instant::now() + Duration::from_secs(30),
            )?;
            deletion?;
        } else if operation != *b"TQ-KEEP-IMAGE" {
            return Err(fail(
                EXIT_REJECTED,
                "Relocated uninstall requested an invalid completion operation.",
            ));
        }
        Ok(i32::from_le_bytes(pipe_read::<4>(
            self.handle.as_raw_handle(),
            Some(process.as_raw_handle()),
            Instant::now() + Duration::from_secs(700),
        )?))
    }

    pub(super) fn authenticate(
        &self,
        shell_process: &OwnedHandle,
        image: &Path,
        before_accept: Option<&dyn Fn() -> Result<()>>,
    ) -> Result<OwnedHandle> {
        let expected_worker = unsafe { GetProcessId(shell_process.as_raw_handle()) };
        let deadline = Instant::now() + Duration::from_secs(30);
        pipe_connect(
            self.handle.as_raw_handle(),
            shell_process.as_raw_handle(),
            deadline,
        )?;
        let delegated_value = u64::from_le_bytes(pipe_read::<8>(
            self.handle.as_raw_handle(),
            Some(shell_process.as_raw_handle()),
            deadline,
        )?);
        let worker_process = duplicate_delegated_process_handle(delegated_value, expected_worker)?;
        if unsafe { NtSuspendProcess(worker_process.as_raw_handle()) } < 0 {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot suspend the setup worker for verification.",
            ));
        }
        let mut worker = 0;
        let verification = (|| {
            if unsafe { GetNamedPipeClientProcessId(self.handle.as_raw_handle(), &mut worker) } == 0
                || worker != expected_worker
                || file_hash(&process_image(worker)?)? != file_hash(image)?
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "The setup pipe client is not the elevated same-image worker.",
                ));
            }
            verify_peer_claims(std::process::id(), worker)
        })();
        if unsafe { NtResumeProcess(worker_process.as_raw_handle()) } < 0 {
            unsafe { TerminateProcess(worker_process.as_raw_handle(), EXIT_REJECTED as u32) };
            return Err(fail(
                EXIT_REJECTED,
                "Cannot resume the verified setup worker.",
            ));
        }
        let claims_binding = verification?;
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|_| fail(EXIT_FAILURE, "Windows randomness is unavailable."))?;
        let secret = ephemeral_secret()?;
        let public = secret.public_key().to_sec1_bytes();
        let mut hello = Vec::with_capacity(97);
        hello.extend_from_slice(&nonce);
        hello.extend_from_slice(&public);
        let monitor = Some(worker_process.as_raw_handle());
        pipe_write(self.handle.as_raw_handle(), &hello, monitor, deadline)?;
        let worker_public_bytes = pipe_read::<65>(self.handle.as_raw_handle(), monitor, deadline)?;
        let worker_public = PublicKey::from_sec1_bytes(&worker_public_bytes)
            .map_err(|_| fail(EXIT_REJECTED, "Worker P-256 key is invalid."))?;
        let shared = diffie_hellman(secret.to_nonzero_scalar(), worker_public.as_affine());
        let image_hash = peer_binding(&file_hash(image)?, &claims_binding);
        let expected = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            std::process::id(),
            worker,
            &image_hash,
            &public,
            &worker_public_bytes,
            b"worker",
            &[],
        );
        let worker_proof = pipe_read::<32>(self.handle.as_raw_handle(), monitor, deadline)?;
        if worker_proof != expected {
            return Err(fail(
                EXIT_REJECTED,
                "The elevated worker transcript proof is invalid.",
            ));
        }
        let mut request = Vec::with_capacity(6);
        request.extend_from_slice(&[
            match self.action {
                Action::Install => 1,
                Action::Repair => 2,
                Action::Uninstall => 3,
                #[cfg(feature = "stale-schema2-cleanup")]
                Action::CleanStaleSchema2 => 4,
                Action::Update => {
                    return Err(fail(
                        EXIT_REJECTED,
                        "Update authority cannot come from a controller request.",
                    ));
                }
            },
            u8::from(self.silent),
        ]);
        request.extend_from_slice(&self.lifecycle_parent.to_le_bytes());
        pipe_write(
            self.handle.as_raw_handle(),
            b"TQ-SETUP-ACCEPTED",
            monitor,
            deadline,
        )?;
        pipe_write(self.handle.as_raw_handle(), &request, monitor, deadline)?;
        let controller_proof = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            std::process::id(),
            worker,
            &image_hash,
            &public,
            &worker_public_bytes,
            b"controller",
            &request,
        );
        pipe_write(
            self.handle.as_raw_handle(),
            &controller_proof,
            monitor,
            deadline,
        )?;
        if let Some(before_accept) = before_accept {
            if pipe_read::<22>(self.handle.as_raw_handle(), monitor, deadline)?
                != *b"TQ-UNINSTALL-JOURNALED"
            {
                return Err(fail(
                    EXIT_REJECTED,
                    "Elevated uninstall worker did not persist cleanup authority.",
                ));
            }
            before_accept()?;
            pipe_write(
                self.handle.as_raw_handle(),
                b"TQ-UNINSTALL-ARMED",
                monitor,
                deadline,
            )?;
        }
        let mut transcript = Sha256::new();
        transcript.update(b"TalkingQuill/setup-authenticated-transcript/v1");
        transcript.update(nonce);
        transcript.update(&public);
        transcript.update(worker_public_bytes);
        transcript.update(image_hash);
        transcript.update(&request);
        transcript.update(worker_proof);
        transcript.update(controller_proof);
        let transcript_hash: [u8; 32] = transcript.finalize().into();
        let receipt = AuthenticationReceiptInput {
            controller: std::process::id(),
            worker,
            package_sha256: &file_hash(image)?,
            peer_binding: &image_hash,
            transcript_sha256: &transcript_hash,
            nonce: &nonce,
            controller_public: &public,
            worker_public: &worker_public_bytes,
            request: &request,
            worker_proof: &worker_proof,
            controller_proof: &controller_proof,
        };
        let _ = publish_authentication_receipt(&receipt, worker_process.as_raw_handle());
        Ok(worker_process)
    }
}

pub(super) struct AuthenticationReceiptInput<'a> {
    pub(super) controller: u32,
    pub(super) worker: u32,
    pub(super) package_sha256: &'a [u8; 32],
    pub(super) peer_binding: &'a [u8; 32],
    pub(super) transcript_sha256: &'a [u8; 32],
    pub(super) nonce: &'a [u8; 32],
    pub(super) controller_public: &'a [u8],
    pub(super) worker_public: &'a [u8; 65],
    pub(super) request: &'a [u8],
    pub(super) worker_proof: &'a [u8; 32],
    pub(super) controller_proof: &'a [u8; 32],
}

pub(super) fn publish_authentication_receipt(
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

pub(super) struct WorkerChannel;
impl WorkerChannel {
    pub(super) fn connect_and_authenticate(
        image: &Path,
        expected_server: Option<&Path>,
    ) -> Result<(Action, OwnedHandle, bool, u32)> {
        let server = parent_process_id()?;
        let name = wide(OsStr::new(&format!(
            r"\\.\pipe\TalkingQuill.Setup.{server}"
        )));
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot open the medium setup controller pipe.",
            ));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let deadline = Instant::now() + Duration::from_secs(30);
        let server_process_raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_DUP_HANDLE | SYNCHRONIZE,
                0,
                server,
            )
        };
        if server_process_raw.is_null() {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot retain the setup controller process.",
            ));
        }
        let server_process = unsafe { OwnedHandle::from_raw_handle(server_process_raw) };
        let mut observed_server = 0;
        let server_image = process_image(server)?;
        if unsafe { GetNamedPipeServerProcessId(handle.as_raw_handle(), &mut observed_server) } == 0
            || observed_server != server
            || file_hash(&server_image)? != file_hash(image)?
            || expected_server.is_some_and(|expected| {
                !server_image
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
            })
        {
            return Err(fail(
                EXIT_REJECTED,
                "The setup pipe server is not the same-image controller.",
            ));
        }
        let monitor = Some(server_process.as_raw_handle());
        let mut delegated = ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                GetCurrentProcess(),
                server_process.as_raw_handle(),
                &mut delegated,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
            || delegated.is_null()
        {
            return Err(fail(
                EXIT_REJECTED,
                "Cannot delegate the worker lifecycle handle to its controller.",
            ));
        }
        pipe_write(
            handle.as_raw_handle(),
            &(delegated as usize as u64).to_le_bytes(),
            monitor,
            deadline,
        )?;
        let hello = pipe_read::<97>(handle.as_raw_handle(), monitor, deadline)?;
        let nonce: [u8; 32] = hello[..32].try_into().unwrap();
        let controller_public_bytes: [u8; 65] = hello[32..].try_into().unwrap();
        let controller_public = PublicKey::from_sec1_bytes(&controller_public_bytes)
            .map_err(|_| fail(EXIT_REJECTED, "Controller P-256 key is invalid."))?;
        let secret = ephemeral_secret()?;
        let worker_public = secret.public_key().to_sec1_bytes();
        let shared = diffie_hellman(secret.to_nonzero_scalar(), controller_public.as_affine());
        let image_hash = peer_binding(
            &file_hash(image)?,
            &verify_peer_claims(server, std::process::id())?,
        );
        pipe_write(handle.as_raw_handle(), &worker_public, monitor, deadline)?;
        let proof = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            server,
            std::process::id(),
            &image_hash,
            &controller_public_bytes,
            &worker_public,
            b"worker",
            &[],
        );
        pipe_write(handle.as_raw_handle(), &proof, monitor, deadline)?;
        if pipe_read::<17>(handle.as_raw_handle(), monitor, deadline)? != *b"TQ-SETUP-ACCEPTED" {
            return Err(fail(
                EXIT_REJECTED,
                "The medium setup controller rejected the worker.",
            ));
        }
        let request = pipe_read::<6>(handle.as_raw_handle(), monitor, deadline)?;
        let controller_proof = pipe_read::<32>(handle.as_raw_handle(), monitor, deadline)?;
        let expected = authenticated_proof(
            shared.raw_secret_bytes(),
            &nonce,
            server,
            std::process::id(),
            &image_hash,
            &controller_public_bytes,
            &worker_public,
            b"controller",
            &request,
        );
        if controller_proof != expected {
            return Err(fail(
                EXIT_REJECTED,
                "The controller transcript proof is invalid.",
            ));
        }
        let silent = match request[1] {
            0 => false,
            1 => true,
            _ => {
                return Err(fail(
                    EXIT_REJECTED,
                    "The setup controller sent an invalid UI mode.",
                ));
            }
        };
        let lifecycle_parent = u32::from_le_bytes(request[2..6].try_into().unwrap());
        match request[0] {
            1 if lifecycle_parent == 0 => Ok((Action::Install, handle, silent, 0)),
            2 if lifecycle_parent == 0 => Ok((Action::Repair, handle, silent, 0)),
            3 => Ok((Action::Uninstall, handle, silent, lifecycle_parent)),
            #[cfg(feature = "stale-schema2-cleanup")]
            4 if lifecycle_parent != 0 => {
                Ok((Action::CleanStaleSchema2, handle, true, lifecycle_parent))
            }
            _ => Err(fail(
                EXIT_REJECTED,
                "The setup controller requested an invalid operation.",
            )),
        }
    }
}

pub(super) fn peer_binding(image: &[u8; 32], claims: &[u8; 32]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(image);
    hash.update(claims);
    hash.finalize().into()
}

pub(super) fn evidence_signing_key() -> Result<SigningKey> {
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

pub(super) fn ephemeral_secret() -> Result<SecretKey> {
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
pub(super) fn authenticated_proof(
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
pub(super) fn channel_proof(
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
