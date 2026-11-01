# Roadmap: media-lite

Una sola aplicación en Rust que junta **Lumière** (Pluto TV, Runtime, Plex) y **Sanson**
(YouTube, Twitch, Kick, YouTube Music) y se distribuye como **AppImage** para antiX y cualquier
Debian-like de 64 bits.

**Equipo objetivo:** Pentium T4500 (2 núcleos, 2,3 GHz, SSE3/SSSE3, **sin SSE4**), 2 GB de RAM,
gráficos Intel integrados (probablemente GMA 4500M: OpenGL 2.1, sin Vulkan), antiX (X11, sin
systemd).

---

## Estado (1 de noviembre de 2026)

- **Publicado:** `v0.3.0` en `main` (AppImage en GitHub Releases; `v0.2.1` ya estaba publicada). `v0.1.0` tiene el primer AppImage con TV gratis.
- **Hecho:** Fases 1, 2, 3 y 9 (con pendientes), 4a (solucionador), y parte de la 7 (Ajustes, biblioteca, búsqueda global, miniaturas solo de lo visible).
- **Sin hacer:** YouTube (4b y 4c: streams y catálogo), Twitch y Kick (5), música (6), cuentas (8), y lo que quedó pendiente en cada fase.
- **Pendiente de la laptop:** mediciones de la Fase 0, prueba de las distros de la Fase 9, y medir la RAM en release con 60 elementos.

Ver `docs/hardware.md` para lo medido y `README.md` para instalar.

## 1. Objetivos y presupuesto

| Métrica | Meta | Referencia |
|---|---|---|
| RAM de la app en reposo (grilla con miniaturas) | **< 60 MB** | Firefox con YouTube abierto: 600 MB–1,2 GB |
| RAM total viendo un video a 480p (app + mpv) | **< 220 MB** | |
| CPU viendo 480p H.264 | **< 60 %** de un núcleo | |
| Binario | **< 14 MB** | Incluye QuickJS-NG (~1,5 MB) para los desafíos de YouTube |
| AppImage | **< 10 MB** | Sanson.tcz: 1,1 MB, pero sin toolkit propio |
| Arranque hasta ver la portada | **< 2 s** (sin contar red) | |
| Abrir un video de YouTube (reproductor de YouTube ya en caché) | **< 3 s** | Sanson: 1–2 s con VisionOS |
| Primera vez tras un cambio del reproductor de YouTube | **< 30 s**, una vez cada varios días | Medir en la Fase 0 |
| Pico de RAM del solucionador JS (proceso aparte, temporal) | **< 256 MB** | Medido: 210 MB la primera vez, 76 MB con preprocesado. Se libera al terminar |

Principios que guían todas las decisiones:

1. **El trabajo pesado lo hace mpv**, que es un proceso aparte y ya está optimizado en C.
   La app solo navega catálogos, consigue URLs y dibuja la interfaz.
2. **Nada de navegador embebido** (ni WebView, ni Electron, ni Tauri). En este equipo un
   WebKitGTK solo ya se lleva más de 150 MB.
3. **Códecs livianos primero**: H.264 + AAC. VP9 se decodifica mal en este CPU y AV1 no se puede ver.
4. **Todo lo que crece tiene límite**: caché de miniaturas, historial, mensajes del chat, buffer de mpv.
5. **El JavaScript queda encerrado**: solo se ejecuta para los desafíos de YouTube, en un proceso aparte que termina y libera la memoria.

---

## 2. Decisiones de arquitectura

### 2.1 Interfaz: Slint

| Opción | RAM | Aspecto | En GMA 4500 | Veredicto |
|---|---|---|---|---|
| **Slint** | 15–40 MB | Moderno (lenguaje declarativo `.slint`, temas, animaciones) | **Tiene renderizador por software**; femtovg pide solo GL ES 2.0 | ✅ **Elegida** |
| egui / eframe | 30–60 MB | Correcto, se nota "de herramienta" | Repinta seguido y hace falta afinar GL 2.1 | Plan B |
| iced | 40–80 MB | Moderno | wgpu pide Vulkan/GL 3.3, así que cae a tiny-skia | ❌ |
| GTK4 (relm4) | 80–150 MB | Moderno | Cae a Cairo; meterlo en el AppImage es pesado | ❌ |
| fltk-rs | 10–20 MB | Igual que hoy (anticuado) | Perfecto | Solo si todo falla |

- Backend `winit` sobre X11. El renderizador se elige con `SLINT_BACKEND` y queda guardado
  en Ajustes: `winit-software` es el más seguro, `winit-femtovg` es más fluido si el GL anda.
- Fuente incluida: **Inter** recortada a latín (~300 KB), para que se vea igual en cualquier distro.
- Licencia: Slint se puede usar bajo GPLv3, la misma que tienen hoy Lumière y Sanson.

### 2.2 Reproductor: mpv (proceso aparte, controlado por JSON IPC)

Pasar de mplayer a **mpv** elimina gran parte del código de las referencias:

| En las referencias | Por qué existía | En media-lite |
|---|---|---|
| `stream.c` (proxy HTTP local, ~1000 líneas) | El mplayer de FLinux no abre HTTPS ni HLS bien | **Desaparece casi entero**: mpv/ffmpeg abren HTTPS, HLS, fMP4, AES-128 y audio aparte (`--audio-file`) |
| `aes.c` | Descifrar segmentos HLS | Desaparece (lo hace ffmpeg) |
| Descarga y conversión de subtítulos WebVTT/HLS | mplayer no los entiende | mpv los toma del master HLS o con `sub-add <url>` |
| `player.cpp` con `-slave` y parseo de texto | Control de mplayer | Cliente JSON IPC por socket Unix (~300 líneas) |
| `mpkeys.h` | Teclas tipo mplayer | Un `input.conf` incluido en el AppImage con la misma tabla de teclas |

Lo que **sí** sigue haciendo falta: un **mini-proxy que solo reescribe playlists `.m3u8`**
(texto, no video). Se usa para saltar los anuncios de Twitch y para sacar los de Pluto TV
(`SP_SKIP_ADS`, `SP_PLUTO_ADS`). mpv baja los segmentos directo del CDN.

