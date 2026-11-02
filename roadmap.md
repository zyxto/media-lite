# Roadmap: media-lite

Una sola aplicación en Rust que junta **Lumière** (Pluto TV, Runtime, Plex) y **Sanson**
(YouTube, Twitch, Kick, YouTube Music) y se distribuye como **AppImage** para antiX y cualquier
Debian-like de 64 bits.

**Equipo objetivo:** Pentium T4500 (2 núcleos, 2,3 GHz, SSE3/SSSE3, **sin SSE4**), 2 GB de RAM,
gráficos Intel integrados (probablemente GMA 4500M: OpenGL 2.1, sin Vulkan), antiX (X11, sin
systemd).

---

## Estado (2 de noviembre de 2026)

- **Publicado:** `v0.4.0` (pantalla de título, arreglos del reproductor). `v0.1.0` tiene el primer AppImage con TV gratis.
- **Hecho:** Fases 1, 2, 3 y 9 (con pendientes), 4a (solucionador), y parte de la 7 (Ajustes, biblioteca, búsqueda global, miniaturas solo de lo visible).
- **Sin hacer:** YouTube (4b y 4c: streams y catálogo), Twitch y Kick (5), música (6), cuentas (8), y lo que quedó pendiente en cada fase.
- **Pendiente de la laptop:** mediciones de la Fase 0, prueba de las distros de la Fase 9, y medir la RAM en release con 60 elementos.

**Qué frenaba YouTube y cuál es el camino (decidido el 2 de noviembre de 2026):**

