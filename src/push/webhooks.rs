//! The same notifications as chat messages. The URL says where it goes: Discord, Slack,
//! Microsoft Teams (Workflows), Telegram, ntfy, or anything else that takes JSON, which
//! gets a `text` field (Mattermost, Google Chat and Rocket.Chat show it as it is) and
//! the fault's details.

use serde_json::{Value, json};

use super::faults::{Event, Fault};
use crate::state::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Discord,
    Slack,
    Teams,
    Telegram,
    Ntfy,
    Generic,
}

impl Kind {
    pub fn of(url: &str) -> Kind {
        let Ok(u) = url::Url::parse(url.trim()) else { return Kind::Generic };
        let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
        let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
        if (under("discord.com") || under("discordapp.com")) && u.path().starts_with("/api/webhooks/") {
            Kind::Discord
        } else if host == "hooks.slack.com" {
            Kind::Slack
        } else if under("webhook.office.com")
            || under("logic.azure.com")
            || under("powerplatform.com")
            || under("powerautomate.com")
        {
            Kind::Teams
        } else if host == "api.telegram.org" {
            Kind::Telegram
        } else if host.split('.').any(|part| part == "ntfy") {
            Kind::Ntfy
        } else {
            Kind::Generic
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Discord => "Discord",
            Kind::Slack => "Slack",
            Kind::Teams => "Teams",
            Kind::Telegram => "Telegram",
            Kind::Ntfy => "ntfy",
            Kind::Generic => "Webhook",
        }
    }
}

/// How a webhook is named in logs, errors and the dashboard: its kind and host, never
/// its path or query, which usually hold the secret.
pub fn webhook_name(url: &str) -> String {
    match url::Url::parse(url.trim()).ok().and_then(|u| u.host_str().map(str::to_string)) {
        Some(host) => match Kind::of(url) {
            Kind::Generic => host,
            kind => format!("{} ({host})", kind.name()),
        },
        None => "a webhook URL".into(),
    }
}

/// One notification, whatever carries it.
pub struct Message<'a> {
    pub fault: &'a Fault,
    /// A provider that is back: good news, shown as such.
    pub recovered: bool,
    pub test: bool,
}

impl Message<'_> {
    fn colour(&self) -> u32 {
        if self.test {
            0xf2efe8
        } else if self.recovered {
            0x4ade80
        } else if self.fault.level == "err" {
            0xfb7185
        } else {
            0xfbbf24
        }
    }

    fn generic(&self) -> Value {
        let f = self.fault;
        json!({
            "text": format!("{}: {}", f.title, f.body),
            "title": f.title,
            "body": f.body,
            "source": "fusebox",
            "event": if self.test { "test" } else if self.recovered { "recovered" } else { event_name(f.event) },
            "key": f.key,
            "kind": f.kind,
            "level": f.level,
            "provider": f.provider,
            "provider_name": f.provider_name,
            "account": f.account,
            "label": f.label,
            "until": f.until.map(|t| t.to_rfc3339()),
        })
    }
}

fn event_name(e: Event) -> &'static str {
    match e {
        Event::SignIn => "sign_in_expired",
        Event::ProviderExhausted => "provider_exhausted",
        Event::AccountUsedUp => "account_used_up",
        Event::AccountErrors => "account_errors",
        Event::Quiet => "quiet",
    }
}