Opciones de mpv para este equipo (el usuario las puede cambiar en Ajustes):

```
--ytdl=no                       # las URLs las resuelve media-lite
--hwdec=auto-safe               # VA-API si el driver i965 decodifica H.264 en esta GPU (verificar con vainfo)
--vo=gpu  | --vo=xv             # xv es el "video liviano" si gpu va lento en GL 2.1
--profile=fast
--vd-lavc-skiploopfilter=all    # lo mismo que hacía Sanson con mplayer
--framedrop=vo
--cache=yes --demuxer-max-bytes=24MiB --demuxer-max-back-bytes=8MiB   # RAM acotada
--input-ipc-server=$XDG_RUNTIME_DIR/media-lite/mpv.sock
--input-conf=$APPDIR/usr/share/media-lite/input.conf
```

Tres formas de mostrar el video:

1. **Ventana flotante (PiP)**: mpv en su propia ventana, sin bordes, siempre encima y en una esquina
   (`--ontop --no-border --geometry=480x270-16-16`). **Es lo que pediste: ver videos mientras
   trabajás.** Es lo primero que se implementa.
2. **Integrado**: mpv dibuja dentro de la ventana de la app (`--wid` sobre una ventana hija X11
   creada con `x11rb` en el área del video, al lado del panel con chat, episodios o guía).
3. **Solo audio** para la música (`--no-video`): casi no usa CPU.

**mpv no va dentro del AppImage.** Es una dependencia del sistema (`sudo apt install mpv`). Así
el AppImage queda en pocos MB y mpv usa los drivers VA-API de la distro. Si no lo encuentra al
arrancar, la app muestra el comando para instalarlo.

### 2.3 Proveedores: Rust nativo, con JavaScript solo donde YouTube lo exige

Sanson corre cada plataforma como un plugin JS dentro de QuickJS. En media-lite **la lógica de
las plataformas se escribe en Rust**, detrás de un solo trait. Un motor de JavaScript sigue haciendo
falta, pero **solo para los desafíos de YouTube** (ver 2.4), metido en una caja sin red ni disco:

- Un solo lenguaje para catálogos, búsquedas y listas, con tipos verificados. No queda un
  intérprete abierto por plataforma.
- **Para no recompilar cada vez que YouTube cambia algo**, los datos que cambian seguido
  (`clientVersion`, user agents, orden de los clientes, parámetros de pestañas) van en un
  `clients.toml` incluido en el binario que se puede reemplazar desde `~/.config/media-lite/`.

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> PlatformId;
    fn caps(&self) -> Caps;                          // LIVE | MOVIES | SERIES | VIDEOS | MUSIC | CHAT | GUIDE | LOGIN …
    fn home(&self, ctx: &Ctx) -> Result<Vec<Shelf>>;
    fn categories(&self, sec: Section, ctx: &Ctx) -> Result<Vec<Category>>;
    fn list(&self, sec: Section, cat: &str, page: Option<Cursor>, ctx: &Ctx) -> Result<Page>;
    fn search(&self, q: &str, page: Option<Cursor>, ctx: &Ctx) -> Result<Page>;
    fn details(&self, item: &Item, ctx: &Ctx) -> Result<Details>;     // fuentes, audios, subtítulos
    fn seasons(&self, series: &Item, ctx: &Ctx) -> Result<Vec<Season>> { unsupported() }
    fn guide(&self, channel: &Item, ctx: &Ctx) -> Result<Vec<Program>> { unsupported() }
    fn related(&self, item: &Item, ctx: &Ctx) -> Result<Page> { unsupported() }
    fn comments(&self, item: &Item, page: Option<Cursor>, ctx: &Ctx) -> Result<Page> { unsupported() }
    fn chat(&self, item: &Item, ctx: &Ctx) -> Result<Box<dyn ChatSource>> { unsupported() }
}
```

En la implementación de la Fase 1 se simplificó lo siguiente: los proveedores guardan su propio agente HTTP, así que no reciben un `ctx`; `caps()` devuelve una estructura `Caps` con un booleano por función; `categories()` tiene un valor por defecto vacío. `home()` llega en la Fase 3.

El modelo `Item / Page / Details / Source / Track / Season / Program` sale de juntar
`lumiere/src/backend.h` con los campos de Sanson (`author_url`, `views`, `subs`, `is_short`, …),
que Lumière ya tenía en parte "por compatibilidad".

### 2.4 YouTube: desafíos de JavaScript, PO tokens y SABR

**Situación a noviembre de 2026** (fuentes al final):

- **YouTube necesita un motor de JavaScript.** Las URLs de los formatos traen dos desafíos que hay
  que resolver ejecutando el código del reproductor de YouTube (`base.js`, de unos 2–3 MB). El
  desafío `n` existe para frenar la descarga, y si no se resuelve el video va lentísimo o da 403.
  El desafío `sig` es la firma de las URLs cifradas. Desde la versión 2025.11.12, **yt-dlp pide un
  motor externo** (Deno, Node, QuickJS o QuickJS-NG) más los scripts del proyecto
  [`yt-dlp/ejs`](https://github.com/yt-dlp/ejs). Sin motor, el soporte de YouTube figura como obsoleto y faltan formatos.
- **Los clientes "sin JS" se están cerrando.** Sanson usa el cliente VisionOS, que hoy todavía
  entrega un HLS completo (H.264 hasta 1080p) sin PO token ni JS. Pero `android_vr`, que estaba en la
  misma situación, pasó a pedir PO token a mediados de 2026. VisionOS tampoco funciona con videos
  "para niños" y a veces pide iniciar sesión. **No se puede depender de un solo cliente.**
- **PO tokens:** son tokens antibots que se generan con BotGuard, algo que en la práctica necesita un
  navegador. Generarlos en este equipo es inviable. **media-lite usa solo clientes que no piden PO
  token** (lo explica la guía de PO tokens de yt-dlp):
  - `web_safari`: sus formatos **HLS** no piden token por ahora, pero sí resolver los desafíos JS.
  - `web_embedded`: solo funciona con videos que se pueden insertar en otras páginas.
  - `tv`: necesita cookies de sesión o de invitado.
  - Los **directos en HLS** no piden token en ningún cliente salvo `ios`.
- **SABR** es el protocolo propio de YouTube que entrega el video en pedazos según el ancho de banda.
  mpv/ffmpeg **no lo reproducen**, así que se descartan los clientes que solo entregan SABR (`web`
  sin token).
- Las URLs de los streams **vencen a las ~6 h** y **quedan atadas a la IP** que las pidió. Por eso se
  resuelven justo antes de reproducir y se vuelven a pedir si mpv recibe un 403. Si cambiás de VPN a
  mitad de un video, también hay que pedirlas de nuevo.

**Diseño en media-lite:**

```
           ┌──────────────── clients.toml (reemplazable) ────────────────┐
 videoId → │ 1. visionos     HLS sin JS ni token          (rápido)       │ → URLs H.264/AAC → mpv
           │ 2. web_safari   HLS, desafío n con JS        (normal)       │
           │ 3. web_embedded / tv con JS                  (normal)       │
           │ 4. yt-dlp -J si está instalado, con su motor (lento, último)│
           └─────────────────────────────────────────────────────────────┘
