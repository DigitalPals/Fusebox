use super::*;

type Models = BTreeMap<Option<String>, BTreeMap<Option<String>, Metrics>>;

fn metrics(i: usize) -> Metrics {
    let mut metrics = Metrics { observations: (i % 11 + 1) as i64, ..Default::default() };
    // Include equal costs, negative stored values, missing prices and sums that
    // cannot be represented in the dashboard's signed 64-bit JSON fields.
    metrics.sums[0] = match i % 17 {
        0 => None,
        1 => Some(i128::from(i64::MAX) + 1),
        2 => Some(i128::from(i64::MIN) - 1),
        _ => Some((i % 7) as i128 - 1),
    };
    metrics.known[0] = i64::from(metrics.sums[0].is_some());
    metrics
}

fn fixture(count: usize, model_count: usize) -> (BTreeMap<Option<String>, Group>, Models) {
    let mut groups = BTreeMap::new();
    let mut models = BTreeMap::new();
    for i in 0..count {
        let id = (i != 0).then(|| format!("client-{i:05}"));
        groups.insert(
            id.clone(),
            Group {
                metrics: metrics(i),
                accounts: BTreeSet::from([format!("account-{}", i % 3)]),
                provider: Some("openai".into()),
            },
        );
        models.insert(id, (0..model_count).map(|m| ((m != 0).then(|| format!("model-{m:03}")), metrics(m))).collect());
    }
    (groups, models)
}

// Preserve the original serialize-everything behavior as an independent oracle
// for ordering, overflow, tie breaking, nested model limits and output fields.
fn reference(groups: BTreeMap<Option<String>, Group>, mut models: Models) -> Vec<Value> {
    let mut values: Vec<Value> = groups
        .into_iter()
        .map(|(id, group)| {
            let mut value = group.metrics.value();
            let mut model_rows: Vec<_> = models.remove(&id).unwrap_or_default().into_iter().collect();
            model_rows.sort_by(|a, b| {
                b.1.sums[0].cmp(&a.1.sums[0]).then(b.1.observations.cmp(&a.1.observations)).then(a.0.cmp(&b.0))
            });
            model_rows.truncate(50);
            value["models"] = model_rows
                .into_iter()
                .map(|(id, m)| {
                    json!({"id":id,"observations":m.observations,"unpriced":m.observations-m.known[0],"unpriced_reasons":m.unpriced_reasons,
                        "estimated_cost_nanos":m.sums[0].and_then(|n|i64::try_from(n).ok())})
                })
                .collect();
            value["id"] = json!(id);
            value["accounts"] = json!(group.accounts.len());
            value["provider"] = json!(group.provider);
            value
        })
        .collect();
    values.sort_by(|a, b| {
        b["estimated_cost_nanos"]
            .as_i64()
            .cmp(&a["estimated_cost_nanos"].as_i64())
            .then(b["observations"].as_i64().cmp(&a["observations"].as_i64()))
            .then(a["id"].as_str().cmp(&b["id"].as_str()))
    });
    values.truncate(500);
    values
}

#[test]
fn bounded_breakdowns_match_prior_output_with_ties_overflow_and_missing_prices() {
    for count in [0, 7, 499, 500, 501, 637] {
        let (groups, mut models) = fixture(count, 53);
        let values = breakdown_values("client", groups, &mut models).unwrap();
        assert_eq!(values.len(), count.min(500));
        assert_eq!(models.len(), count.saturating_sub(500), "unselected clients need no model serialization");
        let (groups, models) = fixture(count, 53);
        assert_eq!(values, reference(groups, models), "{count} groups");
    }
}

#[test]
fn provider_breakdown_retains_all_fields_without_consuming_client_models() {
    let (groups, mut models) = fixture(7, 2);
    let values = breakdown_values("provider", groups, &mut models).unwrap();
    assert_eq!(models.len(), 7);
    let (groups, models) = fixture(7, 2);
    let mut expected = reference(groups, models);
    for value in &mut expected {
        value.as_object_mut().unwrap().shift_remove("models");
    }
    assert_eq!(values, expected);
}

#[test]
#[ignore = "manual CPU benchmark; run with --ignored --nocapture"]
fn usage_dashboard_selection_benchmark() {
    let count = 10_000;
    let (groups, models) = fixture(count, 1);
    let started = std::time::Instant::now();
    let expected = reference(groups, models);
    let before = started.elapsed();
    let (groups, mut models) = fixture(count, 1);
    let started = std::time::Instant::now();
    let actual = breakdown_values("client", groups, &mut models).unwrap();
    drop(models);
    let after = started.elapsed();
    assert_eq!(actual, expected);
    println!(
        "{}",
        json!({"groups":count,"serialized_groups_before":count,"serialized_groups_after":actual.len(),
            "serialize_then_sort_ms":before.as_secs_f64()*1000.0,
            "select_then_serialize_ms":after.as_secs_f64()*1000.0})
    );
}
