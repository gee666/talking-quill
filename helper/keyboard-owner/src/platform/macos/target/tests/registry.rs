use super::*;

#[test]
fn same_generation_tokens_are_distinct_across_process_epochs() {
    let generation = generation(7);
    let first = token_for(ProcessEpoch([0x11; 16]), generation);
    let second = token_for(ProcessEpoch([0x22; 16]), generation);
    assert_ne!(first, second);
    assert_eq!(first.as_str().len(), 56);
    assert!(first.as_str().is_ascii());
    assert!(first.as_str().len() <= NativeTargetToken::MAX_BYTES);
}

#[test]
fn target_slots_are_bounded_and_wrap_only_at_registry_capacity() {
    assert_eq!(slot(generation(1)), 1);
    assert_eq!(
        slot(generation(TARGET_REGISTRY_CAPACITY as u64 + 1)),
        slot(generation(1))
    );
}

#[test]
fn registry_consumes_exact_generation_and_token_once() {
    let mut registry = TargetRegistry::with_epoch([7; 16]);
    let context = registry.bind_context(generation(7), Some(handle(7)));
    let wrong = ActivationContext::target_unavailable(generation(8))
        .with_target_token(context.target_token().unwrap());
    assert!(registry.take(wrong).is_none());
    assert_eq!(registry.take(context), Some(handle(7)));
    assert!(registry.take(context).is_none());
}

#[test]
fn separate_registries_same_generation_use_distinct_tokens() {
    let mut first = TargetRegistry::with_epoch([1; 16]);
    let mut second = TargetRegistry::with_epoch([2; 16]);
    let first = first.bind_context(generation(1), Some(handle(1)));
    let second = second.bind_context(generation(1), Some(handle(1)));
    assert_ne!(first.target_token(), second.target_token());
}

#[test]
fn bounded_ring_evicts_old_generation_to_clipboard_only_failure() {
    let mut registry = TargetRegistry::with_epoch([3; 16]);
    let old = registry.bind_context(generation(1), Some(handle(1)));
    let mut newest = old;
    for value in 2..=TARGET_REGISTRY_CAPACITY as u64 + 1 {
        newest = registry.bind_context(generation(value), Some(handle(value)));
    }
    assert!(registry.take(old).is_none());
    assert_eq!(
        registry.take(newest),
        Some(handle(TARGET_REGISTRY_CAPACITY as u64 + 1))
    );
}