```

1. **Motor embebido: QuickJS-NG mediante `rquickjs`** (licencia MIT; suma ~1–1,5 MB al binario). Es
   el mismo motor que ya traía Sanson, pero acá **solo ejecuta el solucionador**: no tiene
   `fetch`, ni sockets, ni disco. yt-dlp advierte que con QuickJS-NG **anterior a 0.12** resolver un
   desafío puede tardar *varios minutos*. **Verificado**: `rquickjs-sys` 0.14.0 trae QuickJS-NG 0.16.2
   (ver `docs/hardware.md`).
   - Descartados: **Deno/Node** (de 40 a 100 MB de binario y más de 60 MB de RAM: demasiado para
     esta laptop) y **Boa** (escrito en Rust puro, pero bastante más lento que QuickJS).
2. **Solucionador: los scripts oficiales de [`yt-dlp/ejs`](https://github.com/yt-dlp/ejs)**
   (licencia Unlicense; incluyen meriyah [ISC] y astring [MIT]). No se reescriben: los mantiene el
   equipo de yt-dlp y se actualizan cada vez que YouTube cambia el reproductor. Su entrada y salida en JSON:
   ```jsonc
   // primera vez con un base.js nuevo: lento (hay que analizar unos 2–3 MB de JS)
   { "type": "player", "player": "<base.js>", "output_preprocessed": true,
     "requests": [{ "type": "n", "challenges": ["<n1>", "<n2>"] }, { "type": "sig", "challenges": ["…"] }] }
   // las siguientes veces: rápido
   { "type": "preprocessed", "preprocessed_player": "<cache>", "requests": [ … ] }
   // → { "type": "result", "preprocessed_player": "…", "responses": [{ "type": "result", "data": { "<n1>": "<resuelto>" } }] }
   ```
3. **Caché por versión del reproductor**: el `preprocessed_player` se guarda en
   `~/.cache/media-lite/yt-player/<player_id>.js`. YouTube cambia `base.js` cada algunos días.
   Solo la primera reproducción después de un cambio paga el costo de analizarlo, y la interfaz
   lo avisa con "Preparando YouTube…". También se cachean los `n` ya resueltos (`n → resultado`)
   mientras siga vigente la misma versión del reproductor.
4. **Se ejecuta en un proceso aparte** (`media-lite --yt-solver`, el mismo binario): recibe JSON
   por stdin y devuelve JSON por stdout. Analizar `base.js` puede usar más de 100 MB por un rato, y
   cuando el proceso termina esa memoria vuelve entera al sistema. Si el solucionador se cuelga o
   falla, la app sigue funcionando. Tiene un tiempo límite configurable (60 s por defecto).
5. **Actualizable sin recompilar**: el AppImage trae una versión de los scripts de `ejs`. En Ajustes,
   la opción «Actualizar el soporte de YouTube» baja la última versión publicada en GitHub (como
   `--remote-components ejs:github` de yt-dlp), verifica el hash y la guarda en
   `~/.local/share/media-lite/ejs/`. Como el JS corre en una caja sin red ni disco, ejecutar código
   bajado no expone el sistema.
6. **Se sigue probando primero el cliente sin JS** (VisionOS, igual que hoy en Sanson) porque es
   el más rápido. Los demás clientes son el respaldo, en el orden que diga `clients.toml`.
7. **Formatos**: se prefiere el HLS de `web_safari`/`visionos` con variantes **H.264**, porque mpv
   lo abre directo y el CPU lo aguanta. Los formatos DASH separados (video `avc1` + audio `mp4a`) se
   pasan a mpv con `--audio-file`.
8. **yt-dlp queda como último recurso** solo si el usuario ya lo tiene instalado **con** un motor
   JS. La versión de yt-dlp en los repositorios de Debian/antiX suele quedar vieja, así que nunca es
   el camino principal.
9. **YouTube Music** usa la misma cascada en modo solo audio. El respaldo en SoundCloud que ya tenía Sanson se mantiene.

### 2.5 Concurrencia y red (pensado para 2 núcleos)

- **Sin tokio.** Un pool fijo de 3 hilos (2 de red y 1 de imágenes) con `crossbeam-channel`. Los
  resultados vuelven a la interfaz con `slint::invoke_from_event_loop`. Es el mismo modelo que
  `backend.cpp`, pero con tipos.
- HTTP: **`ureq`** (bloqueante, chico) con **`rustls`** + certificados del sistema. **No usar
  OpenSSL**: Debian 11 trae libssl1.1 y Debian 12 trae libssl3, y un AppImage enlazado a una no
  arranca en la otra.
- WebSocket del chat: **`tungstenite`** (síncrono, en un hilo por chat abierto).
- Cancelación: cada pedido lleva un número de generación. Si cambiaste de pantalla, el resultado viejo se descarta.

### 2.6 Miniaturas (el mayor riesgo de RAM)

- `image` con solo `jpeg`, `png` y `webp`. Cada imagen **se achica al tamaño de la tarjeta apenas
  se decodifica** (p. ej. 320×180). Así queda en ~230 KB en RAM, y no los 8 MB de una de 1080p.
- Caché en memoria LRU acotada (**24 MB** por defecto) y caché en disco en
  `~/.cache/media-lite/thumbs` (con límite de tamaño, se borran primero las más viejas).
- Las grillas usan `ListView` virtualizado de Slint: solo existen las tarjetas visibles.
- Los emotes animados del chat se muestran estáticos (primer cuadro).

### 2.7 Datos en disco

- `~/.config/media-lite/`: `settings.toml`, `favorites.json`, `following.json`, `history.json`
  (máximo 500 entradas), `music-queue.json`, `cookies/` (permisos 0600).
- `~/.cache/media-lite/`: miniaturas y portadas.
- Respeta `XDG_CONFIG_HOME` / `XDG_CACHE_HOME`, como ya hacía `store.cpp`.
- `serde` + `serde_json`/`toml`. No hace falta SQLite.

### 2.8 Compilación pensada para este CPU

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
```

