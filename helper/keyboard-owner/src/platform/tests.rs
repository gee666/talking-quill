use super::*;
#[test]
fn native_input_permission_policy_fails_closed_on_denied_or_unknown_states() {
    let granted = Permissions {
        accessibility: PermissionState::Granted,
        input_monitoring: PermissionState::Granted,
        event_post: PermissionState::Granted,
    };
    assert!(permissions_allow_native_input(granted));
    assert!(permissions_allow_native_input(Permissions {
        accessibility: PermissionState::NotApplicable,
        input_monitoring: PermissionState::NotApplicable,
        event_post: PermissionState::NotApplicable,
    }));
    for denied in [PermissionState::Denied, PermissionState::Unknown] {
        assert!(!permissions_allow_native_input(Permissions {
            accessibility: denied,
            ..granted
        }));
        assert!(!permissions_allow_native_input(Permissions {
            input_monitoring: denied,
            ..granted
        }));
        assert!(!permissions_allow_native_input(Permissions {
            event_post: denied,
            ..granted
        }));
    }
}
