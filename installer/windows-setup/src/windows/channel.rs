//! Same-image setup peer authentication and lifecycle messages.
use super::*;

mod worker_peer;
pub(super) use worker_peer::*;

mod evidence;
pub(super) use evidence::*;

mod proof;
pub(super) use proof::*;

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