- **No usar `-C target-cpu=native` ni `x86-64-v2`**: el T4500 no tiene SSE4.1/4.2 ni POPCNT y
  el binario se cerraría con "illegal instruction". Se compila para `x86_64-unknown-linux-gnu`
  base (x86-64 v1).
- Se compila dentro de un contenedor **Debian 11 (bullseye, glibc 2.31)** para que ande en
  antiX 21/23, Debian 11/12/13, Ubuntu 20.04+, MX, etc.

### 2.9 Estructura del repositorio

```
media-lite/
├── Cargo.toml                  # workspace
├── crates/
│   ├── core/                   # modelo, store (config/historial/favoritos), caché, pool de hilos, http
│   ├── providers/              # pluto, runtime, plex, youtube, ytmusic, twitch, kick, soundcloud, lrclib
│   │   └── clients.toml        # constantes de innertube que cambian seguido
│   ├── player/                 # cliente IPC de mpv + proxy que reescribe m3u8
│   ├── ytsolver/               # QuickJS-NG (rquickjs) + scripts de yt-dlp/ejs, caché por player_id
│   ├── chat/                   # twitch IRC (ws), kick (ws), youtube live chat (polling), repeticiones
│   └── cli/                    # media-lite-cli: probar proveedores sin interfaz (como lumiere-cli)
├── app/
│   ├── src/                    # main.rs, controladores de pantallas, puente Slint <-> core
│   └── ui/                     # *.slint: tema, tarjetas, grilla, barra lateral, reproductor, ajustes
├── packaging/
│   ├── AppRun, media-lite.desktop, media-lite.png
│   ├── input.conf, mpv.conf
│   ├── ejs/                    # yt.solver.lib + yt.solver.core (versión fijada, con hash y licencias)
│   └── build-appimage.sh
└── docs/
```

---

## 3. Fases

Cada fase termina con algo que se puede usar. Las fases 3 a 6 se pueden hacer en otro orden.

### Fase 0: Validar el hardware (1–2 días)

Antes de escribir la app, medir en la laptop.

- [ ] (pendiente, laptop) `lspci | grep VGA`, `glxinfo -B`, `vainfo`: anotar GPU, versión de GL y si VA-API decodifica H.264.
- [ ] (pendiente, laptop) Probar mpv con un video de YouTube a 360p/480p/720p en H.264
      (`yt-dlp -f "bv[vcodec^=avc1][height<=480]+ba[acodec^=mp4a]"` y pasarle las URLs a mpv).
      Medir CPU y RAM con `--vo=gpu`, con `--vo=xv`, con `--hwdec=no` y con `--hwdec=vaapi`.
- [ ] (pendiente, laptop) Probar un directo HLS de Twitch y un canal de Pluto TV en mpv (las URLs se sacan con las herramientas CLI
      de las referencias si hace falta).
- [ ] (pendiente, laptop) Compilar un ejemplo de Slint (grilla con 200 tarjetas e imágenes) con `winit-software` y
      `winit-femtovg`. Medir RAM y fluidez al hacer scroll.
- [x] **Solucionador de YouTube**: prototipo con `rquickjs` que carga los
      scripts de `yt-dlp/ejs` y resuelve desafíos `n` de un `base.js` real (ver `docs/hardware.md`):
      (a) QuickJS-NG 0.16.2 (≥ 0.12), (b) primera vez: 7,4 s y 210 MB en el entorno de desarrollo,
      (c) con preprocesado: 0,45 s y 76 MB. Pendiente: medir en la T4500.
- [ ] (pendiente, red residencial) Comprobar que los clientes `visionos` y `web_safari` entregan HLS con variantes H.264 para videos comunes y directos. Desde el entorno de desarrollo YouTube bloquea todos los clientes, así que no es concluyente.
- [x] Anotar los resultados en `docs/hardware.md`. Lo medido en la laptop se completa después; mientras tanto rigen los valores conservadores de ese archivo.

**Terminado cuando** sepamos qué combinación de `vo`/`hwdec`/calidad se ve fluida, qué
renderizador de Slint usar y que el solucionador de YouTube anda en tiempos aceptables.

### Fase 1: Esqueleto (1 semana)

- [x] Workspace de Cargo, perfiles de release, `rustfmt`/`clippy` (`-D warnings`), CI en GitHub Actions (`.github/workflows/ci.yml`).
- [x] `core`: modelo de datos, `store` (settings en TOML, listas en JSON, escritura atómica), pool de hilos con generaciones, cliente HTTP con UA y timeouts (ureq + rustls).
- [x] `core`: caché de miniaturas (LRU en memoria con tope en bytes, disco con poda por antigüedad, reducción al tamaño de la tarjeta).
- [x] `app`: ventana Slint con tema oscuro/claro, barra lateral, buscador, grilla virtualizada (`ListView`), pantalla de detalle vacía, Inter embebida.
- [x] Colores de acento por plataforma: el acento es uno solo por ahora; por plataforma queda para la Fase 7.
- [x] La ventana recuerda tamaño y posición. Atajos: `/` buscar, Esc/Retroceso volver, F5 recargar.
- [x] `cli`: `media-lite-cli platforms|list|search|details` imprime JSON.
- [x] Un proveedor de prueba con datos fijos y miniaturas PNG reales, para desarrollar la interfaz sin red.

