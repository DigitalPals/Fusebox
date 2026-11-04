//! When a ChatGPT subscription next charges, or when a cancelled one ends, as its
//! subscription response gives it. Claude has none: its profile only says when the
//! subscription started, and plan changes move the billing day (an account started on
//! 4 September renewed on the 23rd), while the billing endpoints refuse OAuth tokens.
//! For those the operator can set the billing day by hand (`Billing`).

use chrono::{DateTime, Local, Months, NaiveDate, TimeZone, Utc};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Charges to step through at most when moving a passed date on (100 years of months).
const MAX_STEPS: u32 = 1200;

#[derive(Debug, Clone, PartialEq)]
pub struct Renewal {
    /// The date the provider gave.
    anchor: DateTime<Utc>,
    /// Months between charges, to move a passed date on to the next charge.
    months: u32,
    /// The billing period as the provider names it ("monthly").
    period: Option<String>,
    /// Cancelled: the subscription ends at `anchor` and charges no more.
    ends: bool,
    /// Set by hand: the billing day, without a time, in the server's time zone.
    day: Option<NaiveDate>,
    /// Set by hand on another plan: a plan change can move the billing day.
    unsure: bool,
}

/// The next charge (or end) as the dashboard, the app and notifications show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Next {
    pub at: DateTime<Utc>,
    /// Not a date the provider gave for this charge, nor one set by hand for this plan.
    pub estimated: bool,
    pub ends: bool,
    /// Set by hand: a day, and `at` is only that day's morning.
    pub manual: bool,
}

/// How often a subscription set by hand charges. Of the plans without a provider date
/// only Claude Pro sells a yearly one ($200 up front); Max and Ultra are monthly only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Period {
    #[default]
    Monthly,
    Yearly,
}

impl Period {
    pub fn months(self) -> u32 {
        match self {
            Period::Monthly => 1,
            Period::Yearly => 12,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Period::Monthly => "monthly",
            Period::Yearly => "yearly",
        }
    }
}

/// A billing day the operator set for a subscription whose provider gives none, as the
/// account's credential file keeps it (`"billing": {"date": "2026-11-23", ...}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Billing {
    /// A day it charges or charged; the charges after it follow every period.
    pub date: NaiveDate,
    #[serde(default)]
    pub period: Period,
    /// Cancelled: it ends on `date` and charges no more.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ends: bool,
    /// The plan when the date was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
}

/// The morning of `day` in the server's time zone: reminders two days ahead then come
/// in the morning too.
fn morning(day: NaiveDate) -> DateTime<Utc> {
    let at = day.and_hms_opt(9, 0, 0).expect("09:00 is a time");
    Local.from_local_datetime(&at).earliest().map(|t| t.with_timezone(&Utc)).unwrap_or_else(|| at.and_utc())
}

/// The first charge on or after `today`, counting periods from `date` (never from a
/// clamped day, so the 31st stays the 31st where a month has one).
fn first_from(date: NaiveDate, months: u32, today: NaiveDate) -> Option<NaiveDate> {
    (0..=MAX_STEPS / months).filter_map(|k| date.checked_add_months(Months::new(k * months))).find(|d| *d >= today)
}

impl Billing {
    /// A cancelled subscription ends when the period paid for runs out: the next charge
    /// from `date` that would have come, which is then kept as the end.
    pub fn new(date: NaiveDate, period: Period, ends: bool, plan: Option<String>, today: NaiveDate) -> Self {
        let date = if ends { first_from(date, period.months(), today).unwrap_or(date) } else { date };
        Self { date, period, ends, plan }
    }

    /// Set on a plan the account no longer has.
    pub fn plan_changed(&self, plan: Option<&str>) -> bool {
        self.plan.as_deref().zip(plan).is_some_and(|(set, now)| !super::same_plan(set, now))
    }

    pub fn renewal(&self, plan: Option<&str>) -> Renewal {
        Renewal {
            anchor: morning(self.date),
            months: self.period.months(),
            period: Some(self.period.name().into()),
            ends: self.ends,
            day: Some(self.date),
            unsure: self.plan_changed(plan),
        }
    }
}

