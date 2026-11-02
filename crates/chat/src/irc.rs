//! IRC de Twitch con las etiquetas (`CAP REQ :twitch.tv/tags`): mensajes, emotes y avisos.

use crate::{Emote, Event, Message};

/// Imagen de un emote de Twitch.
pub fn emote_url(id: &str) -> String {
    format!("https://static-cdn.jtvnw.net/emoticons/v2/{id}/default/dark/1.0")
}

/// Resultado de leer una línea del servidor.
#[derive(Debug, PartialEq)]
pub enum Line {
    /// Hay que responder con esta línea (`PONG`).
    Reply(String),
    Event(Event),
    Ignored,
}

/// Lee una línea de IRC (sin el salto de línea).
pub fn parse_line(line: &str) -> Line {
    if let Some(rest) = line.strip_prefix("PING") {
        return Line::Reply(format!("PONG{rest}"));
    }
    let Some(tagged) = line.strip_prefix('@') else {
        return Line::Ignored;
    };
    let Some((tags, rest)) = tagged.split_once(' ') else {
        return Line::Ignored;
    };
    // Resto de la línea: `:prefijo COMANDO #canal[ :texto]`.
    let mut parts = rest.splitn(4, ' ');
    let _prefix = parts.next();
    let command = parts.next().unwrap_or("");
    let _channel = parts.next();
    let text = parts
        .next()
        .map(|t| t.strip_prefix(':').unwrap_or(t))
        .unwrap_or("");
    match command {
        "PRIVMSG" => Line::Event(Event::Message(message(tags, text))),
        "USERNOTICE" => match notice(tags, text) {
            Some(n) => Line::Event(Event::Notice(n)),
            None => Line::Ignored,
        },
        _ => Line::Ignored,
    }
}

/// Valor de una etiqueta (`nombre=valor`), sin escapes.
pub fn tag(tags: &str, name: &str) -> Option<String> {
    tags.split(';')
        .find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
        .map(unescape)
}

/// Las etiquetas escapan el espacio (`\s`), el punto y coma (`\:`), la barra (`\\`), etc.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some(':') => out.push(';'),
            Some('r') => out.push('\r'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn author(tags: &str) -> String {
    tag(tags, "display-name")
        .filter(|s| !s.is_empty())
        .or_else(|| tag(tags, "login"))
        .unwrap_or_default()
}

fn message(tags: &str, text: &str) -> Message {
    let (mut body, emotes) = emotes_in(text, &tag(tags, "emotes").unwrap_or_default());
    if let Some(bits) = tag(tags, "bits") {
        body = format!("[{bits} bits] {body}");
    }
    Message {
        author: author(tags),
        text: body,
        color: tag(tags, "color").filter(|c| !c.is_empty()),
        emotes,
        time_ms: None,
    }
}

fn notice(tags: &str, text: &str) -> Option<String> {
    let system = tag(tags, "system-msg").unwrap_or_default();
    let line = match (system.is_empty(), text.is_empty()) {
        (true, true) => String::new(),
        (true, false) => text.to_string(),
        (false, true) => system,
        (false, false) => format!("{system} · {text}"),
    };
    (!line.is_empty()).then_some(line)
}

/// Los emotes de un mensaje. `spec` tiene la forma `25:0-4,6-10/1902:12-16` (id y posiciones en
/// caracteres, contando desde 0). El texto no cambia: los emotes ya aparecen con su nombre.
pub fn emotes_in(text: &str, spec: &str) -> (String, Vec<Emote>) {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    for part in spec.split('/').filter(|p| !p.is_empty()) {
        let Some((id, ranges)) = part.split_once(':') else {
            continue;
        };
        for range in ranges.split(',') {
            let Some((a, b)) = range.split_once('-') else {
                continue;
            };
            let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) else {
                continue;
            };
            if a > b || b >= chars.len() {
                continue;
            }
            out.push(Emote {
                name: chars[a..=b].iter().collect(),
                url: emote_url(id),
            });
        }
    }
    (text.to_string(), out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privmsg_with_tags_and_emotes() {
        let line = "@badge-info=;color=#FF0000;display-name=Ana;emotes=25:0-4/1902:11-15;id=1 :ana!ana@ana.tmi.twitch.tv PRIVMSG #ninja :Kappa hola Keepo";
        let Line::Event(Event::Message(m)) = parse_line(line) else {
            panic!("se esperaba un mensaje");
        };
        assert_eq!(m.author, "Ana");
        assert_eq!(m.color.as_deref(), Some("#FF0000"));
        assert_eq!(m.text, "Kappa hola Keepo");
        assert_eq!(m.emotes.len(), 2);
        assert_eq!(m.emotes[0].name, "Kappa");
        assert_eq!(m.emotes[0].url, emote_url("25"));
        assert_eq!(m.emotes[1].name, "Keepo");
    }

    #[test]
    fn ping_is_answered_with_pong() {
        assert_eq!(
            parse_line("PING :tmi.twitch.tv"),
            Line::Reply("PONG :tmi.twitch.tv".into())
        );
    }

    #[test]
    fn bits_are_shown_in_the_text() {
        let line = "@bits=100;display-name=Luis :luis!luis@luis PRIVMSG #x :gracias";
        let Line::Event(Event::Message(m)) = parse_line(line) else {
            panic!("se esperaba un mensaje");
        };
        assert_eq!(m.text, "[100 bits] gracias");
    }

    #[test]
    fn subscription_is_a_notice() {
        let line = "@msg-id=sub;system-msg=Ana\\ssubscribed\\sfor\\s3\\smonths!;login=ana :tmi.twitch.tv USERNOTICE #x :gracias";
        assert_eq!(
            parse_line(line),
            Line::Event(Event::Notice(
                "Ana subscribed for 3 months! · gracias".into()
            ))
        );
    }

    #[test]
    fn other_commands_and_empty_color_are_ignored() {
        assert_eq!(
            parse_line(":tmi.twitch.tv 001 justinfan1 :Welcome"),
            Line::Ignored
        );
        let line = "@color=;display-name=Z :z!z@z PRIVMSG #x :hola";
        let Line::Event(Event::Message(m)) = parse_line(line) else {
            panic!("se esperaba un mensaje");
        };
        assert_eq!(m.color, None);
    }

    #[test]
    fn emote_ranges_count_characters_not_bytes() {
        let (_, emotes) = emotes_in("ñandú Kappa", "25:6-10");
        assert_eq!(emotes[0].name, "Kappa");
    }
}