**Medido:** la grilla con 60 miniaturas en una ventana de 1100×720, binario de release: **41,7 MB de RAM** (objetivo: < 40 MB, **no cumplido todavía**). Sin miniaturas el app usa 23,2 MB. El costo está en las copias de cada imagen dentro del renderer de Slint (~300 KB por miniatura de 256×144). Bajar a 224×126 solo ahorra 1,2 MB, así que no se hizo. La solución de fondo (cargar miniaturas solo para las filas visibles) quedó hecha en la Fase 7: falta volver a medir en release.

Otras medidas: binario de release de 18,4 MB con Slint (software y femtovg). Femtovg aporta 0,65 MB; se mantiene para poder elegirlo en Ajustes.

**Terminado cuando** se pueda navegar una grilla falsa con miniaturas reales, con menos de 40 MB de RAM. → Navegación: sí. RAM: 41,7 MB (ver arriba).

### Fase 2: Reproductor (1 semana)

- [x] `player`: lanza mpv, se conecta por IPC y observa `time-pos`, `duration`, `pause`, `volume`, `track-list`, `core-idle`, `paused-for-cache` y `fullscreen`. (`demuxer-cache-state` queda para más adelante: `paused-for-cache` alcanza para detectar trabas.)
- [x] Modo **ventana flotante** (PiP: `--ontop --no-border --geometry=480x270-16-16`). El modo solo audio existe en `player` (`Mode::AudioOnly`) y se usa en la Fase 6.
- [~] Barra de controles: pausa, barra de avance (un clic salta), ±10 s, pantalla completa, **Idioma** (cicla pistas de audio) y **Subtítulos** hechos. Pendiente: control de volumen en la barra y selector de calidad durante la reproducción (la calidad hoy se elige en Ajustes).
- [~] Teclas: espacio (pausa), flechas (±10 s) desde la app, y las de mpv por defecto en la ventana de video. Falta un `input.conf` propio con la tabla de mplayer (teclas `9/0`, `w/e/d/D`, `s/S`…): va con el empaquetado (Fase 9).
- [x] Retomar donde quedó: posición guardada cada 10 s y al cerrar, en `history.json`; al volver a abrir el video se usa `start=`.
- [ ] Reproducción continua con cuenta regresiva de 5 s y sin mezclar shorts con videos. **Movido a la Fase 3**: necesita una lista de "siguientes" (episodios, canales, videos) que recién existe con los proveedores de TV.
- [x] Si la imagen se traba más de 20 s, mpv se vuelve a cargar en la misma posición con una calidad menor (1080 → 720 → 480 → 360 → 240), y como mucho cada 30 s. Implementado; falta probarlo con una red lenta de verdad.
- [x] Proxy de playlists `.m3u8` en `127.0.0.1`, con puerto libre y token aleatorio en la ruta. Filtra variantes por altura (la calidad elegida) y por códec (se descartan AV1, VP9 y HEVC, que el T4500 no decodifica), y quita segmentos que coincidan con patrones de anuncios. Los filtros concretos de Pluto y Twitch llegan en las Fases 3 y 5.

**Medido (entorno de desarrollo, no la T4500):**
- Prueba de punta a punta con mpv sin pantalla (`vo=null`) y el HLS público de prueba a través del proxy: `time-pos` avanza. Prueba en `crates/player/tests/playback.rs` (`cargo test -- --ignored`, necesita red).
- Con el app abriendo el primer video: mpv con variantes de 1080p usaba **468 MB**. Con el límite de 480p bajó a **194 MB**. El objetivo de app + mpv a 480p es < 220 MB; con el app en release (~40 MB) quedan ~235 MB, así que hay que bajarlo: pendiente de medir en la T4500 y de probar `--vd-lavc-threads=1` y `--cache-secs`.

**Terminado cuando** una URL HLS y un par video+audio separados se puedan ver en ventana flotante, controlados desde la app. → HLS: sí. Par video+audio separado (`audio-add`): implementado, sin prueba todavía.

### Fase 3: TV gratis, migrar Lumière (1–2 semanas)

Portar `pluto.cpp`, `runtime.cpp`, `plex.cpp` y `providers.cpp`:

- [x] **Pluto TV** (`crates/providers/src/pluto.rs`): sesión de `boot.pluto.tv` con renovación (y reintento ante 401), 192 canales con categorías y «Ahora:» de la guía, 1 813 películas y 1 563 series por categoría, búsqueda, temporadas y episodios, y el stream del stitcher. Los anuncios se quitan con los patrones de Lumière (`_ad%2F`, `%2Fad%2F`, `/ad/creative`, `ad%2Fcreative`) a través del proxy. Probado contra la API real desde el CLI y desde la app.
- [x] **Runtime** (`runtime.rs`): canales, películas y series desde `getconfiguration`; búsqueda; temporadas; MP4 de Kaltura para películas y HLS para canales. Probado contra la API real.
- [x] **Plex** (`plex.rs`): cuenta anónima con token guardado en `plex.json` (0600); 90 canales (en español por defecto), filas de películas y series en español, búsqueda y temporadas. Probado contra la API real. La opción de incluir inglés se toma de Ajustes al arrancar (cambiarla requiere reiniciar).
- [x] Ocultar todo lo que tiene DRM o pide suscripción (igual que Lumière).
- [~] Panel lateral al reproducir: la grilla sigue visible con los demás canales, episodios o películas. **Falta la guía de programación (EPG)** de Pluto y Plex en el panel.
- [x] Anterior/siguiente para cambiar de canal o episodio (botones y teclas `<` `>`). El idioma de audio elegido sigue en el próximo episodio.
- [x] **Reproducción continua** (movido desde la Fase 2): al terminar, cuenta 5 s y pasa al siguiente de la cola. Implementado; falta probarlo con un video que termine.
- [x] Traducir géneros (`genre_es`) y nombres de idiomas (`lang_name`).
- [x] Filtros enchufables en el proxy: el proveedor pone sus patrones en `Source.drop`, y la calidad se limita por altura y por códec (se estima la altura por el ancho de banda cuando falta `RESOLUTION`, como en Lumière).

