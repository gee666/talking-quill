#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

#[cfg(target_os = "macos")]
#[used]
static MACOS_BUILD_VARIANT: &str = env!("TALKING_QUILL_MACOS_BUILD_VARIANT");
#[used]
static SOURCE_COMMIT_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_COMMIT=",
    env!("TALKING_QUILL_SOURCE_COMMIT")
);
#[used]
static SOURCE_TREE_MARKER: &str = concat!(
    "TALKING_QUILL_SOURCE_TREE=",
    env!("TALKING_QUILL_SOURCE_TREE")
);

use serde::Serialize;
use talking_quill_keyboard_owner::{
    ActivationCaptureGate, OWNER_BUILD_MODE, OWNER_MODE_MARKER, local_owner_profile,
};
#[cfg(feature = "local-unsigned-owner")]
use talking_quill_keyboard_owner::{
    NoAuthenticatedConnections, ProcessSingleton, ProductionRuntime, RuntimeSignal,
    RuntimeSignalSource,
};
#[cfg(all(windows, feature = "local-unsigned-owner"))]
use talking_quill_keyboard_owner::{SingletonCoordinator, WindowsSessionSingleton};

#[cfg(feature = "local-unsigned-owner")]
#[derive(Debug)]
struct SmokeShutdown(bool);

