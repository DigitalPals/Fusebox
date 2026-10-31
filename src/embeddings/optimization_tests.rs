//! Opt-in component measurements. The reference routines preserve the original
//! response conversion and cache-sizing work for paired comparisons.
use super::*;
use std::hint::black_box;
use std::time::Instant;

fn reference_response(vectors: Vec<Value>, tokens: u64, encoding: &str) -> Outcome {
    let mut data = Vec::with_capacity(vectors.len());
    for (i, e) in vectors.into_iter().enumerate() {
        let values = e
            .as_array()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| fail(502, "upstream returned an invalid embedding"))?;
        let mut bytes = Vec::with_capacity(values.len() * 4);
        for value in values {
            let n = value.as_f64().ok_or_else(|| fail(502, "upstream returned a nonnumeric embedding"))? as f32;
            if !n.is_finite() {
                return Err(fail(502, "upstream returned an out-of-range embedding"));
            }
            bytes.extend_from_slice(&n.to_le_bytes());
        }
        let embedding = if encoding == "base64" {
            Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
        } else {
            e
        };
        data.push(json!({ "object": "embedding", "index": i, "embedding": embedding }));
    }
    Ok(json!({ "object": "list", "data": data, "usage": { "prompt_tokens": tokens, "total_tokens": tokens } }))
}

fn current_response(vectors: Vec<Value>, tokens: u64, encoding: &str) -> Outcome {
    response(vectors, tokens, encoding).map(|(value, _)| value)
}

fn reference_size(value: &Value, limit: usize) -> Option<usize> {
    let bytes = value.to_string().len();
    (bytes <= limit).then_some(bytes)
}

fn current_size(value: &Value, limit: usize) -> Option<usize> {
    crate::json_size::serialized_size(value, limit).ok()
}

#[test]
fn float_responses_keep_original_precision_and_base64_keeps_float32_bits() {
    let vector = json!([-0.0, 0.123456789012345, 1e-50, f32::MAX as f64, f32::from_bits(1) as f64, u64::MAX]);
    let original = vector.to_string();
    let float = current_response(vec![vector.clone()], 7, "float").unwrap();
    assert_eq!(float["data"][0]["embedding"].to_string(), original);
    assert_eq!(float["usage"], json!({"prompt_tokens":7,"total_tokens":7}));

    let encoded = current_response(vec![vector.clone()], 7, "base64").unwrap();
    let bytes =
        base64::engine::general_purpose::STANDARD.decode(encoded["data"][0]["embedding"].as_str().unwrap()).unwrap();
    assert_eq!(bytes.len(), 6 * 4);
    assert_eq!(&bytes[..4], &(-0.0f32).to_le_bytes());
    assert_eq!(&bytes[8..12], &[0, 0, 0, 0], "float32 underflow still rounds to zero");
    assert_eq!(&bytes[12..16], &f32::MAX.to_le_bytes());
    assert_eq!(&bytes[16..20], &1u32.to_le_bytes(), "the smallest float32 subnormal is retained");
    for encoding in ["float", "base64"] {
        assert_eq!(
            current_response(vec![vector.clone()], 7, encoding),
            reference_response(vec![vector.clone()], 7, encoding)
        );
        assert_eq!(current_response(vec![], 0, encoding), reference_response(vec![], 0, encoding));
    }
}

#[test]
fn invalid_embedding_values_have_the_same_errors_in_both_encodings() {
    for (vector, message) in [
        (Value::Null, "upstream returned an invalid embedding"),
        (json!([]), "upstream returned an invalid embedding"),
        (json!(1), "upstream returned an invalid embedding"),
        (json!([true]), "upstream returned a nonnumeric embedding"),
        (json!([null]), "upstream returned a nonnumeric embedding"),
        (json!(["0.5"]), "upstream returned a nonnumeric embedding"),
        (json!([1e300]), "upstream returned an out-of-range embedding"),
        (json!([-1e300]), "upstream returned an out-of-range embedding"),
    ] {
        for encoding in ["float", "base64"] {
            // An invalid later vector must reject the response as before.
            let vectors = vec![json!([0.5]), vector.clone()];
            let actual = current_response(vectors.clone(), 7, encoding);
            assert_eq!(actual, reference_response(vectors, 7, encoding));
            let (status, body) = actual.unwrap_err();
            assert_eq!(status, 502);
            assert_eq!(body["error"]["message"], message);
        }
    }
}

fn response_ms(vectors: &[Value], encoding: &str, convert: fn(Vec<Value>, u64, &str) -> Outcome) -> f64 {
    // Provider JSON construction/cloning is outside the measured conversion.
    let inputs: Vec<_> = (0..4).map(|_| vectors.to_vec()).collect();
    let started = Instant::now();
    for input in inputs {
        black_box(convert(black_box(input), 512, black_box(encoding)).unwrap());
    }
    started.elapsed().as_secs_f64() * 1000. / 4.
}

fn size_ms(value: &Value, limit: usize, size: fn(&Value, usize) -> Option<usize>) -> f64 {
    let started = Instant::now();
    for _ in 0..4 {
        black_box(size(black_box(value), black_box(limit)));
    }
    started.elapsed().as_secs_f64() * 1000. / 4.
}

fn paired(mut before: impl FnMut() -> f64, mut after: impl FnMut() -> f64) -> Value {
    for _ in 0..2 {
        black_box(before());
        black_box(after());
    }
    let (mut before_ms, mut after_ms) = (Vec::new(), Vec::new());
    for run in 0..9 {
        let (a, b) = if run % 2 == 0 {
            (before(), after())
        } else {
            let b = after();
            (before(), b)
        };
        before_ms.push(a);
        after_ms.push(b);
    }
    json!({"reference_ms":before_ms,"current_ms":after_ms})
}

#[test]
#[ignore = "manual component benchmark; run with --ignored --nocapture"]
fn embedding_conversion_and_sizing_benchmark() {
    let vectors: Vec<Value> = (0..128)
        .map(|row| {
            Value::Array(
                (0..3072)
                    .map(|column| Value::from((((row * 31 + column * 17) % 2001) as f64 - 1000.) / 8191.))
                    .collect(),
            )
        })
        .collect();
    for encoding in ["float", "base64"] {
        assert_eq!(
            reference_response(vectors.clone(), 512, encoding).unwrap(),
            current_response(vectors.clone(), 512, encoding).unwrap()
        );
        let result = paired(
            || response_ms(&vectors, encoding, reference_response),
            || response_ms(&vectors, encoding, current_response),
        );
        println!("embedding_benchmark {}", json!({"case":format!("response_{encoding}"),"runs":result}));
    }
    let value = current_response(vectors, 512, "float").unwrap();
    let bytes = value.to_string().len();
    for (name, limit) in [("cache_size_fits", bytes), ("cache_size_rejected", 4096)] {
        assert_eq!(reference_size(&value, limit), current_size(&value, limit));
        let result = paired(|| size_ms(&value, limit, reference_size), || size_ms(&value, limit, current_size));
        println!("embedding_benchmark {}", json!({"case":name,"json_bytes":bytes,"limit_bytes":limit,"runs":result}));
    }
}