Además, en esta fase: vista de **series** (chips de temporadas y los episodios en la grilla), chips de **categorías**, y dos pools (dos hilos de red y uno de imágenes) para que un pedido del usuario no espere a las miniaturas.

**Pendiente de esta fase:** la guía (EPG) en el panel, `SP_RETIME` (primero probar si mpv maneja bien las discontinuidades de Pluto), y la pantalla de Ajustes (llega con la Fase 7).

**Terminado cuando** todo lo que muestran las capturas de `lumiere/capturas/` funcione en media-lite. → Inicio, búsqueda, películas, series con episodios, canales de Runtime y de Plex, y subtítulos: sí. Guía de canales y Ajustes: no todavía.

### Fase 4: YouTube (3 semanas)

Portar `plugins/youtube/script.js` y `comun.js`, y sumar el solucionador de desafíos (sección 2.4).

**4a. Solucionador (`crates/ytsolver`), 1 semana** — hecho en lo que se puede verificar sin YouTube

- [ ] Obtener la URL y el `player_id` de `base.js` (de la página del video): pendiente, va con el proveedor de YouTube (4b/4c).
- [x] Subcomando `media-lite --yt-solver`: lee el JSON de ejs por stdin y responde por stdout, en un proceso aparte. QuickJS-NG (`rquickjs` 0.14) con límite de memoria de 256 MB; el padre mata el proceso a los 60 s.
- [x] Caché por `player_id` del `preprocessed_player` y de los `n` y `sig` ya resueltos. Los ids se validan (8 hex) para no escribir fuera de la carpeta. La poda de 14 días está escrita (`Cache::prune`); falta llamarla al arrancar.
- [ ] Preprocesar en segundo plano al encontrar un `base.js` nuevo: pendiente, depende de saber la versión del reproductor (4c).
- [x] Actualizar los scripts desde GitHub con verificación: `update::fetch_latest` baja el último release y compara cada archivo con el digest sha256 que publica la API (verificado en vivo). La copia instalada en `~/.local/share/media-lite/ejs/` se usa solo si su manifiesto y sus hashes coinciden. **Falta el botón en Ajustes** (Fase 7).
- [x] Pruebas con un `base.js` real: `solves_n_challenges_like_node` (prueba ignorada, con `MEDIA_LITE_TEST_BASEJS`) da los mismos resultados que Node. Prueba de punta a punta del proceso aparte: pendiente de correrla con el binario.

**4b. Cascada de clientes**

- [ ] Leer de `clients.toml` el orden de los clientes y sus datos: `visionos` → `web_safari` (HLS + `n`) →
      `web_embedded`/`tv` (+ `n`/`sig`) → `yt-dlp` externo si está.
- [ ] Descartar los formatos SABR, los que tienen DRM y los que piden PO token. **Elegir H.264 + AAC**
      hasta la calidad configurada (480p por defecto, 360p si la ventana es chica), igual que Sanson.
- [ ] Recordar qué cliente funcionó la última vez y empezar por ese durante la sesión.
- [ ] Si mpv recibe 403 o la URL vence (~6 h, o cambió la IP), resolverla de nuevo y retomar en la misma posición.
- [ ] Mensajes claros: "YouTube cambió y hay que actualizar el soporte (Ajustes)", "Este video pide iniciar sesión".

**4c. Catálogo e interfaz**

- [ ] Cliente innertube WEB (portada por temas, búsqueda de videos y canales, canal con pestañas
      Videos/En vivo/Shorts, relacionados, `visitorData` guardado 3 días). Estos pedidos no piden ni JS ni token.
- [ ] Directos de YouTube (HLS, sin PO token).
- [ ] Botón Idioma: audio original por defecto (no el doblaje automático) y subtítulos, incluidos los automáticos.
- [ ] Comentarios y chat en vivo (`live_chat/get_live_chat` por polling).
- [ ] "Siguiendo" sin cuenta a través del RSS de cada canal.
- [ ] Botones de filtro Videos / En vivo / Shorts cuando una lista mezcla tipos.

### Fase 5: Twitch y Kick (1–2 semanas)

Portar `plugins/twitch`, `plugins/kick`, `ws.c` y la parte de chat de `app_watch.cpp`:

- [ ] Twitch: GQL (portada, búsqueda, canal, VODs), token de acceso + `usher.ttvnw.net`, salto de anuncios
      en el proxy ("Saltando el anuncio de Twitch…" mientras dura).
- [ ] Kick: API (portada, búsqueda, canal, directos y VODs).
- [ ] Chat en vivo de Twitch (IRC sobre WebSocket) y de Kick (su WebSocket), con emotes
      (caché de imágenes compartida y límite de ~300 mensajes en memoria).
- [ ] Repetición del chat sincronizada con `time-pos` en VODs de Twitch y Kick.

### Fase 6: Música (1–2 semanas)

Portar `plugins/music` y `app_music.cpp`:

- [ ] YouTube Music: canciones, álbumes y artistas (seguir artistas), solo audio. Usa la misma cascada de clientes y el mismo solucionador de la Fase 4.
- [ ] **Barra de reproducción persistente** abajo: la música sigue sonando mientras navegás, y el botón
      «Sonando» lleva a la pantalla de la canción.
- [ ] Portada grande + **letra sincronizada** (`lrclib.net`): la línea actual resaltada, y un clic salta a ese momento.
- [ ] Cola editable (reproducir a continuación, agregar, subir/bajar/quitar/vaciar), guardada en disco.
- [ ] Aleatorio, repetir todo/una, **radio** al terminar la cola.
- [ ] Respaldo en SoundCloud si YouTube no entrega el audio.
- [ ] Opcional: teclas multimedia del teclado (XF86AudioPlay/Next/Prev) con un atajo global X11.

### Fase 7: Unificación (1 semana)

Es lo que hace que se sienta **una sola aplicación** y no dos pegadas.

- [~] **Barra lateral única**:
  - [x] Biblioteca: Inicio («Seguir viendo» y favoritos), Historial y Favoritos. Se abren sin red.
  - [x] Ajustes: pantalla propia (calidad, salida de video, decodificación, video liviano, renderizador, Plex con inglés, caché de miniaturas, tema).
  - [ ] En vivo, películas y series **de todas las plataformas juntas**: hoy cada plataforma se elige por separado.
  - [ ] Siguiendo: canales de Twitch, Kick y YouTube (llega con esas fases).
