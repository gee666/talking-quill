use super::*;

fn generation(value: u64) -> ActivationGeneration {
    ActivationGeneration::new(value).unwrap()
}

const fn handle(value: u64) -> TargetHandle {
    TargetHandle {
        publication_id: value,
    }
}

fn evidence(value: i32) -> TargetEvidence {
    let range = ffi::CFRange {
        location: value as isize,
        length: 0,
    };
    TargetEvidence {
        process_id: value,
        application: create_cf_string(c"app").unwrap(),
        window: create_cf_string(c"window").unwrap(),
        focused_control: create_cf_string(c"control").unwrap(),
        selected_text_range_value: create_range_value_for_test(&range).unwrap(),
        selected_text_range: range,
    }
}

fn distinct_evidence(value: i32, app: &CStr, window: &CStr, control: &CStr) -> TargetEvidence {
    let range = ffi::CFRange {
        location: value as isize,
        length: 0,
    };
    TargetEvidence {
        process_id: value,
        application: create_cf_string(app).unwrap(),
        window: create_cf_string(window).unwrap(),
        focused_control: create_cf_string(control).unwrap(),
        selected_text_range_value: create_range_value_for_test(&range).unwrap(),
        selected_text_range: range,
    }
}

mod capture;

mod registry;

mod cache;

mod observers;

mod validation_pool;

mod validation_response;

mod insertion;
