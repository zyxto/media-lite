# Validación del hardware y del solucionador de YouTube

Estado: **parcial**. Lo que se pudo medir desde el entorno de desarrollo está en la primera
parte. Lo que requiere la laptop (Pentium T4500, GMA 4500M, antiX) figura como pendiente.

Los números de la primera parte **no son los de la T4500**: sirven para verificar que las cosas
funcionan y para corregir el presupuesto, no para fijar tiempos finales.

## Medido en el entorno de desarrollo

Entorno: Linux x86-64 de escritorio, rustc 1.95.0.

### Solucionador de YouTube (QuickJS-NG + scripts de yt-dlp/ejs)

| Ítem | Resultado |
|---|---|
| QuickJS-NG que trae `rquickjs-sys` 0.14.0 | **0.16.2** (cumple el mínimo 0.12 que pide yt-dlp) |
| Scripts `ejs` | yt-dlp/ejs **0.8.0**: `yt.solver.lib.min.js` (sha256 `c55987fe…aa577`) y `yt.solver.core.min.js` (sha256 `18da6ce0…cd00`) |
| Reproductor de prueba | `base.js` de `11a45ffb`, 3,0 MB |
| Primera vez (`type: "player"`) | **7,4 s**, pico de RSS **210 MB**. Con un límite de memoria de 150 MB falla por falta de memoria: hace falta al menos **256 MB** |
| Con preprocesado (`type: "preprocessed"`) | **0,45 s**, pico de RSS **76 MB** |
| Correctitud | Node (motor de referencia) y QuickJS-NG dan la misma salida para los dos desafíos `n` de prueba |

Consecuencias para el roadmap:

- El pico de RAM de la primera preprocesada **no entra en los 150 MB** que se había presupuestado.
  El presupuesto pasa a **256 MB** (se libera al terminar el proceso del solucionador).
- Las reproducciones siguientes, con el preprocesado en caché, usan menos de 80 MB.

### Clientes de YouTube

- Desde el entorno de desarrollo, YouTube respondió «confirmá que no sos un bot» a `visionos` y `tv`,
  y «video no disponible» también al cliente WEB normal. **No es concluyente**: ese entorno está
  bloqueado para YouTube. Hay que repetir la prueba desde una red residencial (pendiente).
- 2 de noviembre de 2026: la red de la laptop es residencial (Telmex, AR). La prueba no se pudo
  correr porque el clasificador de permisos de Claude Code bloquea los pedidos del agente a
  `/player`. La tiene que correr el usuario (ver «Pruebas contra YouTube» en la Fase 4 del roadmap).
- **Prueba en la laptop (2 de noviembre de 2026, antiX, yt-dlp 2026.10.21, sin motor JS):**
  `yt-dlp -v -F --extractor-args "youtube:player_client=visionos,tv_samsung" dQw4w9WgXcQ`.
  **No hubo aviso de bot.** `visionos` entregó HLS `avc1` 144p–1080p (229–232, 270) con audio
  aparte (233/234), y DASH directo `avc1` 160/133/134/135/136/137 + `mp4a` 139/140. A 480p, DASH 135
  pesa 530 kb/s y HLS 231 pesa 863 kb/s. No vino el itag 18. `tv_samsung` trajo un anuncio previo de
  6 s, y sus formatos no salieron porque no había motor JS. Reproductor: `4efb8b2c`.
- `tv_samsung` (cliente por defecto de yt-dlp desde el 10/10/2026): TVHTML5 `5.20260707`,
  Samsung / UKS9800, Tizen 2.4.0. Necesita `sig` + `n` y no pide PO token.
  `web_safari` solo da HLS con sesión desde 2026.07, y `android_vr` da 403 desde el 17/08/2026.
- Constantes vigentes en yt-dlp (`yt_dlp/extractor/youtube/_base.py`, revisado el 1 de noviembre de 2026):
  - `visionos`: `clientName` VISIONOS, `clientVersion` 1.02, `osVersion` 26.5.23O471, sin JS obligatorio.
  - `web_safari`: WEB, `clientVersion` 2.20260708.00.00, UA de Safari.
  - `web_embedded`: WEB_EMBEDDED_PLAYER, misma versión.
  - `tv`: TVHTML5, `clientVersion` 7.20260707.07.00, UA de Cobalt.
  
  Sanson usa constantes más viejas (VisionOS 1.04 / 26.6.0). Por eso van en `clients.toml`, no en el código.

## Pendiente (requiere la laptop)

- [ ] `lspci | grep VGA`, `glxinfo -B`, `vainfo`: GPU, versión de GL y si VA-API decodifica H.264.
- [ ] mpv con un video de YouTube a 360p, 480p y 720p en H.264: CPU y RAM con `--vo=gpu`, `--vo=xv`,
      `--hwdec=no` y `--hwdec=vaapi`.
- [ ] Un directo HLS de Twitch y un canal de Pluto TV en mpv.
- [ ] Ejemplo de Slint con 200 tarjetas con `winit-software` y `winit-femtovg`: RAM y fluidez.
- [ ] Solucionador en la T4500: tiempo y pico de RAM de la primera vez y con preprocesado.
      Si la primera vez pasa de 30 s, revisar la caché y el preprocesado en segundo plano.
- [ ] `visionos` y `web_safari`: HLS con variantes H.264 desde una red residencial.

Hasta que se midan, los valores por defecto son conservadores: renderizador `winit-software`,
`--vo=xv`, `--hwdec=no` y calidad 480p (360p si la ventana es chica).