fn time(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?.trim()).ok().map(|t| t.with_timezone(&Utc))
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase)
}

impl Renewal {
    /// From ChatGPT's subscription response: its active entitlement's `renews_at`, or
    /// `cancels_at` (or the paid-up date) when it won't renew. Free and gratis
    /// subscriptions charge nothing.
    pub fn from_codex(value: &Value) -> Option<Self> {
        let ent = &value["entitlement"];
        if ent["has_active_subscription"] != true || ent["is_active_subscription_gratis"] == true {
            return None;
        }
        let period = text(&ent["billing_period"]).or_else(|| text(&value["billing_period"]));
        let months = match period.as_deref() {
            Some("yearly" | "annual" | "annually" | "year") => 12,
            _ => 1,
        };
        let stops = value["will_renew"] == false || value["last_active_subscription"]["will_renew"] == false;
        let cancels = time(&ent["cancels_at"])
            .or_else(|| stops.then(|| time(&ent["expires_at"]).or_else(|| time(&value["active_until"]))).flatten());
        let (anchor, ends) = match cancels {
            Some(at) => (at, true),
            None => (time(&ent["renews_at"]).or_else(|| time(&value["active_until"]))?, false),
        };
        Some(Self { anchor, months, period, ends, day: None, unsure: false })
    }

    /// The next charge after `now`, or the end of a cancelled subscription. A charge
    /// date that has passed moves on by the billing period (an estimate until the next
    /// read confirms it); an end that has passed is no date at all.
    pub fn next(&self, now: DateTime<Utc>) -> Option<Next> {
        if let Some(day) = self.day {
            return self.next_by_hand(day, now.with_timezone(&Local).date_naive());
        }
        if self.ends {
            return (self.anchor > now).then_some(Next {
                at: self.anchor,
                estimated: false,
                ends: true,
                manual: false,
            });
        }
        if self.anchor > now {
            return Some(Next { at: self.anchor, estimated: false, ends: false, manual: false });
        }
        // Always from the anchor: stepping from a clamped date (31 Jan, 28 Feb) would
        // lose the day for good (28 Mar).
        (1..=MAX_STEPS / self.months)
            .filter_map(|k| self.anchor.checked_add_months(Months::new(k * self.months)))
            .find(|at| *at > now)
            .map(|at| Next { at, estimated: true, ends: false, manual: false })
    }

    /// A day set by hand stays the next charge until that day is over, as the time of
    /// day it charges is unknown. The days after it follow every period, as set: no
    /// estimate, unless the plan changed since.
    fn next_by_hand(&self, day: NaiveDate, today: NaiveDate) -> Option<Next> {
        let next = if self.ends { (day >= today).then_some(day) } else { first_from(day, self.months, today) };
        next.map(|d| Next { at: morning(d), estimated: self.unsure, ends: self.ends, manual: true })
    }
}

/// `{"at", "estimated", "ends", "period", "manual"}` for the next charge, or null once a
/// cancelled subscription has ended. `manual` dates are a day: `at` is its morning.
impl Serialize for Renewal {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Some(next) = self.next(Utc::now()) else { return s.serialize_none() };
        let mut out = s.serialize_struct("Renewal", 5)?;
        out.serialize_field("at", &next.at)?;
        out.serialize_field("estimated", &next.estimated)?;
        out.serialize_field("ends", &next.ends)?;
        out.serialize_field("period", &self.period)?;
        out.serialize_field("manual", &next.manual)?;
        out.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_passed_date_moves_on_and_month_ends_keep_their_day() {
        let v = json!({
            "entitlement": { "has_active_subscription": true, "renews_at": "2024-01-31T15:34:42Z" },
            "billing_period": "monthly",
        });
        let r = Renewal::from_codex(&v).unwrap();
        let rolled = |now: &str| r.next(at(now)).unwrap();
        assert_eq!(
            rolled("2024-02-10T00:00:00Z"),
            Next { at: at("2024-02-29T15:34:42Z"), estimated: true, ends: false, manual: false }
        );
        assert_eq!(rolled("2024-03-01T00:00:00Z").at, at("2024-03-31T15:34:42Z"));
        assert_eq!(rolled("2026-11-02T00:00:00Z").at, at("2026-11-30T15:34:42Z"));
    }