/// Sends one message to one webhook. Errors name the webhook, not its URL.
pub async fn send(app: &App, url: &str, msg: &Message<'_>) -> Result<(), String> {
    let f = msg.fault;
    let name = webhook_name(url);
    let client = app.http.control(None);
    let request = match Kind::of(url) {
        Kind::Discord => client.post(url.trim()).json(&json!({
            "username": "Fusebox",
            "embeds": [{ "title": f.title, "description": f.body, "color": msg.colour() }],
        })),
        Kind::Slack => client.post(url.trim()).json(&json!({ "text": format!("*{}*\n{}", f.title, f.body) })),
        Kind::Teams => client.post(url.trim()).json(&json!({
            "type": "message",
            "attachments": [{
                "contentType": "application/vnd.microsoft.card.adaptive",
                "content": {
                    "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
                    "type": "AdaptiveCard",
                    "version": "1.4",
                    "body": [
                        { "type": "TextBlock", "text": f.title, "weight": "Bolder", "wrap": true },
                        { "type": "TextBlock", "text": f.body, "wrap": true },
                    ],
                },
            }],
        })),
        Kind::Telegram => {
            // https://api.telegram.org/bot<token>/sendMessage?chat_id=<chat>
            let mut u = url::Url::parse(url.trim()).map_err(|_| format!("{name} is not a URL"))?;
            let chat = u.query_pairs().find(|(k, _)| k == "chat_id").map(|(_, v)| v.into_owned());
            let Some(chat) = chat else {
                return Err(format!("{name}: add ?chat_id=<your chat id> to the sendMessage URL"));
            };
            u.set_query(None);
            client.post(u).json(&json!({ "chat_id": chat, "text": format!("{}\n{}", f.title, f.body) }))
        }
        Kind::Ntfy => {
            let (tags, priority) = if msg.recovered || msg.test {
                ("white_check_mark", "3")
            } else if f.level == "err" {
                ("rotating_light", "4")
            } else {
                ("warning", "3")
            };
            client
                .post(url.trim())
                .header("Title", ascii_header(&f.title))
                .header("Tags", tags)
                .header("Priority", priority)
                .body(f.body.clone())
        }
        Kind::Generic => client.post(url.trim()).json(&msg.generic()),
    };
    let resp = request.send().await.map_err(|e| {
        let why = if e.is_timeout() { "timed out" } else { "is unreachable" };
        format!("{name} {why}")
    })?;
    let status = resp.status();
    if status.is_success() { Ok(()) } else { Err(format!("{name} answered {}", status.as_u16())) }
}

/// HTTP header values must be plain ASCII; ntfy titles lose anything else.
fn ascii_header(text: &str) -> String {
    text.chars().map(|c| if c.is_ascii() && !c.is_ascii_control() { c } else { '-' }).collect()
}

/// Sends a fault to every configured webhook, logging (never returning) failures.
pub async fn deliver_all(app: &App, urls: &[String], fault: &Fault) {
    let msg = Message { fault, recovered: fault.kind == "recovered", test: false };
    for url in urls {
        match send(app, url, &msg).await {
            Ok(()) => tracing::info!(fault = %fault.key, webhook = %webhook_name(url), "webhook sent"),
            Err(e) => tracing::warn!(fault = %fault.key, "webhook failed: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_says_where_a_notification_goes() {
        for (url, kind) in [
            ("https://discord.com/api/webhooks/1/abc", Kind::Discord),
            ("https://canary.discord.com/api/webhooks/1/abc", Kind::Discord),
            ("https://hooks.slack.com/services/T/B/X", Kind::Slack),
            ("https://prod-12.westus.logic.azure.com:443/workflows/x/triggers/manual/paths/invoke", Kind::Teams),
            ("https://acme.webhook.office.com/webhookb2/x", Kind::Teams),
            ("https://api.telegram.org/bot123:abc/sendMessage?chat_id=42", Kind::Telegram),
            ("https://ntfy.sh/fusebox-alerts", Kind::Ntfy),
            ("https://ntfy.example.com/alerts", Kind::Ntfy),
            ("https://chat.example.com/hooks/abc", Kind::Generic),
            ("https://discord.com/channels/1/2", Kind::Generic),
        ] {
            assert_eq!(Kind::of(url), kind, "{url}");
        }
    }

    #[test]
    fn names_never_show_the_secret_part() {
        assert_eq!(webhook_name("https://discord.com/api/webhooks/1/s3cret"), "Discord (discord.com)");
        assert_eq!(webhook_name("https://chat.example.com/hooks/s3cret?token=x"), "chat.example.com");
        assert_eq!(webhook_name("not a url s3cret"), "a webhook URL");
        assert!(!webhook_name("https://api.telegram.org/bot1:s3cret/sendMessage").contains("s3cret"));
    }

    #[test]
    fn the_generic_payload_has_text_and_details() {
        let fault = Fault {
            key: "signin:file:a.json".into(),
            event: Event::SignIn,
            title: "Claude sign-in expired".into(),
            body: "work@example.com".into(),
            kind: "signin",
            level: "err",
            ..Default::default()
        };
        let v = Message { fault: &fault, recovered: false, test: false }.generic();
        assert_eq!(v["text"], "Claude sign-in expired: work@example.com");
        assert_eq!(v["event"], "sign_in_expired");
        assert_eq!(v["level"], "err");
        assert_eq!(ascii_header("Claude · back"), "Claude - back");
    }
}
