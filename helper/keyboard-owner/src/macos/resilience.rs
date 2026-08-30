/// Converts one peer-local handshake result into a listener result. Rejection
/// is deliberately data, not endpoint failure: only listener/worker
/// infrastructure may return `ConnectionSourceError` to the runtime.
pub(crate) fn isolate_peer_rejection<T, E>(result: Result<T, E>) -> Option<T> {
    result.ok()
}

#[cfg(test)]
mod tests {
    use super::isolate_peer_rejection;

    #[test]
    fn adversarial_peer_failures_do_not_poison_the_next_connection() {
        let attempts = [
            isolate_peer_rejection::<u8, _>(Err("truncated")),
            isolate_peer_rejection::<u8, _>(Err("oversized")),
            isolate_peer_rejection::<u8, _>(Err("wrong-token")),
            isolate_peer_rejection::<u8, _>(Err("wrong-code")),
            isolate_peer_rejection::<u8, _>(Err("bad-proof")),
            isolate_peer_rejection::<u8, &str>(Ok(7)),
        ];
        assert_eq!(attempts, [None, None, None, None, None, Some(7)]);
    }
}
