//! Worker-side same-image controller authentication.
use super::*;

pub(in super::super) struct WorkerChannel;
impl WorkerChannel {
    pub(in super::super) fn connect_and_authenticate(
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
