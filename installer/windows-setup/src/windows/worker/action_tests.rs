use super::*;

#[test]
fn manual_repair_can_finish_recovery_then_install() {
    assert!(recovered_action_is_authorized(
        Action::Repair,
        Action::Install,
        "fresh"
    ));
    assert!(recovered_action_is_authorized(
        Action::Install,
        Action::Repair,
        "fresh"
    ));
    assert!(!recovered_action_is_authorized(
        Action::Repair,
        Action::Install,
        "update"
    ));
    assert!(!recovered_action_is_authorized(
        Action::Uninstall,
        Action::Install,
        "fresh"
    ));
    assert!(!recovered_action_is_authorized(
        Action::Install,
        Action::Uninstall,
        "fresh"
    ));
}