- El «confirmá que no sos un bot» depende de la **IP**, no del cliente. Lo dicen los mantenedores de yt-dlp y de Invidious, y sale con VPN o con IP de datacenter. La prueba anterior se hizo desde un entorno bloqueado. La red de la laptop hoy es residencial (Telmex, AR) y no hay evidencia de que esté bloqueada.
- El bloqueo que quedaba no era de YouTube: **el clasificador de permisos de Claude Code (modo auto)** rechaza que el agente haga pedidos de prueba a `youtubei/v1/player` y los marca como «Bot Check Circumvention». Ver «Pruebas contra YouTube» en la Fase 4.
- **Probado en la laptop el 2 de noviembre de 2026** (antiX, yt-dlp 2026.10.21, sin VPN): **no hubo aviso de bot**. `visionos` entregó todo sin JS: HLS `avc1` de 144p a 1080p con audio aparte (233/234), y DASH directo `avc1` 134/135/136 + `mp4a` 139/140. `tv_samsung` respondió, pero yt-dlp no tenía motor JS para sus `sig`/`n`. Además **trajo un anuncio previo de 6 s**: YouTube no sirve el video hasta que pasa ese tiempo.
- **Camino elegido:** **`visionos` primero** (sin JS, sin espera). Si falla, `tv_samsung` con el solucionador que ya está hecho, respetando la espera del anuncio. Más `visitorData` y caché del reproductor para abrir rápido. Ningún cliente necesita PO token. Si YouTube igual pide confirmar, se usan las cookies del usuario (adelantar la Fase 8). No se esquiva el control: nada de rotar IPs, ni proxies, ni generar PO tokens. Detalle en 2.4.
- **Twitch y Kick no dependen de YouTube.** No tienen ese bloqueo y se pueden hacer ya. Conviene hacer la Fase 5 **antes** que la 4b.

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
- **Clientes de reproducción (estado de yt-dlp `master` al 26 de octubre de 2026).** Sus clientes
  por defecto son `tv_samsung`, `visionos` y `web`. Sin motor JS usa solo `visionos`.

  | Cliente | ¿JS? | ¿PO token? | Estado | En media-lite |
  |---|---|---|---|---|
  | `visionos` | No | No | Anda (probado en la laptop). No sirve para videos «para niños» | **1.º** |
  | `tv_samsung` | Sí: `sig` + `n` + `signatureTimestamp` | No | Por defecto en yt-dlp desde el 10/10/2026 (#17805), anda sin cookies. **Puede traer un anuncio previo y hay que esperar lo que dura** | **2.º** |
  | `web_embedded` | Sí | No | Solo videos que se pueden insertar | 3.º |
  | `web_safari` | Sí | No para HLS | **Desde 2026.07 solo da HLS con sesión iniciada o «confiable»** | Solo con cookies |
  | `tv` (Cobalt 7.x) | Sí | No | Todo viene con DRM si no se pasan cookies | Solo con cookies |
  | `android_vr` | No | — | **403 en todos los formatos desde el 17/08/2026** (#17461) | Descartado |
  | `web` | Sí | Sí | Solo SABR | Descartado |
  | `ios`, `android`, `mweb` | — | Sí | Piden token | Descartados |

  Los **directos en HLS** no piden token en ningún cliente salvo `ios`.
- **«Confirmá que no sos un bot» (`playabilityStatus.status = LOGIN_REQUIRED`).** Según los
  mantenedores de yt-dlp y de Invidious, significa que YouTube **no confía en la IP** (VPN,
  datacenter o IP marcada). Ningún cliente lo saltea. Lo que sí ayuda es lo mismo que hace un
  usuario normal:
  1. Mandar `visitorData` en el contexto. Sanson lo pide a `youtubei/v1/visitor_id` antes del
     primer `/player` y lo guarda 3 días. Su código dice que sin eso aparece el aviso.
  2. No usar VPN. La URL igual queda atada a la IP que la pidió.
  3. Si sigue apareciendo, usar **las cookies de la cuenta del usuario** (como `--cookies` de yt-dlp
     y como hace RustyPipe cuando detecta la IP bloqueada). Por eso las cookies de la Fase 8 se
     adelantan a la Fase 4b.

  **media-lite no esquiva este control.** No rota IPs, no usa proxies ni Tor, no genera PO tokens
  y no imita huellas TLS. Si el aviso persiste, explica la causa y ofrece cargar cookies.
- **PO tokens:** son tokens antibots que se generan con BotGuard, algo que en la práctica necesita un
  navegador. Generarlos en este equipo es inviable, así que **media-lite usa solo clientes que no
  piden PO token** (ver la tabla).
- **SABR** es el protocolo propio de YouTube que entrega el video en pedazos según el ancho de banda.
  mpv/ffmpeg **no lo reproducen**, así que se descartan los clientes que solo entregan SABR (`web`
  sin token).
- Las URLs de los streams **vencen a las ~6 h** y **quedan atadas a la IP** que las pidió. Por eso se
  resuelven justo antes de reproducir y se vuelven a pedir si mpv recibe un 403. Si cambiás de VPN a
  mitad de un video, también hay que pedirlas de nuevo.

**Diseño en media-lite:**

**Cómo lo resolvieron otros y qué tomamos de cada uno:**

| Proyecto | Cómo consigue los streams | Qué tomamos |
|---|---|---|
| **yt-dlp** | Pide `tv_samsung` + `visionos` + `web` y junta los formatos. Resuelve `n`/`sig` con ejs (Deno o QuickJS) y cachea el reproductor | Los clientes y sus constantes, ejs, la caché por `player_id`, el `player_id` sacado de `/iframe_api` |
| **Sanson** (nuestra referencia) | `visionos` con `visitorData`. Abre en 1–2 s | Pedir `visitorData` antes del `/player`. `visionos` como primer cliente, porque no necesita JS ni hace esperar |
| **RustyPipe** (Rust) | Si la IP está bloqueada, pasa solo a usar las cookies del usuario. El PO token lo genera un binario aparte | Usar cookies cuando aparece `LOGIN_REQUIRED` por bot |
| **Invidious / NewPipe** | PO token con BotGuard (Deno + youtube.js, o WebView) | Nada: para 2 GB de RAM es demasiado |
| **mpv + yt-dlp** (ytfzf y similares) | Delegan todo en yt-dlp | Solo como último recurso |

```
 videoId (+visitorData) ─► visionos (sin JS, sin espera) ──── formatos avc1 ──────────────────────► mpv
                               │ falla o no responde en 2 s
                               ▼
                           tv_samsung (sts; sig y n con ejs) ─► esperar el anuncio previo si lo hay ─► mpv
                               │ falla
                               ▼
                           web_embedded (+ n/sig) ──► con cookies: tv_samsung / web_embedded / web_safari autenticados
                                                  ──► yt-dlp -J externo si está y es ≥ 2025.11.12
```

Todo lo que cambia seguido (versiones, user agents, orden) va en `clients.toml`.

**Apertura rápida (meta: < 3 s con la caché caliente).** El tiempo de un video se reparte entre
`/player` (estimado: 300–600 ms, sin medir), el solucionador (~0,5 s con el preprocesado en caché; 7 s la primera vez) y
el arranque de mpv. Por eso:

1. **Todo lo lento se hace antes del clic.** Al iniciar la app, en el pool de red y sin bloquear:
   pedir `visitorData` si venció (3 días, en `~/.config/media-lite/youtube.json`), leer el
   `player_id` de `https://www.youtube.com/iframe_api` (regex `player\\?/([0-9a-fA-F]{8})\\?/`, una
   vez por sesión), y si es nuevo, bajar `base.js`, preprocesarlo con el solucionador y guardar su
   `signatureTimestamp` (regex `(?:signatureTimestamp|sts)\s*:\s*([0-9]{5})`) en
   `yt-player/<id>.sts`. Así ningún video paga los 7 s del primer análisis.
2. **Pre-resolver.** Al abrir el detalle de un video, y durante la cuenta regresiva del siguiente,
   se resuelven las URLs. Quedan en memoria por `videoId` durante 5 h, y si mpv recibe un 403 se
   tiran. Cuando el usuario aprieta «Ver», ya están.
3. **`visionos` primero, `tv_samsung` de respaldo.** `visionos` no usa el solucionador ni hace
   esperar, así que es el camino rápido. `tv_samsung` sale si `visionos` falla, o en paralelo si
   `visionos` no respondió en 2 s. Se recuerda qué cliente anduvo durante la sesión.
4. **La espera del anuncio de `tv_samsung` se respeta, no se saltea.** Si la respuesta trae
   `adPlacements` (tipo `AD_PLACEMENT_KIND_START`) o `adSlots` (`SLOT_TRIGGER_EVENT_BEFORE_CONTENT`)
   con `instreamVideoAdRenderer`, YouTube no sirve el video hasta que pasa la duración del anuncio
   (`playerVars` → `length_seconds`, o `skipOffsetMilliseconds` si se puede saltar). yt-dlp lo
   maneja igual, con `available_at`. media-lite espera ese tiempo y muestra «YouTube hace esperar
   N s». Pre-resolver al abrir el detalle (punto 2) suele cubrir esa espera.
5. **Una sola llamada al solucionador por video**, con todos los `n` y `sig` juntos. La caché
   `n → resultado` ya existe (`Cache::solved`).
6. **Formatos de `visionos` (vistos en la laptop):** no trae el progresivo itag 18. Trae dos
   opciones:
   - **DASH directo:** video `avc1` 160/133/134/135/136/137 (144p–1080p) más audio `mp4a` 139/140.
     A 480p pesa **530 kb/s**, contra 863 kb/s del HLS. Va a mpv como URL de video +
     `Source.audio_url`.
   - **HLS** (`hlsManifestUrl`): variantes `avc1` 229–232/270 y audio aparte (233/234) en
     `EXT-X-MEDIA`. Pasa por el proxy, que ya filtra altura y códec y conserva el `EXT-X-MEDIA`.

   **Preferir DASH** (menos datos y sin proxy), salvo que la prueba de velocidad muestre que lo
   frenan. En ese caso, HLS. Los directos son siempre HLS.
7. **Una conexión que se reusa:** un solo `ureq::Agent` para `www.youtube.com`, y mpv sigue abierto
   entre videos (`loadfile`, ya hecho).

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
6. **`visionos` primero y `tv_samsung` de respaldo** (ver «Apertura rápida»). Después vienen
   `web_embedded` y los clientes con cookies, en el orden que diga `clients.toml`.
7. **Formatos**: solo **H.264 + AAC**. Se descartan VP9, AV1, los que traen `drmFamilies` y los
   que son solo SABR. El orden de preferencia está en «Apertura rápida», punto 5.
8. **yt-dlp queda como último recurso** solo si el usuario ya lo tiene instalado **con** un motor
   JS. La versión de yt-dlp en los repositorios de Debian/antiX suele quedar vieja (la de esta
   máquina es 2024.04.09), así que nunca es el camino principal.
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
- [x] **Hecho el 2 de noviembre de 2026, en la laptop:** sin aviso de bot. `visionos` entrega HLS y DASH `avc1`/`mp4a` sin JS; `tv_samsung` responde con un anuncio previo de 6 s (ver Estado). Reproductor `4efb8b2c`. Falta: velocidad de descarga de los DASH de `visionos` (para descartar que los frenen sin `n`) y reproducirlo en mpv.
- Así se repite la prueba (**la corre el usuario**, con un yt-dlp ≥ 2026.10; el de apt en la laptop ya es 2026.10.21): `yt-dlp -v -F --extractor-args "youtube:player_client=visionos,tv_samsung" dQw4w9WgXcQ`. Si sale «confirm you're not a bot», probar sin VPN; si persiste, la red está marcada y hacen falta cookies. Cuando exista, usar también `media-lite-cli yt-resolve` (Fase 4b).
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

- [ ] Obtener la URL y el `player_id` de `base.js`: **pasa a 4b** («Reproductor JS»). Se saca de `/iframe_api`, no de la página del video, que es más pesada.
- [x] Subcomando `media-lite --yt-solver`: lee el JSON de ejs por stdin y responde por stdout, en un proceso aparte. QuickJS-NG (`rquickjs` 0.14) con límite de memoria de 256 MB; el padre mata el proceso a los 60 s.
- [x] Caché por `player_id` del `preprocessed_player` y de los `n` y `sig` ya resueltos. Los ids se validan (8 hex) para no escribir fuera de la carpeta. La poda de 14 días está escrita (`Cache::prune`); falta llamarla al arrancar.
- [ ] Preprocesar en segundo plano al encontrar un `base.js` nuevo: **pasa a 4b** («Reproductor JS»), al arrancar la app.
- [x] Actualizar los scripts desde GitHub con verificación: `update::fetch_latest` baja el último release y compara cada archivo con el digest sha256 que publica la API (verificado en vivo). La copia instalada en `~/.local/share/media-lite/ejs/` se usa solo si su manifiesto y sus hashes coinciden. **Falta el botón en Ajustes** (Fase 7).
- [x] Pruebas con un `base.js` real: `solves_n_challenges_like_node` (prueba ignorada, con `MEDIA_LITE_TEST_BASEJS`) da los mismos resultados que Node. Prueba de punta a punta del proceso aparte: pendiente de correrla con el binario.

**Pruebas contra YouTube (leer antes de empezar).** En modo auto, el clasificador de permisos de
Claude Code **rechaza** que el agente haga pedidos de prueba a `youtubei/v1/player` y los marca como
«Bot Check Circumvention». Pasó dos veces, la última el 2 de noviembre de 2026. Quien implemente esto
**no debe buscarle la vuelta** (otros comandos, tests que en realidad hacen el pedido, etc.):

- Escribir el código y las pruebas **sin red**, con respuestas de `/player` guardadas como fixtures
  en `crates/providers/tests/fixtures/youtube/`. Las fixtures las genera el usuario con
  `media-lite-cli yt-resolve --dump`.
- Pedirle al usuario que corra los comandos contra YouTube (`! media-lite-cli yt-resolve <id>` en
  el prompt), o que agregue él mismo una regla de permiso para `media-lite-cli yt-resolve` en
  `.claude/settings.json`.
- El catálogo (`browse`, `search`, `next`) no es parte de este bloqueo, pero conviene probarlo igual
  con fixtures.

**4b. Cascada de clientes**

Antes que nada: agregar `Platform::YouTube` (clave `youtube`, nombre «YouTube») en
`crates/core/src/model.rs` y registrarlo en `providers::registry`.

Archivos nuevos: `crates/providers/src/youtube/{mod.rs, innertube.rs, player_js.rs, streams.rs}` y
`crates/providers/clients.toml` (incluido con `include_str!` y reemplazable desde
`~/.config/media-lite/clients.toml`). `providers` pasa a depender de `media-lite-ytsolver`, y el
ejecutable del solucionador es `std::env::current_exe()` con `--yt-solver` (ya existe en
`app/src/main.rs`).

- [ ] **`clients.toml`**: una tabla por cliente con `name`, `number` (`X-YouTube-Client-Name`),
      `version`, `user_agent`, `device_make`, `device_model`, `os_name`, `os_version`, `needs_js` y
      `embed_url` (solo `web_embedded`: `https://www.reddit.com/`). Más `order = ["visionos",
      "tv_samsung", "web_embedded"]` y `hedge_ms = 2000` (a los cuántos ms sin respuesta de `visionos`
      sale también `tv_samsung`). Valores iniciales, copiados de yt-dlp `_base.py`:
      - `visionos`: VISIONOS, 101, `1.02`, Apple / RealityDevice17,1, visionOS 26.5.23O471, UA
        `Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15`, `needs_js = false`.
      - `tv_samsung`: TVHTML5, 7, `5.20260707`, Samsung / UKS9800, Tizen 2.4.0, UA
        `Mozilla/5.0 (SMART-TV; Linux; Tizen 2.4.0) AppleWebKit/538.1 (KHTML, like Gecko) Version/2.4.0 TV Safari/538.1`, `needs_js = true`.
      - `web_embedded`: WEB_EMBEDDED_PLAYER, 56, `2.20260708.00.00`, UA de Safari
        `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/15.5 Safari/605.1.15,gzip(gfe)`, `needs_js = true`.
- [ ] **`visitorData`** (`innertube.rs`): `POST /youtubei/v1/visitor_id` con el contexto WEB, y si no
      viene, `browse` con `FEwhat_to_watch`. Se guarda en `youtube.json` con la fecha y vence a los
      3 días. Va en `context.client.visitorData` de cada `/player`, y también en el encabezado
      `X-Goog-Visitor-Id`. También se actualiza con `responseContext.visitorData` de cada respuesta.
- [ ] **Reproductor JS** (`player_js.rs`): `player_id` desde `/iframe_api`, URL
      `https://www.youtube.com/s/player/<id>/player_ias.vflset/en_US/base.js`, `sts` por regex
      (ver 2.4), y preprocesado en segundo plano con `ytsolver::solve` (el `player_js` solo se pasa
      si no hay caché). Llamar a `Cache::prune(MAX_AGE)` al arrancar.
- [ ] **`/player`**: `POST https://www.youtube.com/youtubei/v1/player?prettyPrint=false` con
      `{context:{client:{…, hl, gl, visitorData}}, videoId, contentCheckOk:true, racyCheckOk:true,
      playbackContext:{contentPlaybackContext:{html5Preference:"HTML5_PREF_WANTS",
      signatureTimestamp:<sts>}}}`. Encabezados: `User-Agent`, `X-YouTube-Client-Name`,
      `X-YouTube-Client-Version`, `Origin: https://www.youtube.com` y `Content-Type`.
      **Validar que `videoDetails.videoId` sea el pedido** (YouTube a veces responde otro video, ver
      NewPipe #8713).
- [ ] **Clasificar `playabilityStatus`**: `OK` sigue. `LOGIN_REQUIRED` con «bot» en el `reason`
      es `YtError::BotCheck`: se cortan los demás clientes sin cookies y se va a cookies.
      `LOGIN_REQUIRED` por edad y `UNPLAYABLE` pasan al siguiente cliente. `ERROR` es «video no
      disponible».
- [ ] **Descifrar** (`streams.rs`): si el formato trae `signatureCipher`, se parsea como query
      (`s`, `sp`, que por defecto es `signature`, y `url`). Se resuelve `s` como desafío `sig` y se
      agrega `&<sp>=<resultado>`. En toda URL con parámetro `n`, se resuelve como desafío `n` y se
      reemplaza. Todos los desafíos de un video van **en una sola** llamada a `ytsolver::solve`.
      Si el solucionador no puede, la falla es `YtError::Solver` y el mensaje dice «YouTube cambió;
      actualizá el soporte en Ajustes».
- [ ] **Elegir formatos**: descartar los que traen `drmFamilies`, los que no tienen `url` ni
      `signatureCipher` (SABR) y los que no son `avc1`/`mp4a`. Prioridad: DASH `avc1` ≤ calidad +
      itag 140 (`Source.audio_url`); si no hay, `hlsManifestUrl`; itag 18 solo si viene (de
      `tv_samsung`). Los directos van por `hlsManifestUrl`. Generar un `Source` por altura
      (240/360/480/720) para que el «bajar calidad por traba» que ya existe tenga a dónde ir.
- [ ] **Respaldo y caché** (`mod.rs`): `visionos` primero. Si falla, o si no respondió en
      `hedge_ms`, sale `tv_samsung` en otro hilo (`std::thread::scope`, sin tokio). Se usa el
      primero válido. `StreamCache` en memoria: `videoId → (Vec<Source>, available_at, Instant)`,
      vence a las 5 h. Se recuerda qué cliente anduvo.
- [ ] **Espera del anuncio previo** (solo `tv_samsung`): calcular `available_at` como yt-dlp
      (`_get_available_at_timestamp` en `_video.py`, ver «Apertura rápida» punto 4). Si al apretar
      «Ver» todavía no llegó la hora, `playback.rs` muestra «YouTube hace esperar N s» con una
      cuenta regresiva y recién después hace `loadfile`. No se saltea.
- [ ] **403 o URL vencida** (~6 h, o cambió la IP): `playback.rs` saca el elemento de la caché,
      vuelve a llamar a `details()` y retoma con `start=` en la misma posición. Un solo reintento.
- [ ] **Cookies mínimas (adelantado de la Fase 8)**: en Ajustes, «Pegar cookies de YouTube»
      (formato Netscape o encabezado `Cookie:`), guardadas en `cookies/youtube.txt` con permisos
      0600 y nunca en los logs. Con cookies, el `/player` lleva `Cookie`, `X-Goog-AuthUser: 0`,
      `X-Origin` y `Authorization: SAPISIDHASH <ts>_<sha1(ts + " " + SAPISID + " " + origin)>` (crate
      `sha1`), y la cascada pasa a `tv_samsung` → `web_embedded` → `web_safari` (HLS).
- [ ] **yt-dlp externo**: solo si `yt-dlp --version` da ≥ 2025.11.12. Se corre
      `yt-dlp -J --no-playlist -f "bv[vcodec^=avc1][height<=H]+ba[acodec^=mp4a]/18" <url>` y se
      toman `requested_formats[].url`. Si encuentra cookies, se las pasa con `--cookies`.
- [ ] **Mensajes** (en español, en la barra): «YouTube pide confirmar que no sos un bot. Pasa con
      VPN o redes marcadas: desactivá la VPN o cargá tus cookies en Ajustes», «YouTube cambió:
      actualizá el soporte en Ajustes», «Este video pide iniciar sesión (edad)», «Video no
      disponible en tu país».
- [ ] **CLI**: `media-lite-cli yt-resolve <id|url> [--client X] [--dump DIR] [--timing]` imprime
      el cliente ganador, los `Source` elegidos y el tiempo de cada paso (`visitorData`, `/player`,
      solucionador). Con `--dump` guarda las respuestas crudas como fixtures, **sin cookies ni
      `visitorData`**.
- [ ] **Pruebas sin red**: descifrado (`signatureCipher` → URL), elección de formatos, clasificación
      de `playabilityStatus` y parseo de `clients.toml`, todo con fixtures.

**Terminado cuando** `media-lite-cli yt-resolve dQw4w9WgXcQ --timing`, corrido por el usuario,
muestre `Source` H.264 en < 3 s con la caché caliente, y el video se vea en mpv desde la app.

**4c. Catálogo e interfaz** — **hecho el 2 de noviembre de 2026** (commit «Fase 4c - …»)

Se portó de `Sanson-FLinux/plugins/youtube/script.js`: la `api()` con el cliente web, las pestañas
de canal, los parsers de videos (clásicos, `lockupViewModel`, Shorts y canales) y el `visitorData` de
3 días. Lo que se necesitaba de `clients.toml` para el catálogo (el cliente web) se movió acá desde la
4b, porque la 4c no puede esperar a ella. `Platform::YouTube` también se agregó acá.

Cambios frente a Sanson, por lo que se vio en vivo (2 de noviembre de 2026):

- **Temas de la portada:** de los cinco de Sanson, quedan **Noticias** y **Deportes**. «Música» solo
  trae mezclas (`RD…`) y «Videojuegos» llega vacío. La música de YouTube queda para la Fase 6.
- **Canal:** se abre como serie, con sus pestañas como temporadas: «Videos», «En vivo» y «Shorts».
- **Duración:** en los resultados nuevos viene como texto plano dentro de `thumbnailBadges`, en
  cualquier overlay. Se lee desde todas las etiquetas de la miniatura.

- [x] **Cliente innertube WEB**: `clients.toml` (`web`, versión `2.20260708.00.00`, reemplazable desde
      `~/.config/media-lite/clients.toml`), portada por temas, búsqueda de videos y canales, y canal
      con pestañas. Probado en vivo: búsqueda («rick astley», 91 resultados con página siguiente),
      portada de Noticias (145 elementos) y las tres pestañas de un canal.
- [x] **`visitorData` guardado 3 días** en `~/.config/media-lite/youtube.json`. Se renueva con cada
      respuesta de YouTube. Sin cuenta: ni JS ni PO token.
- [x] **Búsqueda y pestañas de canal**: los Shorts salen aparte, como una temporada más.
- [ ] **Relacionados** (`next`): el trait `Provider` no tiene ese método todavía. Pasa a la 7, junto
      con el resto del detalle.
- [ ] **Listas de reproducción** (`playlistRenderer`, `lockupViewModel` de tipo lista): hoy se ignoran.
      Sanson las mostraba como listas; acá pueden ser series con una sola temporada.
- [ ] **Comentarios y chat en vivo** (`live_chat/get_live_chat` por polling): el chat se conecta al
      `crates/chat` cuando haya streams (4b), porque necesita el id del video.
- [ ] **«Siguiendo» sin cuenta** por el RSS de cada canal: pasa a la Fase 7 (la lista de canales
      seguidos es común a todas las plataformas).
- [x] **Botones de filtro Videos / En vivo / Shorts**: en los canales, cada pestaña es una temporada.
- [ ] **Directos de YouTube (HLS)** y **botón Idioma** (audio original y subtítulos): dependen de
      `/player`, así que pasan a la **4b**.

### Fase 5: Twitch y Kick (1–2 semanas)

**No depende de YouTube ni del solucionador, y no tiene el problema del «no sos un bot».** Se puede
hacer antes que la 4b. Hay que portar `Sanson-FLinux/plugins/twitch/script.js` (494 líneas),
`plugins/kick/script.js` (430), `ws.c` y la parte de chat de `app_watch.cpp`. Agregar
`Platform::Twitch` y `Platform::Kick`, con archivos `crates/providers/src/{twitch.rs, kick.rs}`.

**Hecho el 2 de noviembre de 2026** (commits «Fase 5 - …»). Lo probado en vivo desde el entorno de
desarrollo: portada, búsqueda, canal con temporadas, directo y grabación de cada plataforma, y chat
en vivo de Twitch y de Kick. Un canal se abre como serie (`Kind::Series`), porque `Kind::Channel` lo usan
los canales de TV.

- [x] **Twitch, catálogo**: `POST https://gql.twitch.tv/gql` con `Client-Id:
      kimne78kx3ncx6brgo4mv6wki5h1ko`, `Origin`/`Referer` `https://www.twitch.tv`. Portada de directos
      con cursor, búsqueda (canales, directos y grabaciones), canal con «En vivo» y «Transmisiones anteriores».
- [x] **Twitch, stream**: `streamPlaybackAccessToken` y `usher` con `supported_codecs=h264`
      (probado: 5 variantes, todas `avc1`). Las grabaciones usan `videoPlaybackAccessToken`.
- [x] **Anuncios de Twitch en el proxy**: `playlist::strip_twitch_ads` quita los segmentos de cada
      `EXT-X-DATERANGE` `twitch-stitched-ad` (o `stitched-ad-`) mientras dura, y los
      `EXT-X-TWITCH-PREFETCH`. Se activa con `Source.twitch_ads` y la consulta `a=1` al proxy. La barra
      muestra «Saltando el anuncio de Twitch…» durante 6 s después de cada corte.
- [x] **Kick**: portada (`web.kick.com/api/v1/livestreams`), búsqueda, canal (`api/v2/channels/<slug>`,
      con `playback_url`) y grabaciones (`api/v2/channels/<slug>/videos`, con respaldo en
      `api/v1/channels/<slug>`). Probado en vivo.
- [x] **Cloudflare en Kick**: un 403/503 con HTML no se esquiva. Muestra «Kick bloqueó el acceso desde
      esta red» y deja Kick desactivado en la sesión (`core::http::KICK_BLOCKED`).
- [x] **Chat en vivo de Twitch** (IRC sobre WebSocket, anónimo, con etiquetas, emotes y bits; crate
      `media-lite-chat`). Probado en vivo.
- [x] **Chat en vivo de Kick** (historial, luego Centrifugo con token de invitado). Probado en vivo.
- [x] **Repetición del chat de las grabaciones** sincronizada con `time-pos`: Twitch por comentarios
      del video, Kick por cursor. Probadas las dos en vivo (los primeros tres minutos).
- [x] **Panel de chat** en la ventana: columna superpuesta a la derecha, con las últimas 60 líneas y
      hasta 300 en memoria. Se abre al reproducir un directo o una grabación con chat.
- [ ] **Emotes como imágenes**: hoy el chat muestra el nombre del emote (`Kappa`), y la URL de la
      imagen ya viene en cada mensaje. Falta bajarla con la caché de miniaturas y mostrarla en la fila.
- [ ] **Estado del panel en la laptop**: el diseño superpuesto no se miró a ojo en la laptop (solo se
      comprobó que compila y que la app arranca).
- [ ] **Twitch, directos con tokens restringidos** (suscriptores, sin cuenta): hoy se muestra el motivo
      que da Twitch. Pasa a la Fase 8 (cuentas).

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

- [ ] Iniciar sesión **pegando cookies** (formato Netscape o encabezado `Cookie:`), como ya permite Sanson. **Para YouTube pasa a la 4b**; acá queda para Twitch y Kick.
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
| YouTube pide «confirmá que no sos un bot» (IP sin confianza) | `visitorData` siempre; avisar de la VPN; cookies del usuario (4b). No esquivarlo |
| El agente no puede probar contra YouTube (clasificador de Claude Code) | Fixtures + comandos que corre el usuario (ver «Pruebas contra YouTube», Fase 4) |
| Kick responde con el desafío de Cloudflare | Avisar y deshabilitar Kick en la sesión; no imitar huellas TLS |
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

**Orden desde la v0.4 (2 de noviembre de 2026):** Fase 5 (Twitch y Kick, no tiene bloqueos) →
4c (catálogo de YouTube, sin `/player`) → 4b (streams, con las pruebas que corre el usuario) →
Fase 6 → resto de la 7 → 8.

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
- [yt-dlp: constantes de clientes (`_base.py`)](https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/youtube/_base.py): versiones de `visionos`, `tv_samsung`, `web_safari`, `web_embedded` y `tv`; nota de que `web_safari` solo da HLS con sesión desde 2026.07 (revisado el 2 de noviembre de 2026)
- [yt-dlp: `_video.py`](https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/youtube/_video.py): `_DEFAULT_CLIENTS = ('tv_samsung', 'visionos', 'web')`, `_download_player_url` (`/iframe_api`), regex de `signatureTimestamp`, `_invalid_player_response`
- [yt-dlp #17723](https://github.com/yt-dlp/yt-dlp/pull/17723) y [#17805](https://github.com/yt-dlp/yt-dlp/pull/17805): cliente `tv_samsung` y su uso por defecto (10 de octubre de 2026)
- [yt-dlp #17461](https://github.com/yt-dlp/yt-dlp/pull/17461): `android_vr` da 403 en todo desde el 17 de agosto de 2026
- [yt-dlp #15865](https://github.com/yt-dlp/yt-dlp/issues/15865): «Sign in to confirm you're not a bot» significa que YouTube no confía en la IP
- [Invidious: errores de YouTube explicados](https://docs.invidious.io/youtube-errors-explained/): el aviso de bot como bloqueo de IP; PO tokens con BotGuard
- [RustyPipe](https://codeberg.org/ThetaDev/rustypipe): cliente de YouTube en Rust; pasa a cookies cuando la IP está bloqueada
- [NewPipe Extractor: `PoTokenProvider`](https://teamnewpipe.github.io/NewPipeExtractor/javadoc/org/schabi/newpipe/extractor/services/youtube/PoTokenProvider.html)
- [streamlink: plugin de Twitch](https://github.com/streamlink/streamlink/blob/master/src/streamlink/plugins/twitch.py): detección de anuncios por `EXT-X-DATERANGE` (`twitch-stitched-ad`)
