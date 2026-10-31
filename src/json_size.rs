//! Count encoded JSON bytes without retaining a serialized copy. Callers choose
//! an inclusive limit so oversized values stop traversal as soon as possible.
use serde::Serialize;

pub fn serialized_size<T: Serialize + ?Sized>(value: &T, limit: usize) -> serde_json::Result<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes) {
                return Err(std::io::Error::other("JSON exceeds size limit"));
            }
            self.bytes += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::{Error, SerializeSeq};
    use serde_json::{Value, json};
    use std::cell::Cell;

    #[test]
    fn encoded_bytes_and_inclusive_limits_match_json_serialization() {
        for value in [
            Value::Null,
            json!(false),
            json!(-0.0),
            json!([]),
            json!({}),
            json!(["quotes: \" \\ \n \r \t \u{0000}", "日本語 🦀", {"b":2,"a":1}]),
        ] {
            let bytes = serde_json::to_vec(&value).unwrap();
            let size = bytes.len();
            assert_eq!(serialized_size(&value, size).unwrap(), size);
            assert_eq!(serialized_size(&value, usize::MAX).unwrap(), size);
            assert!(serialized_size(&value, size - 1).is_err());
            assert!(serialized_size(&value, 0).is_err());
        }
        // The shared counter also accepts unsized serialized values.
        assert_eq!(serialized_size("🦀\n", usize::MAX).unwrap(), "\"🦀\\n\"".len());
    }

    #[test]
    fn serializer_failures_are_returned_without_becoming_successful_counts() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(S::Error::custom("fixture serialization failed"))
            }
        }
        assert_eq!(serialized_size(&Invalid, usize::MAX).unwrap_err().to_string(), "fixture serialization failed");
    }

    #[test]
    fn oversized_sequences_stop_before_serializing_the_tail() {
        struct Sequence<'a>(&'a Cell<usize>);
        impl Serialize for Sequence<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(Some(10_000))?;
                for _ in 0..10_000 {
                    self.0.set(self.0.get() + 1);
                    sequence.serialize_element(&123)?;
                }
                sequence.end()
            }
        }
        let visited = Cell::new(0);
        assert!(serialized_size(&Sequence(&visited), 8).is_err());
        assert!(visited.get() < 10_000, "the size limit must stop traversal");
        assert_eq!(serialized_size(&[1, 2][..], 5).unwrap(), 5);
    }
}
