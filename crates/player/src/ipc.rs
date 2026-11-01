//! Cliente del protocolo JSON IPC de mpv: un comando por línea por el socket Unix.
//!
//! mpv responde con eventos (`property-change`, `file-loaded`, `end-file`…). Un hilo los lee y
//! los pasa por un canal, así que la interfaz nunca se bloquea esperando a mpv.

use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender};
use serde_json::{json, Value};

/// Lo que mpv informa.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Cambió una propiedad observada (`time-pos`, `pause`…). `value` es `null` si no tiene valor.
    Property {
        name: String,
        value: Value,
    },
    FileLoaded,
    /// Terminó el archivo. `reason` es `eof`, `stop`, `quit`, `error` o `redirect`.
    EndFile {
        reason: String,
    },
}

/// Escribe comandos en el socket. Los eventos llegan por el `Receiver` de `connect`.
pub struct Ipc {
    writer: Mutex<UnixStream>,
}

impl Ipc {
    /// Conecta al socket de mpv. mpv tarda un momento en crearlo, así que reintenta hasta `timeout`.
    pub fn connect(path: &Path, timeout: Duration) -> io::Result<(Ipc, Receiver<Event>)> {
        let started = Instant::now();
        let stream = loop {
            match UnixStream::connect(path) {
                Ok(stream) => break stream,
                Err(e) if started.elapsed() < timeout => {
                    log::trace!("esperando el socket de mpv: {e}");
                    thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        };
        let reader = stream.try_clone()?;
        let (tx, rx) = unbounded();
        thread::Builder::new()
            .name("mpv-ipc".into())
            .spawn(move || read_loop(reader, tx))?;
        Ok((
            Ipc {
                writer: Mutex::new(stream),
            },
            rx,
        ))
    }

    /// Envía un comando. `args` es la lista tal como la pide mpv: `["set_property", "pause", true]`.
    pub fn command(&self, args: &[Value]) -> io::Result<()> {
        let mut line = json!({ "command": args }).to_string();
        line.push('\n');
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("socket envenenado"))?;
        writer.write_all(line.as_bytes())
    }

    /// Pide a mpv que avise cada vez que cambie la propiedad `name`.
    pub fn observe(&self, id: u64, name: &str) -> io::Result<()> {
        self.command(&[json!("observe_property"), json!(id), json!(name)])
    }
}

fn read_loop(reader: UnixStream, tx: Sender<Event>) {
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(event) = parse_event(&value) {
            if tx.send(event).is_err() {
                break;
            }
        }
    }
    log::debug!("el socket de mpv se cerró");
}

/// Traduce una línea de mpv a un `Event`. Las respuestas a comandos y otros avisos se descartan.
pub fn parse_event(value: &Value) -> Option<Event> {
    match value.get("event")?.as_str()? {
        "property-change" => Some(Event::Property {
            name: value.get("name")?.as_str()?.to_string(),
            value: value.get("data").cloned().unwrap_or(Value::Null),
        }),
        "file-loaded" => Some(Event::FileLoaded),
        "end-file" => Some(Event::EndFile {
            reason: value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_property_changes() {
        let v: Value = serde_json::from_str(
            r#"{"event":"property-change","id":1,"name":"time-pos","data":12.5}"#,
        )
        .unwrap();
        assert_eq!(
            parse_event(&v),
            Some(Event::Property {
                name: "time-pos".into(),
                value: json!(12.5)
            })
        );
    }

    #[test]
    fn property_without_data_is_null() {
        let v: Value =
            serde_json::from_str(r#"{"event":"property-change","name":"duration"}"#).unwrap();
        assert_eq!(
            parse_event(&v),
            Some(Event::Property {
                name: "duration".into(),
                value: Value::Null
            })
        );
    }

    #[test]
    fn parses_end_file_reason() {
        let v: Value = serde_json::from_str(r#"{"event":"end-file","reason":"eof"}"#).unwrap();
        assert_eq!(
            parse_event(&v),
            Some(Event::EndFile {
                reason: "eof".into()
            })
        );
    }

    #[test]
    fn command_replies_are_ignored() {
        let v: Value = serde_json::from_str(r#"{"error":"success","request_id":0}"#).unwrap();
        assert_eq!(parse_event(&v), None);
    }
}
