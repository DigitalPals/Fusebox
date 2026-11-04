//! Reminders before a subscription charges again, or ends: two days ahead, a day ahead, or
//! both, as the settings say, once for each date and lead. Days are the server's local time,
//! as the rest of its notifications are.

use chrono::{Duration as Span, Local};

use super::*;

impl Push {
    /// The reminders due now: subscriptions that charge or end within one of `leads` (days
    /// ahead, shortest first) and whose day wasn't reminded of at that lead yet. A date that
    /// is already inside a shorter lead gets only that reminder, not the longer one late as
    /// well. Only dates the provider gave or that were set by hand for the current plan: a
    /// passed date moved on by a month (no read since) is no reason to remind. Each is
    /// remembered until a day after its date.
    pub(super) fn renewals(&self, accounts: &[AccountView], now: DateTime<Utc>, leads: &[i64]) -> Vec<Fault> {
        let mut stored = self.stored.lock();
        let before = stored.renewals.len();
        stored.renewals.retain(|_, at| *at > now - Span::days(1));
        let mut changed = stored.renewals.len() != before;
        let mut due = Vec::new();
        for a in accounts {
            let Some(next) = a.renewal.filter(|n| !n.estimated && n.at > now) else {
                continue;
            };
            let Some(&lead) = leads.iter().find(|&&days| next.at - now <= Span::days(days)) else {
                continue;
            };
            let day = next.at.with_timezone(&Local).date_naive();
            let key = |days: i64| match days {
                // Two days ahead keeps the key it had before there was a choice.
                2 => format!("renewal:{}:{day}", a.id),
                _ => format!("renewal:{}:{day}:{days}d", a.id),
            };
            if stored.renewals.contains_key(&key(lead)) {
                continue;
            }
            for &longer in leads.iter().filter(|&&d| d >= lead) {
                stored.renewals.insert(key(longer), next.at);
            }
            changed = true;
            due.push(reminder(a, next, key(lead), now));
        }
        if changed {
            self.save(&stored);
        }
        due
    }
}