    /// The shape production's subscription response had on 2026-11-02.
    fn codex(entitlement: Value, top: Value) -> Value {
        let mut ent = json!({
            "has_active_subscription": true,
            "is_active_subscription_gratis": false,
            "subscription_plan": "chatgptpro",
            "expires_at": "2026-11-06T15:53:56+00:00",
            "renews_at": "2026-11-06T09:53:56+00:00",
            "cancels_at": null,
            "billing_period": "monthly",
        });
        ent.as_object_mut().unwrap().extend(entitlement.as_object().unwrap().clone());
        let mut v = json!({
            "plan_type": "pro",
            "entitlement": ent,
            "last_active_subscription": { "will_renew": true },
            "active_until": "2026-11-06T09:53:56Z",
            "billing_period": "monthly",
            "will_renew": true,
        });
        v.as_object_mut().unwrap().extend(top.as_object().unwrap().clone());
        v
    }

    #[test]
    fn codex_gives_the_date() {
        let r = Renewal::from_codex(&codex(json!({}), json!({}))).unwrap();
        let now = at("2026-11-02T12:00:00Z");
        assert_eq!(
            r.next(now).unwrap(),
            Next { at: at("2026-11-06T09:53:56Z"), estimated: false, ends: false, manual: false }
        );
        assert_eq!(serde_json::to_value(&r).unwrap()["period"], "monthly");
        // Not read again since it passed: a month on, as an estimate.
        assert_eq!(
            r.next(at("2026-11-06T10:00:00Z")).unwrap(),
            Next { at: at("2026-12-06T09:53:56Z"), estimated: true, ends: false, manual: false }
        );
        let yearly = Renewal::from_codex(&codex(json!({ "billing_period": "yearly" }), json!({}))).unwrap();
        assert_eq!(yearly.next(at("2026-11-07T00:00:00Z")).unwrap().at, at("2027-11-06T09:53:56Z"));
        // Only the top-level date.
        let top = Renewal::from_codex(&codex(json!({ "renews_at": null }), json!({}))).unwrap();
        assert_eq!(top.next(now).unwrap().at, at("2026-11-06T09:53:56Z"));
    }

    #[test]
    fn a_cancelled_codex_subscription_ends() {
        let ends = |at: DateTime<Utc>| Next { at, estimated: false, ends: true, manual: false };
        let now = at("2026-11-02T12:00:00Z");
        let cancelled = Renewal::from_codex(&codex(json!({ "cancels_at": "2026-11-06T09:53:56Z" }), json!({})));
        assert_eq!(cancelled.unwrap().next(now), Some(ends(at("2026-11-06T09:53:56Z"))));
        let stops = Renewal::from_codex(&codex(json!({}), json!({ "will_renew": false }))).unwrap();
        assert_eq!(stops.next(now), Some(ends(at("2026-11-06T15:53:56Z"))));
        let stops =
            Renewal::from_codex(&codex(json!({}), json!({ "last_active_subscription": { "will_renew": false } })));
        assert!(stops.unwrap().next(now).unwrap().ends);
        // Once it has ended there is nothing to show.
        let ended = Renewal::from_codex(&codex(json!({ "cancels_at": "2026-11-01T00:00:00Z" }), json!({}))).unwrap();
        assert_eq!(ended.next(now), None);
        assert_eq!(serde_json::to_value(&ended).unwrap(), Value::Null);
    }