- [x] **Búsqueda global**: la última entrada de la lista de plataformas, «Todas las plataformas», consulta a los tres proveedores a la vez, en el pool de red. Los resultados se agregan a la grilla al llegar, con el nombre de la plataforma en el subtítulo. Probado en vivo: «prueba» devolvió 99 resultados de Pluto TV y de Runtime. Falta agruparlos en filas por plataforma con «ver más».
- [x] Historial y "Seguir viendo" con la posición de cada título (el historial guarda el elemento completo, para volver a abrirlo).
- [ ] Ajustes unificados: falta el botón «Actualizar el soporte de YouTube» (llega con la Fase 4) y el selector de plataformas visibles.

**Terminado cuando** se pueda usar sin pensar en qué plataforma es cada cosa. → Todavía no: falta la búsqueda global y el listado unificado.

### Fase 8: Cuentas (opcional, 1 semana)

- [ ] Iniciar sesión **pegando cookies** (formato Netscape o encabezado `Cookie:`), como ya permite Sanson.
- [ ] Importar cookies de Firefox/Chromium/Pale Moon instalados en la distro (leer `cookies.sqlite`
      con `rusqlite` con la opción `bundled`, solo si se usa esta función).
- [ ] Con sesión: recomendaciones de la cuenta, canales que seguís en la plataforma, escribir en el
      chat (Twitch/Kick/YouTube), comentar en YouTube (`SAPISIDHASH`).
- [ ] Las cookies se guardan con permisos 0600 y nunca se escriben en los logs.

### Fase 9: Empaquetado AppImage (3–4 días)

- [x] `packaging/build-appimage.sh`: corre dentro de `debian:bullseye` (glibc 2.31) con Docker. Compila con `--locked`, verifica con `objdump` que el binario pide glibc ≤ 2.31 (mide **GLIBC_2.30**), arma el `AppDir` y empaqueta con `appimagetool` y el runtime estático `type2-runtime`. **Debian 11 ya no tiene soporte**: el script apunta apt a `archive.debian.org`, solo dentro del contenedor.
- [x] AppDir: `usr/bin/media-lite`, `.desktop`, ícono 256×256 (generado en `packaging/media-lite.png`), `AppRun`, y `usr/share/doc/media-lite/` con la GPL y la licencia OFL de Inter.
- [x] **Resultado: `media-lite-x86_64.AppImage` de 9,3 MB.** Se ejecuta desde el archivo y abre la ventana. Sus dependencias del sistema son solo libc, libm, libgcc_s, libdl y libpthread: X11, fontconfig y GL se cargan con `dlopen`. mpv no va adentro (se pide con `sudo apt install mpv`).
- [x] Si falta mpv, el app muestra el comando para instalarlo. El README explica `--appimage-extract-and-run` si falta FUSE.
- [x] **Automatización**: `.github/workflows/appimage.yml` corre en cada push a `main` y publica el release `v<versión de Cargo.toml>` con el AppImage y su `.sha256`.
- [ ] Integración con el menú (copiar el `.desktop` a `~/.local/share/applications`): pendiente.
- [ ] Actualizaciones con zsync: pendiente.
- [ ] Probar en antiX 23, Debian 11, Debian 12, MX y Ubuntu 22.04: **pendiente, requiere las máquinas**.
- [ ] Licencias de terceros de los crates (con `cargo-about`): pendiente. Las de `ejs` y `rquickjs` llegan con la Fase 4.

### Fase 10: Pulido y rendimiento (continuo)

- [~] Perfil **"equipo lento"** de un clic: falta. Hoy se configura a mano en Ajustes (salida `xv`, video liviano, calidad 360p).
- [x] Miniaturas solo de las filas visibles, con dos filas de margen, y liberación de las que quedan lejos. Pendiente medir la RAM en release con 60 elementos (objetivo < 40 MB).
- [ ] Medir con `heaptrack` y `/proc/<pid>/status` (VmRSS) en cada versión, con un tope en la CI.
- [ ] Mensajes de error en español y comprensibles: en gran parte ya están (los errores de los proveedores se muestran en la barra).
- [ ] Capturas y README: README hecho; capturas nuevas pendientes.

## 4. De qué se libra cada pieza vieja

| Pieza en las referencias | Qué pasa en media-lite |
|---|---|
| FLTK (`ui.cpp`, `views.cpp`) | Slint (`app/ui/*.slint`) |
| mplayer + `player.cpp` | mpv + cliente IPC (`crates/player`) |
| `stream.c` (proxy completo) | Proxy que solo reescribe `.m3u8` |
| `aes.c` | Se elimina (ffmpeg) |
| cJSON | `serde_json` |
| QuickJS + `engine.c` + `sanson_host.js` | **QuickJS-NG se queda, pero con otro rol**: no corre plataformas, solo el solucionador oficial de `yt-dlp/ejs`, en un proceso aparte y sin red (`crates/ytsolver`). Se elimina `sanson_host.js` |
| `plugins/*.js` | `crates/providers/*` (lógica portada uno a uno) |
| `ws.c` | `tungstenite` |
| `hash.c` (MD5/SHA1 para SAPISIDHASH, etc.) | crates `md-5` / `sha1` |
| `cookies.cpp` (lee `cookies.sqlite` con la lib de Pale Moon) | Parser de cookies + `rusqlite` opcional |
| `store.cpp` (×2) | `crates/core/store` (un solo store) |
| `lumiere-cli`, `sanson-cli`, `sp-test` | `media-lite-cli` |
| `make_tcz.sh` | `packaging/build-appimage.sh` |

## 5. Crates previstos

