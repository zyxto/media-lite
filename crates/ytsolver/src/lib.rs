//! Solucionador de los desafíos `n` y `sig` de YouTube.
//!
//! Los scripts oficiales de yt-dlp/ejs (ver `assets/README.md`) corren en QuickJS-NG, un motor
//! sin red ni disco. Se ejecutan en un proceso aparte (`media-lite --yt-solver`): se le pasa el
//! JSON por stdin y devuelve la respuesta por stdout. Cuando el proceso termina, su memoria vuelve
//! al sistema. Si se cuelga, el padre lo mata a los 60 segundos.

pub mod cache;
pub mod solve;
pub mod update;

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Versión de yt-dlp/ejs que trae el AppImage.
pub const EJS_VERSION: &str = "0.8.0";
const LIB: &[u8] = include_bytes!("../assets/yt.solver.lib.min.js");
const CORE: &[u8] = include_bytes!("../assets/yt.solver.core.min.js");
pub const LIB_SHA256: &str = "c55987fe697e5b9ee18830163f7af85327e9bb5c3e674b969d38c8d205eaa577";
pub const CORE_SHA256: &str = "18da6ce0758b416e7ae645084f4f8801f9f9d59d6c477c05eaa0ff94ebd8cc00";

/// Límites del solucionador. El análisis de `base.js` es lo más pesado: ver docs/hardware.md.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub memory_bytes: usize,
    pub time: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            memory_bytes: 256 * 1024 * 1024,
            time: Duration::from_secs(60),
        }
    }
}

/// Los dos scripts, ya verificados contra sus hashes.
#[derive(Clone)]
pub struct Scripts {
    pub lib: Vec<u8>,
    pub core: Vec<u8>,
}

impl Scripts {
    /// Los scripts que trae el binario. No dependen del disco.
    pub fn embedded() -> Scripts {
        Scripts {
            lib: LIB.to_vec(),
            core: CORE.to_vec(),
        }
    }

    /// Comprueba que los bytes coincidan con los hashes fijados.
    pub fn verify(&self) -> bool {
        sha256_hex(&self.lib) == LIB_SHA256 && sha256_hex(&self.core) == CORE_SHA256
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Ejecuta una petición del protocolo de ejs (`type` `player` o `preprocessed`).
/// Devuelve la respuesta de ejs (`{"type":"result",…}` o `{"type":"error",…}`).
pub fn run(input: &Value, scripts: &Scripts, limits: Limits) -> Result<Value, String> {
    let lib = std::str::from_utf8(&scripts.lib).map_err(|_| "lib no es UTF-8".to_string())?;
    let core = std::str::from_utf8(&scripts.core).map_err(|_| "core no es UTF-8".to_string())?;
    let runtime = rquickjs::Runtime::new().map_err(|e| format!("QuickJS: {e}"))?;
    runtime.set_memory_limit(limits.memory_bytes);
    let started = Instant::now();
    runtime.set_interrupt_handler(Some(Box::new(move || started.elapsed() > limits.time)));
    let context = rquickjs::Context::full(&runtime).map_err(|e| format!("QuickJS: {e}"))?;

    let text = context.with(|ctx| -> Result<String, String> {
        ctx.globals()
            .set("__input", input.to_string())
            .map_err(|e| format!("QuickJS: {e}"))?;
        // `lib` define `lib.meriyah` y `lib.astring`; `core` los necesita como globales.
        let setup = "var meriyah = lib.meriyah; var astring = lib.astring;";
        ctx.eval::<(), _>(lib).map_err(|e| js_error(&ctx, e))?;
        ctx.eval::<(), _>(setup).map_err(|e| js_error(&ctx, e))?;
        ctx.eval::<(), _>(core).map_err(|e| js_error(&ctx, e))?;
        // Los errores de JS se capturan aquí para que lleguen con su mensaje.
        let wrapped = "(function () { try { return JSON.stringify(jsc(JSON.parse(__input))); } \
                       catch (e) { return JSON.stringify({ type: 'error', error: String(e && e.message !== undefined ? e.message : e) }); } })()";
        ctx.eval::<String, _>(wrapped).map_err(|e| js_error(&ctx, e))
    })?;
    serde_json::from_str(&text).map_err(|e| format!("respuesta ilegible: {e}"))
}

fn js_error(ctx: &rquickjs::Ctx<'_>, e: rquickjs::Error) -> String {
    match e {
        rquickjs::Error::Exception => {
            let value: rquickjs::Value = ctx.catch();
            // Un `null` como excepción es el límite de memoria de QuickJS.
            if value.is_null() || value.is_undefined() {
                "memoria agotada o tiempo límite".to_string()
            } else {
                format!("excepción de JavaScript: {value:?}")
            }
        }
        other => other.to_string(),
    }
}

/// Lo que devuelve el proceso hijo: `{"ok": <respuesta>}` o `{"error": "…"}`.
pub fn child_main() -> i32 {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return fail("no se pudo leer la entrada");
    }
    let input: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return fail(&format!("entrada inválida: {e}")),
    };
    let reply = match run(&input, &Scripts::embedded(), Limits::default()) {
        Ok(out) => json!({ "ok": out }),
        Err(e) => json!({ "error": e }),
    };
    println!("{reply}");
    0
}

