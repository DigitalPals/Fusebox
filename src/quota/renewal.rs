//! When a subscription next charges, or when a cancelled one ends. ChatGPT's subscription
//! response gives the date; Claude's profile only says when the subscription started, and
//! a monthly plan charges again on that day of each month (Stripe keeps the day, and takes
//! the last day of shorter months).

use chrono::{DateTime, Months, Utc};
use serde::Serialize;
use serde::ser::{SerializeStruct, Serializer};
use serde_json::Value;

/// Charges to step through at most when moving a passed date on (100 years of months).
const MAX_STEPS: u32 = 1200;

#[derive(Debug, Clone, PartialEq)]
pub struct Renewal {
    /// The date the provider gave, or the day the subscription started (`estimated`).
    anchor: DateTime<Utc>,
    /// Months between charges, to move a passed date on to the next charge.
    months: u32,
    /// The billing period as the provider names it ("monthly"); Claude doesn't say.
    period: Option<String>,
    /// Worked out from the subscription's start: the provider gives no date.
    estimated: bool,
    /// Cancelled: the subscription ends at `anchor` and charges no more.
    ends: bool,
}

/// The next charge (or end) as the dashboard, the app and notifications show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Next {
    pub at: DateTime<Utc>,
    /// Not a date the provider gave for this charge.
    pub estimated: bool,
    pub ends: bool,
}

fn time(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?.trim()).ok().map(|t| t.with_timezone(&Utc))
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase)
}

impl Renewal {
    /// From a Claude profile: monthly from `subscription_created_at`, while the
    /// subscription is active. Claude doesn't say whether a plan is billed yearly, nor
    /// whether a cancelled one still counts as active until its period ends.
    pub fn from_claude(profile: &Value) -> Option<Self> {
        let org = &profile["organization"];
        if text(&org["subscription_status"]).as_deref() != Some("active") {
            return None;
        }
        // Invoiced organizations (Team, Enterprise) are not charged by the month.
        if text(&org["billing_type"]).is_some_and(|t| !t.contains("subscription")) {
            return None;
        }
        Some(Self {
            anchor: time(&org["subscription_created_at"])?,
            months: 1,
            period: None,
            estimated: true,
            ends: false,
        })
    }

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
        Some(Self { anchor, months, period, estimated: false, ends })
    }

    /// The next charge after `now`, or the end of a cancelled subscription. A charge
    /// date that has passed moves on by the billing period (an estimate until the next
    /// read confirms it); an end that has passed is no date at all.
    pub fn next(&self, now: DateTime<Utc>) -> Option<Next> {
        if self.ends {
            return (self.anchor > now).then_some(Next { at: self.anchor, estimated: self.estimated, ends: true });
        }
        if self.anchor > now && !self.estimated {
            return Some(Next { at: self.anchor, estimated: false, ends: false });
        }
        // Always from the anchor: stepping from a clamped date (31 Jan, 28 Feb) would
        // lose the day for good (28 Mar).
        (1..=MAX_STEPS / self.months)
            .filter_map(|k| self.anchor.checked_add_months(Months::new(k * self.months)))
            .find(|at| *at > now)
            .map(|at| Next { at, estimated: true, ends: false })
    }
}

/// `{"at", "estimated", "ends", "period"}` for the next charge, or null once a
/// cancelled subscription has ended.
impl Serialize for Renewal {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let Some(next) = self.next(Utc::now()) else { return s.serialize_none() };
        let mut out = s.serialize_struct("Renewal", 4)?;
        out.serialize_field("at", &next.at)?;
        out.serialize_field("estimated", &next.estimated)?;
        out.serialize_field("ends", &next.ends)?;
        out.serialize_field("period", &self.period)?;
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

    /// The fields production's profile returned on 2026-11-02 (identities left out).
    fn claude(org: Value) -> Value {
        let mut base = json!({
            "organization_type": "claude_max",
            "billing_type": "stripe_subscription",
            "rate_limit_tier": "default_claude_max_20x",
            "subscription_status": "active",
            "subscription_created_at": "2026-09-04T05:42:21.385091Z",
        });
        base.as_object_mut().unwrap().extend(org.as_object().unwrap().clone());
        json!({ "account": { "has_claude_max": true }, "organization": base })
    }

    #[test]
    fn claude_estimates_monthly_from_the_subscription_start() {
        let r = Renewal::from_claude(&claude(json!({}))).unwrap();
        let next = r.next(at("2026-11-02T12:00:00Z")).unwrap();
        assert_eq!(next, Next { at: at("2026-11-04T05:42:21.385091Z"), estimated: true, ends: false });
        // Just after a charge, the next one is a month on.
        assert_eq!(r.next(at("2026-11-04T06:00:00Z")).unwrap().at, at("2026-12-04T05:42:21.385091Z"));
        // Before the first charge.
        assert_eq!(r.next(at("2026-09-04T06:00:00Z")).unwrap().at, at("2026-10-04T05:42:21.385091Z"));
    }

    #[test]
    fn month_ends_keep_their_day() {
        let r = Renewal::from_claude(&claude(json!({ "subscription_created_at": "2024-01-31T15:34:42Z" }))).unwrap();
        assert_eq!(r.next(at("2024-02-10T00:00:00Z")).unwrap().at, at("2024-02-29T15:34:42Z"));
        assert_eq!(r.next(at("2024-03-01T00:00:00Z")).unwrap().at, at("2024-03-31T15:34:42Z"));
        assert_eq!(r.next(at("2026-11-02T00:00:00Z")).unwrap().at, at("2026-11-30T15:34:42Z"));
    }

    #[test]
    fn claude_without_an_active_subscription_has_no_date() {
        for org in [
            json!({ "subscription_status": "canceled" }),
            json!({ "subscription_status": null }),
            json!({ "subscription_created_at": null }),
            json!({ "subscription_created_at": "soon" }),
            json!({ "billing_type": "invoice" }),
        ] {
            assert_eq!(Renewal::from_claude(&claude(org.clone())), None, "{org}");
        }
        assert_eq!(Renewal::from_claude(&json!({})), None);
        // Older profiles without a billing type still count.
        assert!(Renewal::from_claude(&claude(json!({ "billing_type": null }))).is_some());
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
        assert_eq!(r.next(now).unwrap(), Next { at: at("2026-11-06T09:53:56Z"), estimated: false, ends: false });
        assert_eq!(serde_json::to_value(&r).unwrap()["period"], "monthly");
        // Not read again since it passed: a month on, as an estimate.
        assert_eq!(
            r.next(at("2026-11-06T10:00:00Z")).unwrap(),
            Next { at: at("2026-12-06T09:53:56Z"), estimated: true, ends: false }
        );
        let yearly = Renewal::from_codex(&codex(json!({ "billing_period": "yearly" }), json!({}))).unwrap();
        assert_eq!(yearly.next(at("2026-11-07T00:00:00Z")).unwrap().at, at("2027-11-06T09:53:56Z"));
        // Only the top-level date.
        let top = Renewal::from_codex(&codex(json!({ "renews_at": null }), json!({}))).unwrap();
        assert_eq!(top.next(now).unwrap().at, at("2026-11-06T09:53:56Z"));
    }

    #[test]
    fn a_cancelled_codex_subscription_ends() {
        let ends = |at: DateTime<Utc>| Next { at, estimated: false, ends: true };
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
}