| Crate | Para qué | Por qué este |
|---|---|---|
| `slint` (`backend-winit`, `renderer-software`, `renderer-femtovg`) | Interfaz | Liviana, moderna, funciona sin GPU |
| `ureq` + `rustls` + `rustls-native-certs` | HTTP | Bloqueante y chico, sin OpenSSL |
| `serde`, `serde_json`, `toml` | Datos | Estándar |
| `crossbeam-channel` | Pool de hilos | Simple, sin runtime async |
| `image` (jpeg, png, webp) | Miniaturas | Se activan solo los formatos necesarios |
| `lru` | Caché en memoria | |
| `tungstenite` (rustls) | Chat | Síncrono |
| `rquickjs` (QuickJS-NG) | Desafíos `n`/`sig` de YouTube | MIT, ~1,5 MB, el motor más rápido entre los livianos; `rquickjs-sys` 0.14.0 trae QuickJS-NG 0.16.2 |
| `sha2` | Verificar los scripts `ejs` bajados | |
| `x11rb` | Ventana hija para mpv integrado, teclas multimedia | Puro Rust |
| `md-5`, `sha1`, `base64`, `url`, `percent-encoding` | Utilidades | |
| `directories` | Rutas XDG | |
| `log` + `env_logger` | Depuración (`MEDIA_LITE_DEBUG=1`) | |
| `rusqlite` (`bundled`, opcional) | Importar cookies del navegador | Detrás de una *feature* de Cargo |

## 6. Riesgos y cómo mitigarlos

| Riesgo | Mitigación |
|---|---|
| YouTube cambia el reproductor (`base.js`) y el solucionador deja de andar | Scripts `ejs` actualizables desde Ajustes sin recompilar; usar los mismos que yt-dlp, que se arreglan en horas o días |
| Se cierran los clientes sin PO token (como pasó con `android_vr`) | Cascada en `clients.toml` reemplazable; probar todas las noches en la CI con `media-lite-cli`; respaldo con `yt-dlp` externo. **Generar PO tokens queda fuera de alcance** (necesita un navegador) |
| YouTube obliga a usar SABR en todos los clientes | mpv no lo reproduce. Último recurso: un mini-proxy SABR → HLS local siguiendo la librería [`googlevideo`](https://ytjs.dev/googlevideo/). Es mucho trabajo; hacerlo solo si llega a ser necesario |
| Preprocesar `base.js` es demasiado lento en este CPU | Caché por versión, preprocesado en segundo plano, y la Fase 0 mide antes de comprometerse |
| La GPU no da con `--vo=gpu` | `--vo=xv` (lo que ya usaban las referencias) |
| VA-API no anda en esta GPU | Decodificación por software de H.264 ≤ 480p, que este CPU aguanta |
| Slint por software se traba al hacer scroll | Tarjetas de tamaño fijo, `ListView` virtualizado, sin sombras ni desenfoques, probar femtovg |
| mpv integrado (`--wid`) con problemas de foco o teclado | Arrancar con la ventana flotante, que es más simple; el modo integrado viene después |
| glibc demasiado nueva en el binario | Compilar en Debian 11 y verificar con `objdump -T` el símbolo GLIBC más alto |
| Una plataforma bloquea por región o VPN | Igual que hoy: avisarlo en el README y en el error |
| Términos de uso de las plataformas | Solo contenido gratuito y sin DRM, sin descargas; dejarlo claro en el README |

## 7. Orden sugerido y tiempos

```
Semana 1      Fase 0 + Fase 1
Semana 2      Fase 2 (ya se pueden ver URLs en ventana flotante)
Semanas 3–4   Fase 3 (TV gratis: primera versión usable)       → v0.1 AppImage (adelantar lo mínimo de la Fase 9)
Semanas 5–7   Fase 4 (YouTube, con el solucionador JS)          → v0.2
Semanas 8–9   Fase 5 (Twitch/Kick)                              → v0.3
Semanas 10–11 Fase 6 (Música)                                   → v0.4
Semana 12     Fase 7 (unificación)                              → v0.5
Semana 13     Fase 8 (cuentas, opcional) + Fase 9 completa     → v1.0
```

Consejo: **armar el AppImage desde la v0.1** (aunque sea a mano) y usarlo a diario en la laptop.
Los problemas de RAM y CPU aparecen ahí, no en la máquina de desarrollo.

## 8. Fuentes (consultadas el 1 de noviembre de 2026)

- [yt-dlp #15012: se necesita un motor JavaScript externo para tener soporte completo de YouTube](https://github.com/yt-dlp/yt-dlp/issues/15012) (noviembre de 2025)
- [yt-dlp wiki: EJS](https://github.com/yt-dlp/yt-dlp/wiki/EJS): motores admitidos y advertencia de lentitud de QuickJS-NG anterior a 0.12
- [yt-dlp/ejs](https://github.com/yt-dlp/ejs) y su [`main.ts`](https://github.com/yt-dlp/ejs/blob/main/src/yt/solver/main.ts): formato de entrada y salida, y `preprocessed_player`
- [yt-dlp wiki: guía de PO tokens](https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide): qué cliente necesita token; HLS de `web_safari` y directos
- [yt-dlp #17348: `android_vr` ahora necesita PO token](https://github.com/yt-dlp/yt-dlp/issues/17348) (2026)
- [CDisc-Music wiki: YouTube](https://github.com/valkdzx/CDisc-Music/wiki/YouTube): VisionOS como primer intento sin claves, y URLs atadas a la IP
- [Pedido de VRChat de actualizar a yt-dlp 2026.08.19](https://feedback.vrchat.com/feature-requests/p/update-to-yt-dlp-20260819): `visionos` como cliente por defecto y sus problemas
- [ytdlp-ejs (crate)](https://docs.rs/crate/ytdlp-ejs/0.1.1): versión en Rust de `ejs` (con SWC). Sirve de referencia, pero es muy nueva y pesa más
- [rquickjs](https://github.com/DelSkayn/rquickjs): bindings de QuickJS-NG para Rust (MIT)
- [googlevideo (SABR)](https://ytjs.dev/googlevideo/): referencia por si alguna vez hace falta implementar SABR
- [yt-dlp/ejs 0.8.0](https://github.com/yt-dlp/ejs/releases/tag/0.8.0): scripts `yt.solver.{lib,core}.min.js` usados en el prototipo de la Fase 0
- [yt-dlp: constantes de clientes (`_base.py`)](https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/youtube/_base.py): versiones de `visionos`, `web_safari`, `web_embedded` y `tv`