fn fail(message: &str) -> i32 {
    println!("{}", json!({ "error": message }));
    1
}

/// Ejecuta `input` en un proceso hijo `exe --yt-solver`, con tiempo límite.
pub fn run_child(exe: &Path, input: &Value, time: Duration) -> Result<Value, String> {
    let mut child = Command::new(exe)
        .arg("--yt-solver")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("no se pudo lanzar el solucionador: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let body = input.to_string();
        thread::spawn(move || {
            let _ = stdin.write_all(body.as_bytes());
        });
    }
    let mut stdout = child.stdout.take().ok_or("sin salida del solucionador")?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let out = match rx.recv_timeout(time + Duration::from_secs(5)) {
        Ok(out) => out,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err("el solucionador tardó demasiado".to_string());
        }
    };
    let _ = child.wait();
    let reply: Value = serde_json::from_str(out.trim())
        .map_err(|_| "respuesta del solucionador ilegible".to_string())?;
    if let Some(err) = reply.get("error").and_then(Value::as_str) {
        return Err(err.to_string());
    }
    reply
        .get("ok")
        .cloned()
        .ok_or_else(|| "respuesta del solucionador sin resultado".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_scripts_match_their_pinned_hashes() {
        assert!(Scripts::embedded().verify());
    }

    #[test]
    fn a_broken_script_is_reported_not_panicked() {
        let scripts = Scripts {
            lib: b"esto no es javascript (".to_vec(),
            core: CORE.to_vec(),
        };
        let err = run(&json!({"type": "player"}), &scripts, Limits::default()).unwrap_err();
        assert!(err.contains("excepción") || err.contains("Error") || !err.is_empty());
    }

    /// Con un `base.js` real (variable `MEDIA_LITE_TEST_BASEJS`): los resultados son los que dan
    /// Node y QuickJS-NG en el prototipo de la Fase 0 (player 11a45ffb).
    #[test]
    #[ignore = "necesita MEDIA_LITE_TEST_BASEJS con un base.js real"]
    fn solves_n_challenges_like_node() {
        let path = std::env::var("MEDIA_LITE_TEST_BASEJS").expect("definí MEDIA_LITE_TEST_BASEJS");
        let player = std::fs::read_to_string(path).expect("base.js");
        let input = json!({
            "type": "player", "player": player, "output_preprocessed": true,
            "requests": [{"type": "n", "challenges": ["AbCdEfGhIjKl", "xYz123QwErTy"]}]
        });
        let out = run(&input, &Scripts::embedded(), Limits::default()).expect("solver");
        let data = &out["responses"][0]["data"];
        assert_eq!(data["AbCdEfGhIjKl"], "h47ayNZgqc");
        assert_eq!(data["xYz123QwErTy"], "5tv1DQ3-SJ");
        assert!(out["preprocessed_player"].as_str().unwrap().len() > 1000);
    }
}