#[cfg(feature = "local-unsigned-owner")]
impl RuntimeSignalSource for SmokeShutdown {
    fn poll_signal(&mut self) -> Option<RuntimeSignal> {
        (!std::mem::replace(&mut self.0, true)).then_some(RuntimeSignal::Shutdown)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildInfo {
    profile: &'static str,
    mode_marker: &'static str,
    capture_enabled: bool,
    runtime_rollback_active: bool,
    test_seams: bool,
}

fn main() -> ExitCode {
    std::hint::black_box(SOURCE_COMMIT_MARKER);
    std::hint::black_box(SOURCE_TREE_MARKER);
    let mut arguments = std::env::args_os();
    let _executable = arguments.next();
    let first = arguments.next();
    #[cfg(all(target_os = "macos", feature = "local-unsigned-owner"))]
    if first.as_deref() == Some(std::ffi::OsStr::new("--macos-auth-broker")) {
        return run_macos_auth_broker(arguments);
    }
    match (first.as_deref(), arguments.next()) {
        (Some(argument), None) if argument == "--build-info" => print_build_info(),
        (Some(argument), None) if argument == "--runtime-smoke" => run_runtime_smoke(),
        #[cfg(all(windows, feature = "local-unsigned-owner"))]
        (Some(argument), None) if argument == "--probe-owner-singleton-v1" => {
            probe_owner_singleton_v1()
        }
        #[cfg(not(feature = "local-unsigned-owner"))]
        (None, None) => run_owner_entrypoint(),
        #[cfg(all(windows, feature = "local-unsigned-owner"))]
        (None, None) => run_local_owner_entrypoint(),
        #[cfg(all(not(windows), feature = "local-unsigned-owner"))]
        (None, None) => run_owner_entrypoint(),
        _ => {
            eprintln!("usage: talking-quill-keyboard-owner [--build-info]");
            ExitCode::from(2)
        }
    }
}

#[cfg(all(windows, feature = "local-unsigned-owner"))]
fn probe_owner_singleton_v1() -> ExitCode {
    let Ok(mut singleton) = WindowsSessionSingleton::for_current_session() else {
        return ExitCode::from(70);
    };
    match singleton.try_acquire() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(74),
        Err(_) => ExitCode::from(70),
    }
}

#[cfg(all(target_os = "macos", feature = "local-unsigned-owner"))]
fn run_macos_auth_broker(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> ExitCode {
    use talking_quill_keyboard_owner::macos::run_native_auth_broker_child;

    let Some(identity_socket_path) = arguments.next() else {
        return ExitCode::from(5);
    };
    if arguments.next().is_some() {
        return ExitCode::from(5);
    }
    match run_native_auth_broker_child(std::path::Path::new(&identity_socket_path)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(5),
    }
}

#[cfg(all(windows, feature = "local-unsigned-owner"))]
fn run_local_owner_entrypoint() -> ExitCode {
    use talking_quill_keyboard_owner::{
        ProductionRuntime, WindowsNamedPipeConnectionSource, WindowsSessionRuntimeSignals,
        WindowsSessionSingleton,
    };

    let result = (|| {
        let source = WindowsNamedPipeConnectionSource::for_current_session()
            .map_err(|_| talking_quill_keyboard_owner::RuntimeError::ConnectionSource)?;
        let signals = WindowsSessionRuntimeSignals::install()?;
        let singleton = WindowsSessionSingleton::for_current_session()
            .map_err(talking_quill_keyboard_owner::RuntimeError::Singleton)?;
        ProductionRuntime::production(source, signals, singleton)
            .and_then(|mut runtime| runtime.run())
    })();
    finish_runtime(result)
}

#[cfg(all(target_os = "macos", feature = "local-unsigned-owner"))]
fn run_owner_entrypoint() -> ExitCode {
    use talking_quill_keyboard_owner::macos::{
        MacosAuthenticatedConnectionSource, MacosEndpointConfig, MacosFlockSingleton,
        MacosRuntimeSignals,
    };

    let result = (|| {
        let preflight_runtime =
            talking_quill_keyboard_owner::macos::MacosRuntimeDirectory::for_current_audit_session()
                .map_err(|_| talking_quill_keyboard_owner::RuntimeError::ConnectionSource)?;
        if talking_quill_keyboard_owner::macos::removal_poisoned(preflight_runtime.path()) {
            loop {
                if talking_quill_keyboard_owner::macos::finish_poisoned_without_bundle(
                    preflight_runtime.path(),
                )
                .is_ok_and(|complete| complete)
                {
                    return Ok(());
                }
                if let Ok(config) = MacosEndpointConfig::load_for_current_install()
                    && let Ok(mut bridge) =
                        talking_quill_keyboard_owner::macos::RemovalServiceBridge::start(
                            &config.bridge_policy,
                        )
                    && talking_quill_keyboard_owner::macos::finish_removed_install(
                        preflight_runtime.path(),
                        &mut bridge,
                    )
                    .is_ok()
                {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        }
        let config = MacosEndpointConfig::load_for_current_install()
            .map_err(|_| talking_quill_keyboard_owner::RuntimeError::ConnectionSource)?;
        config
            .reconcile_startup_maintenance()
            .map_err(|_| talking_quill_keyboard_owner::RuntimeError::ConnectionSource)?;
        let runtime_path = config.runtime_directory.path().to_path_buf();
        let removal_bridge_policy = config.bridge_policy.clone();
        let mut removal_bridge = talking_quill_keyboard_owner::macos::RemovalServiceBridge::start(
            &removal_bridge_policy,
        )
        .map_err(|_| talking_quill_keyboard_owner::RuntimeError::ConnectionSource)?;
        let (signals, lifecycle) = MacosRuntimeSignals::install(
            config.runtime_directory.audit_session_id(),
            config.runtime_directory.uid(),
            config.outer_bundle_path.clone(),
            runtime_path.clone(),
        )?;
        let singleton = MacosFlockSingleton::new(config.runtime_directory.clone());
        let source = MacosAuthenticatedConnectionSource::new(config);
        let result = ProductionRuntime::production(source, signals, singleton)
            .and_then(|mut runtime| runtime.run());
        if result.is_ok() && lifecycle.outer_bundle_missing() {
            // ProductionRuntime and all endpoint/singleton handles are gone.
            // Capture can never reopen in this process; retain failed-closed
            // ownership and retry authenticated poison cleanup until durable success.
            loop {
                if talking_quill_keyboard_owner::macos::finish_removed_install(
                    &runtime_path,
                    &mut removal_bridge,
                )
                .is_ok()
                {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
                if let Ok(replacement) =
                    talking_quill_keyboard_owner::macos::RemovalServiceBridge::start(
                        &removal_bridge_policy,
                    )
                {
                    removal_bridge = replacement;
                }
            }
        }
        result
    })();
    finish_runtime(result)
}

#[cfg(all(
    feature = "local-unsigned-owner",
    not(any(windows, target_os = "macos"))
))]
#[allow(dead_code)] // all-target clippy builds this unsupported fallback without its bin entrypoint
fn run_owner_entrypoint() -> ExitCode {
    use talking_quill_keyboard_owner::OsRuntimeSignals;

    let result = OsRuntimeSignals::install().and_then(|signals| {
        ProductionRuntime::production(
            NoAuthenticatedConnections,
            signals,
            ProcessSingleton::default(),
        )
        .and_then(|mut runtime| runtime.run())
    });
    finish_runtime(result)
}

#[cfg(not(feature = "local-unsigned-owner"))]
#[allow(dead_code)] // all-target clippy compiles the feature-free inert bin helper without main
fn run_owner_entrypoint() -> ExitCode {
    eprintln!("keyboard-owner runtime unavailable in feature-free inert build");
    ExitCode::from(5)
}

#[cfg(feature = "local-unsigned-owner")]
fn run_runtime_smoke() -> ExitCode {
    finish_runtime(
        ProductionRuntime::production(
            NoAuthenticatedConnections,
            SmokeShutdown(false),
            ProcessSingleton::default(),
        )
        .and_then(|mut runtime| runtime.run()),
    )
}

#[cfg(not(feature = "local-unsigned-owner"))]
fn run_runtime_smoke() -> ExitCode {
    eprintln!("keyboard-owner runtime unavailable in feature-free inert build");
    ExitCode::from(5)
}

#[cfg(feature = "local-unsigned-owner")]
fn finish_runtime(result: Result<(), talking_quill_keyboard_owner::RuntimeError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("keyboard-owner runtime unavailable: {error}");
            if matches!(
                error,
                talking_quill_keyboard_owner::RuntimeError::SingletonBusy
            ) {
                ExitCode::from(75)
            } else {
                ExitCode::from(5)
            }
        }
    }
}

fn print_build_info() -> ExitCode {
    let gate = ActivationCaptureGate::for_process();
    let local_profile = local_owner_profile();
    if OWNER_BUILD_MODE.is_local_unsigned_owner()
        && local_profile == "LOCAL_UNSIGNED_OWNER_UNSUPPORTED_TARGET"
    {
        eprintln!("local unsigned keyboard-owner profile is unavailable for this target");
        return ExitCode::from(3);
    }

    let info = BuildInfo {
        profile: if OWNER_BUILD_MODE.is_local_unsigned_owner() {
            local_profile
        } else {
            "SAFE_DISABLED_OWNER"
        },
        mode_marker: OWNER_MODE_MARKER,
        capture_enabled: gate.is_open(),
        runtime_rollback_active: gate.runtime_rollback_active(),
        test_seams: cfg!(feature = "transactional-shortcuts-dev")
            || cfg!(feature = "windows-native-test-input"),
    };
    match serde_json::to_string(&info) {
        Ok(encoded) => {
            println!("{encoded}");
            ExitCode::SUCCESS
        }
        Err(_) => ExitCode::from(4),
    }
}
