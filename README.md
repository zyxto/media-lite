# media-lite

Televisión gratis, películas y series en una sola app liviana, pensada para equipos viejos
(2 GB de RAM, procesadores sin SSE4) con antiX, Debian o cualquier derivado de Debian de 64 bits.

Junta **Pluto TV**, **Runtime** y **Plex** con un reproductor propio (mpv en su ventana). Está
escrita en Rust con la interfaz Slint, sin navegador embebido. YouTube, Twitch, Kick y la música
llegan en fases posteriores: ver [roadmap.md](roadmap.md).

## Instalar

1. Instalá el reproductor del sistema (no va dentro del AppImage):

   ```sh
   sudo apt install mpv
   ```

2. Descargá `media-lite-x86_64.AppImage` desde la sección **Releases** del repositorio, dale
   permiso de ejecución y abrilo:

   ```sh
   chmod +x media-lite-x86_64.AppImage
   ./media-lite-x86_64.AppImage
   ```

   Si no abre porque falta FUSE, usá:

   ```sh
   ./media-lite-x86_64.AppImage --appimage-extract-and-run
   ```

## Uso

- Barra lateral: plataformas y secciones (en vivo, películas, series).
- Arriba: buscador y categorías. En una serie, los chips son sus temporadas.
- Un clic en una tarjeta abre el detalle; **Reproducir** abre el video en una ventana flotante
  chica, con controles en la app.

| Tecla | Acción |
|---|---|
| `/` | buscar |
| Esc o Retroceso | volver |
| F5 | recargar |
| Espacio | pausa |
| ← / → | 10 segundos atrás / adelante |
| `<` / `>` | anterior / siguiente |

Lo que se ve queda guardado en el propio equipo (`~/.config/media-lite`). Al terminar un video,
la app espera cinco segundos y pasa al siguiente de la lista.

## Qué no hace

- No descarga nada: solo reproduce contenido gratuito que las plataformas entregan sin DRM ni
  suscripción.
- No reproduce lo que pide cuenta paga ni lo que tiene DRM (se oculta).

## Desarrollo

```sh
cargo test --workspace            # pruebas
cargo test -p media-lite-player -- --ignored   # prueba de reproducción (necesita red y mpv)
cargo run -p media-lite-cli -- platforms       # probar los proveedores sin interfaz
packaging/build-appimage.sh       # construye el AppImage dentro de Debian 11 (requiere Docker)
```

Estructura: `crates/core` (modelo, ajustes, caché de miniaturas, HTTP), `crates/providers` (una
plataforma por archivo, detrás de un trait), `crates/player` (mpv por IPC y proxy de playlists),
`crates/cli` (`media-lite-cli`) y `app` (la interfaz Slint).

## Licencia

GPL-3.0-or-later (ver [LICENSE](LICENSE)). La fuente Inter se distribuye bajo la SIL Open Font
License 1.1 (`app/assets/fonts/OFL.txt`).