    #[test]
    fn codex_without_a_paid_subscription_has_no_date() {
        for ent in [
            json!({ "has_active_subscription": false }),
            json!({ "is_active_subscription_gratis": true }),
            json!({ "renews_at": "never" }),
        ] {
            let mut v = codex(ent.clone(), json!({}));
            v["active_until"] = Value::Null;
            assert_eq!(Renewal::from_codex(&v), None, "{ent}");
        }
        assert_eq!(Renewal::from_codex(&json!({ "plan_type": "free" })), None);
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn by_hand(b: &Billing, plan: Option<&str>, today: &str) -> Option<Next> {
        let r = b.renewal(plan);
        r.next_by_hand(r.day.unwrap(), day(today))
    }

    #[test]
    fn a_day_set_by_hand_charges_every_month_from_it_until_that_day_is_over() {
        let b = Billing::new(day("2026-10-31"), Period::Monthly, false, Some("max_5x".into()), day("2026-11-04"));
        assert_eq!(b.date, day("2026-10-31"), "a charge date is kept, not moved on");
        let next = |today| by_hand(&b, Some("max_5x"), today).unwrap();
        let first = next("2026-11-04");
        assert_eq!(first, Next { at: morning(day("2026-11-30")), estimated: false, ends: false, manual: true });
        assert_eq!(next("2026-11-30").at, morning(day("2026-11-30")), "still the next charge on its day");
        assert_eq!(next("2026-12-01").at, morning(day("2026-12-31")), "the 31st stays the 31st");
        assert_eq!(next("2027-02-10").at, morning(day("2027-02-28")));
        assert_eq!(next("2027-03-01").at, morning(day("2027-03-31")));
        let yearly = Billing::new(day("2026-01-15"), Period::Yearly, false, None, day("2026-11-04"));
        assert_eq!(by_hand(&yearly, Some("pro"), "2026-11-04").unwrap().at, morning(day("2027-01-15")));
        let shown = serde_json::to_value(b.renewal(Some("max_5x"))).unwrap();
        assert_eq!((shown["manual"].as_bool(), shown["period"].as_str()), (Some(true), Some("monthly")));
    }

    #[test]
    fn a_plan_change_makes_a_day_set_by_hand_an_estimate() {
        let b = Billing::new(day("2026-11-23"), Period::Monthly, false, Some("max_5x".into()), day("2026-11-04"));
        assert!(!by_hand(&b, Some("max_5x"), "2026-11-04").unwrap().estimated);
        assert!(!by_hand(&b, None, "2026-11-04").unwrap().estimated, "an unknown plan is no change");
        assert!(by_hand(&b, Some("max_20x"), "2026-11-04").unwrap().estimated, "the billing day may have moved");
        let ultra = Billing { plan: Some("max_20x".into()), ..b.clone() };
        assert!(!ultra.plan_changed(Some("ultra")), "Ultra is the max_20x tier");
        assert!(!Billing { plan: None, ..b }.plan_changed(Some("pro")), "set before the plan was known");
    }

    #[test]
    fn a_cancelled_subscription_set_by_hand_ends_when_its_period_runs_out() {
        // Charged on 23 October, cancelled: it ends on 23 November and charges no more.
        let b = Billing::new(day("2026-10-23"), Period::Monthly, true, None, day("2026-11-04"));
        assert_eq!(b.date, day("2026-11-23"));
        let ends = Next { at: morning(day("2026-11-23")), estimated: false, ends: true, manual: true };
        assert_eq!(by_hand(&b, None, "2026-11-04"), Some(ends));
        assert_eq!(by_hand(&b, None, "2026-11-23"), Some(ends));
        assert_eq!(by_hand(&b, None, "2026-11-24"), None, "ended");
        // Saved straight away from the file it is kept in.
        let saved: Billing = serde_json::from_value(serde_json::to_value(&b).unwrap()).unwrap();
        assert_eq!(saved, b);
        let old: Billing = serde_json::from_value(json!({ "date": "2026-11-23" })).unwrap();
        assert_eq!((old.period, old.ends, old.plan), (Period::Monthly, false, None));
    }
}
