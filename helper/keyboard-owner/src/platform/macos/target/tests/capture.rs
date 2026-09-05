use super::*;

#[test]
fn selected_text_range_requires_exact_ax_cf_range_type_and_scalar_extraction() {
    let valid = ffi::CFRange {
        location: 4,
        length: 2,
    };
    assert!(selected_text_range_is_valid(
        ffi::K_AX_VALUE_CFRANGE_TYPE,
        true,
        valid,
    ));
    assert!(!selected_text_range_is_valid(
        ffi::K_AX_VALUE_CGPOINT_TYPE,
        true,
        valid,
    ));
    assert!(!selected_text_range_is_valid(
        ffi::K_AX_VALUE_CFRANGE_TYPE,
        false,
        valid,
    ));
    assert!(!selected_text_range_is_valid(
        ffi::K_AX_VALUE_CFRANGE_TYPE,
        true,
        ffi::CFRange {
            location: -1,
            length: 0,
        },
    ));
    assert!(!selected_text_range_is_valid(
        ffi::K_AX_VALUE_CFRANGE_TYPE,
        true,
        ffi::CFRange {
            location: 1,
            length: -1,
        },
    ));
    assert!(selected_text_range_is_valid(
        ffi::K_AX_VALUE_CFRANGE_TYPE,
        true,
        ffi::CFRange {
            location: isize::MAX,
            length: 0,
        },
    ));
    assert!(!selected_text_range_is_valid(
        ffi::K_AX_VALUE_CFRANGE_TYPE,
        true,
        ffi::CFRange {
            location: isize::MAX,
            length: 1,
        },
    ));
    assert!(!selected_text_range_is_valid(
        ffi::K_AX_VALUE_CFRANGE_TYPE,
        true,
        ffi::CFRange {
            location: isize::MAX - 1,
            length: 2,
        },
    ));
}

#[test]
fn complete_tuple_comparison_rejects_every_identity_change() {
    let baseline = distinct_evidence(7, c"app", c"window", c"control");
    assert!(same_target(
        &baseline,
        &distinct_evidence(7, c"app", c"window", c"control")
    ));
    assert!(!same_target(
        &baseline,
        &distinct_evidence(8, c"app", c"window", c"control")
    ));
    assert!(!same_target(
        &baseline,
        &distinct_evidence(7, c"other-app", c"window", c"control")
    ));
    assert!(!same_target(
        &baseline,
        &distinct_evidence(7, c"app", c"other-window", c"control")
    ));
    assert!(!same_target(
        &baseline,
        &distinct_evidence(7, c"app", c"window", c"other-control")
    ));
    let mut moved_caret = distinct_evidence(7, c"app", c"window", c"control");
    moved_caret.selected_text_range.location += 1;
    assert!(!same_target(&baseline, &moved_caret));
}

#[test]
fn unavailable_epoch_or_evidence_is_always_targetless() {
    let mut unavailable = TargetRegistry::new();
    assert!(
        unavailable
            .bind_context(generation(1), Some(handle(1)))
            .target_token()
            .is_none()
    );
    let mut initialized = TargetRegistry::with_epoch([4; 16]);
    assert!(
        initialized
            .bind_context(generation(1), None)
            .target_token()
            .is_none()
    );
}

#[test]
fn focused_identity_attribute_names_match_accessibility_constants() {
    assert_eq!(AX_FOCUSED_APPLICATION.to_bytes(), b"AXFocusedApplication");
    assert_eq!(AX_FOCUSED_WINDOW.to_bytes(), b"AXFocusedWindow");
    assert_eq!(AX_FOCUSED_UI_ELEMENT.to_bytes(), b"AXFocusedUIElement");
    assert_eq!(AX_WINDOW.to_bytes(), b"AXWindow");
    assert_eq!(
        AX_FOCUSED_WINDOW_CHANGED.to_bytes(),
        b"AXFocusedWindowChanged"
    );
    assert_eq!(
        AX_FOCUSED_UI_ELEMENT_CHANGED.to_bytes(),
        b"AXFocusedUIElementChanged"
    );
}
