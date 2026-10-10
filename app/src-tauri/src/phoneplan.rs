// What the user says about the phone and messages, parsed locally when it is
// obvious (FEDO.md Phase 6): "call Pulkit", "text Aparna I'll be right there",
// "open zapzap and text Aparna …", "send this to my phone", "read my
// notifications". Names are told from the message by asking the address book
// which leading words are a contact. Pure functions; the effects are in
// assist.rs behind the confirm card.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Sms,
    WhatsApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    WhatsApp,
    Gmail,
    Phone,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PhoneFast {
    Call { who: String },
    Text { who: String, message: String, channel: Channel },
    Notifications,
    /// Send the dropped file somewhere. `who` is None for the phone itself.
    SendFile { via: Via, who: Option<String> },
}

fn strip_message_lead(m: &str) -> String {
    let mut s = m.trim();
    loop {
        let l = s.to_lowercase();
        let mut cut = None;
        for lead in ["saying that ", "saying ", "to say that ", "to say ", "that ", "to tell her ", "to tell him ", ": ", "- ", ", "] {
            if l.starts_with(lead) {
                cut = Some(lead.len());
                break;
            }
        }
        match cut {
            Some(n) => s = s[n..].trim_start(),
            None => break,
        }
    }
    s.trim_matches(|c| c == '"' || c == '“' || c == '”').trim().to_string()
}

/// "... via sms" / "... on whatsapp" at the end: (the rest, channel).
fn trailing_channel(s: &str) -> (String, Option<Channel>) {
    let l = s.to_lowercase();
    for (suffix, ch) in [
        (" via sms", Channel::Sms),
        (" by sms", Channel::Sms),
        (" as an sms", Channel::Sms),
        (" via whatsapp", Channel::WhatsApp),
        (" on whatsapp", Channel::WhatsApp),
        (" via zapzap", Channel::WhatsApp),
        (" on zapzap", Channel::WhatsApp),
        (" in zapzap", Channel::WhatsApp),
        (" by whatsapp", Channel::WhatsApp),
    ] {
        if l.ends_with(suffix) {
            return (s[..s.len() - suffix.len()].trim_end_matches([',', ' ']).to_string(), Some(ch));
        }
    }
    (s.to_string(), None)
}

/// The leading words that are a contact: longest first, up to three words.
fn split_name<'a>(rest: &'a str, has: &dyn Fn(&str) -> bool) -> Option<(String, &'a str)> {
    let words: Vec<(usize, usize)> = {
        let mut v = Vec::new();
        let mut start = None;
        for (i, c) in rest.char_indices() {
            if c.is_whitespace() {
                if let Some(s) = start.take() {
                    v.push((s, i));
                }
            } else if start.is_none() {
                start = Some(i);
            }
        }
        if let Some(s) = start {
            v.push((s, rest.len()));
        }
        v
    };
    for n in (1..=words.len().min(3)).rev() {
        let end = words[n - 1].1;
        let name = rest[..end].trim_end_matches([',', ':']);
        if has(name) {
            return Some((name.to_string(), rest[end..].trim_start()));
        }
    }
    None
}

