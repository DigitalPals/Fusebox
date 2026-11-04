//! SSE decoder property: chunking must not change what a stream decodes to.

use proptest::prelude::*;

use crate::sse::{SseDecoder, SseEvent};

fn render(events: &[SseEvent]) -> Vec<(Option<String>, String)> {
    events.iter().map(|e| (e.event.clone(), e.data.clone())).collect()
}

/// One wire line (without its newline) from the shapes upstreams send.
fn wire_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "[a-z ]{0,12}".prop_map(|t| format!("data: {t}")),
        "[a-z ]{0,12}".prop_map(|t| format!("data:{t}")),
        "[a-z]{1,8}".prop_map(|t| format!("event: {t}")),
        Just(": keepalive".into()),
        Just("data: café 🦀".into()),
        Just("retry: 100".into()),
        Just("data:".into()),
    ]
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(128))]

    #[test]
    fn any_chunking_decodes_the_same(
        lines in proptest::collection::vec(wire_line(), 0..24),
        chunk in 1usize..17,
        crlf in any::<bool>(),
        trailing_newline in any::<bool>(),
    ) {
        let sep = if crlf { "\r\n" } else { "\n" };
        let mut wire = lines.join(sep);
        if trailing_newline && !wire.is_empty() {
            wire.push_str(sep);
        }

        let mut whole = SseDecoder::default();
        let mut expected = whole.push(wire.as_bytes());
        expected.extend(whole.finish());

        let mut split = SseDecoder::default();
        let mut actual = Vec::new();
        for piece in wire.as_bytes().chunks(chunk) {
            actual.extend(split.push(piece));
        }
        actual.extend(split.finish());

        prop_assert_eq!(render(&actual), render(&expected));
        prop_assert_eq!(split.failed(), whole.failed());
    }

    #[test]
    fn arbitrary_bytes_never_panic_and_chunking_agrees_on_failure(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
        chunk in 1usize..33,
    ) {
        let mut whole = SseDecoder::with_limit(256);
        let mut expected = whole.push(&bytes);
        expected.extend(whole.finish());

        let mut split = SseDecoder::with_limit(256);
        let mut actual = Vec::new();
        for piece in bytes.chunks(chunk) {
            actual.extend(split.push(piece));
        }
        actual.extend(split.finish());

        prop_assert_eq!(render(&actual), render(&expected));
        prop_assert_eq!(split.failed(), whole.failed());
    }
}