/// "Codex subscription renews tomorrow", or "ends" when cancelled, with the day.
fn reminder(a: &AccountView, next: crate::quota::NextRenewal, key: String, now: DateTime<Utc>) -> Fault {
    let at = next.at.with_timezone(&Local);
    let when = match (at.date_naive() - now.with_timezone(&Local).date_naive()).num_days() {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        _ => format!("on {}", at.format("%A")),
    };
    let day = at.format("%A %-d %B");
    // A day set by hand has no time of day.
    let time = if next.manual { String::new() } else { format!(" at {}", at.format("%H:%M")) };
    let (verb, summary, detail) = if next.ends {
        ("ends", format!("Ends {day}"), format!("The subscription was cancelled and ends {day}{time}."))
    } else {
        ("renews", format!("Renews {day}"), format!("Charges again {day}{time}."))
    };
    Fault {
        key,
        event: Event::Renewal,
        provider: a.provider.clone(),
        provider_name: a.provider_name.clone(),
        title: format!("{} subscription {verb} {when}", a.group_name),
        body: format!("{}: {detail}", a.label),
        summary,
        detail: Some(detail),
        path: format!("#/accounts/{}", url::form_urlencoded::byte_serialize(a.id.as_bytes()).collect::<String>()),
        kind: "renewal",
        level: "ok",
        account: Some(a.id.clone()),
        label: Some(a.label.clone()),
        until: Some(next.at),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::NextRenewal;

    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp() -> Temp {
        let p = std::env::temp_dir().join(format!("renewals-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        Temp(p)
    }

    fn account(id: &str, renewal: Option<NextRenewal>) -> AccountView {
        AccountView {
            id: id.into(),
            provider: "codex".into(),
            provider_name: "Codex".into(),
            group_name: "Codex".into(),
            label: format!("{id}@example.com"),
            renewal,
            ..Default::default()
        }
    }

    fn renews(at: DateTime<Utc>) -> Option<NextRenewal> {
        Some(NextRenewal { at, estimated: false, ends: false, manual: false })
    }

    #[test]
    fn each_date_is_reminded_once_two_days_ahead_and_survives_a_restart() {
        let temp = temp();
        let push = Push::load(&temp.0);
        let now = Utc::now();
        let later = [account("a", renews(now + Span::days(3))), account("b", None)];
        assert!(push.renewals(&later, now, &[2]).is_empty(), "three days out is too early");
        let soon = [account("a", renews(now + Span::hours(47))), account("b", None)];
        let sent = push.renewals(&soon, now, &[2]);
        assert_eq!(sent.len(), 1);
        assert_eq!((sent[0].event, sent[0].kind, sent[0].account.as_deref()), (Event::Renewal, "renewal", Some("a")));
        assert!(sent[0].title.starts_with("Codex subscription renews "), "{}", sent[0].title);
        assert!(sent[0].body.starts_with("a@example.com: Charges again "), "{}", sent[0].body);
        assert!(push.renewals(&soon, now, &[2]).is_empty(), "already reminded");
        assert!(Push::load(&temp.0).renewals(&soon, now, &[2]).is_empty(), "a restart doesn't remind again");
        // A month on, the next charge is reminded too, and the old date is forgotten.
        let month = now + Span::days(30);
        let next = [account("a", renews(month + Span::hours(40)))];
        assert_eq!(push.renewals(&next, month, &[2]).len(), 1);
        assert_eq!(push.stored.lock().renewals.len(), 1);
        // A date already passed is no reminder.
        assert!(push.renewals(&[account("c", renews(now - Span::minutes(1)))], now, &[2]).is_empty());
    }

    #[test]
    fn endings_say_so_and_estimates_are_not_reminded() {
        let temp = temp();
        let push = Push::load(&temp.0);
        let now = Utc::now();
        let at = now + Span::hours(20);
        let accounts = [
            account("moved-on", Some(NextRenewal { at, estimated: true, ends: false, manual: false })),
            account("cancelled", Some(NextRenewal { at, estimated: false, ends: true, manual: false })),
        ];
        let sent = push.renewals(&accounts, now, &[2]);
        assert_eq!(sent.len(), 1, "a date moved on without a read is no reminder");
        assert!(sent[0].title.starts_with("Codex subscription ends "), "{}", sent[0].title);
        assert!(sent[0].body.contains("was cancelled and ends"), "{}", sent[0].body);
        assert_eq!(sent[0].until, Some(at));
    }

    #[test]
    fn the_day_is_named_relative_to_today() {
        let now = Local::now().date_naive().and_hms_opt(9, 0, 0).unwrap().and_local_timezone(Local).unwrap();
        let now = now.with_timezone(&Utc);
        let title =
            |hours| reminder(&account("a", None), renews(now + Span::hours(hours)).unwrap(), "k".into(), now).title;
        assert_eq!(title(2), "Codex subscription renews today");
        assert_eq!(title(24), "Codex subscription renews tomorrow");
        let day = (now + Span::hours(47)).with_timezone(&Local).format("%A").to_string();
        assert_eq!(title(47), format!("Codex subscription renews on {day}"));
    }

    #[test]
    fn a_day_set_by_hand_is_reminded_without_a_time() {
        let temp = temp();
        let push = Push::load(&temp.0);
        let now = Utc::now();
        let at = now + Span::hours(30);
        let mut claude = account("claude", Some(NextRenewal { at, estimated: false, ends: false, manual: true }));
        claude.group_name = "Claude".into();
        let sent = push.renewals(&[claude], now, &[2]);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].title.starts_with("Claude subscription renews "), "{}", sent[0].title);
        let day = at.with_timezone(&Local).format("%A %-d %B");
        assert_eq!(sent[0].detail.as_deref(), Some(format!("Charges again {day}.").as_str()));
        // Set before a plan change: the day may have moved, so no reminder.
        let unsure = account("unsure", Some(NextRenewal { at, estimated: true, ends: false, manual: true }));
        assert!(push.renewals(&[unsure], now, &[1, 2]).is_empty());
    }

    #[test]
    fn a_day_before_comes_alone_or_after_the_two_day_reminder() {
        let temp = temp();
        let push = Push::load(&temp.0);
        let now = Utc::now();
        let at = now + Span::hours(47);
        let a = [account("a", renews(at))];
        // Only a day before: two days out is too early.
        assert!(push.renewals(&a, now, &[1]).is_empty());
        let sent = push.renewals(&a, at - Span::hours(23), &[1]);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].title.ends_with(" tomorrow") || sent[0].title.ends_with(" today"), "{}", sent[0].title);
        assert!(push.renewals(&a, at - Span::hours(22), &[1]).is_empty(), "already reminded");
        // Both: two days ahead, then again a day ahead, and each once.
        let both = [account("b", renews(at))];
        assert_eq!(push.renewals(&both, now, &[1, 2]).len(), 1);
        assert!(push.renewals(&both, now + Span::hours(1), &[1, 2]).is_empty());
        let later = push.renewals(&both, at - Span::hours(23), &[1, 2]);
        assert_eq!(later.len(), 1);
        assert_ne!(later[0].key, format!("renewal:b:{}", at.with_timezone(&Local).date_naive()));
        assert!(Push::load(&temp.0).renewals(&both, at - Span::hours(22), &[1, 2]).is_empty(), "nor after a restart");
        // A date first seen inside a day gets the day-before reminder only, not both at once.
        let late = [account("c", renews(now + Span::hours(20)))];
        assert_eq!(push.renewals(&late, now, &[1, 2]).len(), 1);
        assert!(push.renewals(&late, now + Span::hours(1), &[1, 2]).is_empty());
        // Neither turned on: nothing.
        assert!(push.renewals(&[account("d", renews(now + Span::hours(20)))], now, &[]).is_empty());
    }
}