pub fn fast(text: &str, has_contact: &dyn Fn(&str) -> bool) -> Option<PhoneFast> {
    let t = text.trim();
    let l = t.to_lowercase();
    let l = l.trim_end_matches(['.', '!', '?']);

    if matches!(
        l,
        "read my notifications" | "read notifications" | "phone notifications" | "my phone notifications"
            | "what notifications do i have" | "any notifications" | "check my notifications"
            | "read my phone notifications" | "what did i miss" | "read my messages"
    ) {
        return Some(PhoneFast::Notifications);
    }

    // Send the dropped file.
    for lead in ["send it ", "send this ", "send the file ", "share it ", "share this "] {
        if let Some(rest) = l.strip_prefix(lead) {
            let rest = rest.strip_prefix("to ").unwrap_or(rest);
            if rest == "my phone" || rest == "phone" || rest == "to my phone" || rest == "to phone" {
                return Some(PhoneFast::SendFile { via: Via::Phone, who: None });
            }
            let (via, after) = if let Some(a) = rest.strip_prefix("via ").or_else(|| rest.strip_prefix("on ")) {
                if let Some(a) = a.strip_prefix("zapzap").or_else(|| a.strip_prefix("whatsapp")) {
                    (Some(Via::WhatsApp), a)
                } else if let Some(a) = a.strip_prefix("gmail").or_else(|| a.strip_prefix("email")).or_else(|| a.strip_prefix("mail")) {
                    (Some(Via::Gmail), a)
                } else {
                    (None, "")
                }
            } else {
                (None, "")
            };
            if let Some(via) = via {
                let who = after.trim().trim_start_matches("to ").trim();
                return Some(PhoneFast::SendFile {
                    via,
                    who: (!who.is_empty()).then(|| who.to_string()),
                });
            }
            // "send it to Aparna on whatsapp"
            let (body, _) = (rest, ());
            for (suffix, via) in [(" on whatsapp", Via::WhatsApp), (" via whatsapp", Via::WhatsApp), (" on zapzap", Via::WhatsApp), (" via zapzap", Via::WhatsApp), (" by email", Via::Gmail), (" via gmail", Via::Gmail), (" by mail", Via::Gmail)] {
                if let Some(who) = body.strip_suffix(suffix) {
                    return Some(PhoneFast::SendFile { via, who: Some(who.trim().to_string()) });
                }
            }
        }
    }

    // Call.
    for lead in ["call ", "ring ", "phone ", "give a call to ", "dial "] {
        if l.starts_with(lead) {
            let who = t[lead.len()..].trim().trim_end_matches(['.', '!', '?']).trim();
            if !who.is_empty() && who.split_whitespace().count() <= 3 {
                let who = who.strip_prefix("up ").unwrap_or(who);
                return Some(PhoneFast::Call { who: who.to_string() });
            }
        }
    }

    // Text / WhatsApp. "open zapzap and text …" first.
    let mut channel = None;
    let mut body = t;
    let lb = l.to_string();
    for lead in ["open zapzap and ", "open whatsapp and ", "using zapzap, ", "using whatsapp, "] {
        if lb.starts_with(lead) {
            channel = Some(Channel::WhatsApp);
            body = &t[lead.len()..];
        }
    }
    let lbody = body.to_lowercase();
    for (verb, ch) in [
        ("text ", Channel::Sms),
        ("sms ", Channel::Sms),
        ("message ", Channel::Sms),
        ("msg ", Channel::Sms),
        ("whatsapp ", Channel::WhatsApp),
        ("zapzap ", Channel::WhatsApp),
        ("tell ", Channel::Sms),
        ("send a message to ", Channel::Sms),
        ("send a text to ", Channel::Sms),
        ("send an sms to ", Channel::Sms),
    ] {
        if lbody.starts_with(verb) {
            let rest = body[verb.len()..].trim();
            let (rest, trailing) = trailing_channel(rest);
            let (name, msg) = split_name(&rest, has_contact)?;
            let message = strip_message_lead(msg);
            if message.is_empty() {
                return None;
            }
            // "whatsapp"/"zapzap" in the verb, "open zapzap and", or a trailing "via …".
            let channel = trailing.or(channel).unwrap_or(ch);
            return Some(PhoneFast::Text { who: name, message, channel });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(name: &str) -> bool {
        matches!(name.to_lowercase().as_str(), "aparna" | "pulkit" | "mom" | "pulkit sharma" | "aparna singh")
    }

    #[test]
    fn calls() {
        assert_eq!(fast("call Pulkit", &has), Some(PhoneFast::Call { who: "Pulkit".into() }));
        assert_eq!(fast("Call mom!", &has), Some(PhoneFast::Call { who: "mom".into() }));
        assert_eq!(fast("ring up Aparna Singh", &has), Some(PhoneFast::Call { who: "Aparna Singh".into() }));
        assert_eq!(fast("call it a day and go home early please", &has), None);
    }

    #[test]
    fn texts_split_the_name_from_the_message() {
        assert_eq!(
            fast("text Aparna I'll be right there", &has),
            Some(PhoneFast::Text { who: "Aparna".into(), message: "I'll be right there".into(), channel: Channel::Sms })
        );
        assert_eq!(
            fast("text Aparna Singh that I'm running late", &has),
            Some(PhoneFast::Text { who: "Aparna Singh".into(), message: "I'm running late".into(), channel: Channel::Sms })
        );
        assert_eq!(
            fast("open zapzap and text Aparna I'll be right there", &has),
            Some(PhoneFast::Text { who: "Aparna".into(), message: "I'll be right there".into(), channel: Channel::WhatsApp })
        );
        assert_eq!(
            fast("whatsapp mom: dinner at 8", &has),
            Some(PhoneFast::Text { who: "mom".into(), message: "dinner at 8".into(), channel: Channel::WhatsApp })
        );
        assert_eq!(
            fast("text Pulkit running late via whatsapp", &has),
            Some(PhoneFast::Text { who: "Pulkit".into(), message: "running late".into(), channel: Channel::WhatsApp })
        );
        // Not a contact, or nothing to say: not ours.
        assert_eq!(fast("text Zorg hello", &has), None);
        assert_eq!(fast("text Aparna", &has), None);
    }

    #[test]
    fn notifications_and_sending_the_dropped_file() {
        assert_eq!(fast("read my notifications", &has), Some(PhoneFast::Notifications));
        assert_eq!(fast("send this to my phone", &has), Some(PhoneFast::SendFile { via: Via::Phone, who: None }));
        assert_eq!(
            fast("send it via zapzap to Aparna", &has),
            Some(PhoneFast::SendFile { via: Via::WhatsApp, who: Some("aparna".into()) })
        );
        assert_eq!(
            fast("send it to Pulkit on whatsapp", &has),
            Some(PhoneFast::SendFile { via: Via::WhatsApp, who: Some("pulkit".into()) })
        );
        assert_eq!(
            fast("send it via gmail to hr@x.com", &has),
            Some(PhoneFast::SendFile { via: Via::Gmail, who: Some("hr@x.com".into()) })
        );
        assert_eq!(fast("send it via zapzap", &has), Some(PhoneFast::SendFile { via: Via::WhatsApp, who: None }));
    }
}
