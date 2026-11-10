// PicaRust: visor de imágenes ligero y rápido al estilo del "Photo Viewer" de Picasa.
//
// Uso: picarust <imagen>
//
// Controles (pulsa H dentro de la app para verlos):
//   Rueda / Ctrl+rueda / pellizco      zoom centrado en el cursor
//   Arrastrar                          mover la imagen
//   Doble clic                         alternar "ajustar" / 100 %
//   Clic fuera de la imagen / Esc      cerrar
//   ↑ / ↓  (mantener)                  zoom continuo
//   ← → RePág AvPág Espacio Retr.      imagen anterior / siguiente de la carpeta
//   Inicio / Fin                       primera / última
//   + / -   0   1                      zoom, ajustar, 100 %
//   P                                  píxeles nítidos al ampliar (sin interpolar)
//   R / L                              girar a la derecha / izquierda (sólo vista)
//   I                                  panel de información y metadatos EXIF
//   S                                  presentación (cada 3 s)
//   T                                  opacidad del fondo
//   F / F11                            pantalla completa
//   H                                  ayuda

#![cfg_attr(windows, windows_subsystem = "windows")]

use eframe::egui::{
    self, Color32, ColorImage, FontId, Key, Mesh, Pos2, Rect, Sense, Shape, TextureFilter,
    TextureHandle, TextureOptions, Vec2, ViewportCommand,
};
use egui::emath::Rot2;
use image::ImageDecoder;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "webp", "bmp", "tif", "tiff",
];
const MIN_ZOOM: f32 = 0.01;
const MAX_ZOOM: f32 = 64.0;
/// Opacidades del fondo que se alternan con la tecla T (la primera es la inicial).
const OPACITIES: &[u8] = &[180, 128, 230, 255];

/// Logger mínimo: muestra en la terminal los avisos/errores de eframe (p. ej. si la
/// ventana no puede ser transparente).
struct StderrLogger;
impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{}] {}: {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}
/// Velocidad del zoom con ↑/↓ mantenidas (e^ZOOM_SPEED veces por segundo ≈ 7×).
const ZOOM_SPEED: f32 = 2.0;
const SLIDESHOW: Duration = Duration::from_secs(3);

const HELP: &str = "\
Rueda / pellizco      Zoom en el cursor
Arrastrar             Mover
Doble clic            Ajustar / 100 %
↑ ↓ (mantener)        Zoom continuo
← →  Espacio  Retr.   Anterior / siguiente
Inicio / Fin          Primera / última
+  -  0  1            Zoom, ajustar, 100 %
P                     Píxeles nítidos / suavizado
R / L                 Girar derecha / izquierda
I                     Información y EXIF
S                     Presentación
T                     Opacidad del fondo
F / F11               Pantalla completa
H                     Esta ayuda
Esc / clic fuera      Cerrar";

/// `picarust --bench <imágenes o carpetas...>`: mide la decodificación sin abrir ventana.
fn bench(args: &[PathBuf]) {
    let mut files = Vec::new();
    for a in args {
        if a.is_dir() {
            let any = a.join("x");
            files.extend(list_folder(&any));
        } else {
            files.push(a.clone());
        }
    }
    println!("{:<45} {:>13} {:>10} {:>6}", "archivo", "dimensiones", "decodif.", "EXIF");
    let mut total = 0.0;
    for f in &files {
        let name = f.file_name().unwrap_or_default().to_string_lossy();
        match decode(f, 16384) {
            Ok(d) => {
                total += d.info.decode_ms;
                let [w, h] = d.image.size;
                println!(
                    "{name:<45} {:>13} {:>7.1} ms {:>6}",
                    format!("{w}x{h}"),
                    d.info.decode_ms,
                    d.info.exif.len()
                );
            }
            Err(e) => println!("{name:<45} ERROR: {e}"),
        }
    }
    println!("total: {} imágenes en {total:.1} ms", files.len());
}

fn main() -> eframe::Result {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if args.first().is_some_and(|a| a.as_os_str() == "--bench") {
        bench(&args[1..]);
        return Ok(());
    }
    let initial = args.into_iter().next();
    let _ = log::set_logger(&StderrLogger).map(|()| log::set_max_level(log::LevelFilter::Warn));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PicaRust")
            .with_app_id("picarust")
            .with_decorations(false)
            .with_transparent(true)
            .with_fullscreen(true),
        ..Default::default()
    };

    eframe::run_native(
        "PicaRust",
        options,
        Box::new(move |cc| Ok(Box::new(Viewer::new(cc, initial)))),
    )
}

// ---------------------------------------------------------------------------
// Carga de imágenes (en hilos de fondo)
// ---------------------------------------------------------------------------

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Lista las imágenes de la carpeta de `path`, ordenadas por nombre.
fn list_folder(path: &Path) -> Vec<PathBuf> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file() && is_image(p))
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    files
}

fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.2} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Información de la imagen mostrada en el panel "I".
#[derive(Clone, Default)]
struct Info {
    general: Vec<(String, String)>,
    exif: Vec<(String, String)>,
    decode_ms: f64,
}

struct Decoded {
    image: ColorImage,
    info: Info,
}

fn read_exif(path: &Path) -> Vec<(String, String)> {
    use exif::{In, Tag};
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    let Ok(data) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file))
    else {
        return vec![];
    };
    // Campos más útiles primero, en un orden legible.
    const MAIN: &[(Tag, &str)] = &[
        (Tag::Make, "Fabricante"),
        (Tag::Model, "Cámara"),
        (Tag::LensModel, "Objetivo"),
        (Tag::DateTimeOriginal, "Fecha de captura"),
        (Tag::ExposureTime, "Exposición"),
        (Tag::FNumber, "Apertura"),
        (Tag::PhotographicSensitivity, "ISO"),
        (Tag::FocalLength, "Distancia focal"),
        (Tag::FocalLengthIn35mmFilm, "Focal equiv. 35 mm"),
        (Tag::ExposureBiasValue, "Compensación"),
        (Tag::ExposureProgram, "Programa"),
        (Tag::MeteringMode, "Medición"),
        (Tag::Flash, "Flash"),
        (Tag::WhiteBalance, "Balance de blancos"),
        (Tag::Orientation, "Orientación"),
        (Tag::Software, "Software"),
        (Tag::Artist, "Autor"),
        (Tag::Copyright, "Copyright"),
        (Tag::ImageDescription, "Descripción"),
        (Tag::GPSLatitude, "Latitud"),
        (Tag::GPSLongitude, "Longitud"),
        (Tag::GPSAltitude, "Altitud"),
    ];
    let mut out = Vec::new();
    for (tag, label) in MAIN {
        if let Some(f) = data.get_field(*tag, In::PRIMARY) {
            let mut v = f.display_value().with_unit(&data).to_string();
            if *tag == Tag::GPSLatitude || *tag == Tag::GPSLongitude {
                let ref_tag = if *tag == Tag::GPSLatitude {
                    Tag::GPSLatitudeRef
                } else {
                    Tag::GPSLongitudeRef
                };
                if let Some(r) = data.get_field(ref_tag, In::PRIMARY) {
                    v = format!("{v} {}", r.display_value());
                }
            }
            out.push((label.to_string(), v.trim_matches('"').to_string()));
        }
    }
    out
}

/// Decodifica una imagen (respetando la orientación EXIF) y reúne sus metadatos.
fn decode(path: &Path, max_side: u32) -> Result<Decoded, String> {
    let t0 = Instant::now();
    let reader = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let format = reader
        .format()
        .map(|f| format!("{f:?}").to_uppercase())
        .unwrap_or_else(|| "?".into());
    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let (ow, oh) = (img.width(), img.height());
    let color = format!("{:?}", img.color());
    // Las GPUs tienen un tamaño máximo de textura; reducir sólo si se excede.
    if ow > max_side || oh > max_side {
        img = img.resize(max_side, max_side, image::imageops::FilterType::Triangle);
    }
    let size = [img.width() as usize, img.height() as usize];
    let image = if img.color().has_alpha() {
        ColorImage::from_rgba_unmultiplied(size, img.into_rgba8().as_raw())
    } else {
        ColorImage::from_rgb(size, img.into_rgb8().as_raw())
    };
    let decode_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut general = vec![
        (
            "Archivo".into(),
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        (
            "Carpeta".into(),
            path.parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        ),
        ("Formato".into(), format),
        (
            "Dimensiones".into(),
            format!("{ow} × {oh} px  ({:.1} MP)", (ow as f64 * oh as f64) / 1e6),
        ),
        ("Color".into(), color),
    ];
    if let Ok(meta) = std::fs::metadata(path) {
        general.push(("Tamaño".into(), human_size(meta.len())));
    }
    if size != [ow as usize, oh as usize] {
        general.push((
            "Textura".into(),
            format!("reducida a {} × {} (límite GPU)", size[0], size[1]),
        ));
    }
    general.push(("Decodificación".into(), format!("{decode_ms:.1} ms")));

    Ok(Decoded {
        image,
        info: Info {
            general,
            exif: read_exif(path),
            decode_ms,
        },
    })
}

// ---------------------------------------------------------------------------
// Aplicación
// ---------------------------------------------------------------------------

enum Entry {
    Loading,
    /// El `bool` indica si la textura se subió en modo píxel (sin interpolar).
    Ready(TextureHandle, Info, bool),
    Failed(String),
}

type LoadResult = (PathBuf, bool, Result<Decoded, String>);

struct Viewer {
    files: Vec<PathBuf>,
    index: usize,
    cache: HashMap<PathBuf, Entry>,
    /// Decodificaciones en curso.
    pending: HashSet<PathBuf>,
    /// Ampliar sin interpolar (se ven los píxeles). Por defecto suavizado, como Picasa.
    pixel: bool,
    tx: Sender<LoadResult>,
    rx: Receiver<LoadResult>,
    max_side: u32,
    /// `true` = la imagen se ajusta a la pantalla (sin ampliar las pequeñas).
    fit: bool,
    /// Píxeles físicos de pantalla por píxel de imagen (cuando `fit == false`).
    zoom: f32,
    /// Desplazamiento del centro de la imagen respecto al centro de la ventana (en puntos).
    offset: Vec2,
    /// Giro de la vista en cuartos de vuelta (0..=3, sentido horario).
    rotation: u8,
    fullscreen: bool,
    show_info: bool,
    show_help: bool,
    opacity: usize,
    slideshow: Option<Instant>,
    started: bool,
    initial: Option<PathBuf>,
    launched: Instant,
    /// Tiempo desde el arranque hasta la primera imagen en pantalla.
    first_paint_ms: Option<f64>,
}

impl Viewer {
    fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tx, rx) = channel();
        Self {
            files: Vec::new(),
            index: 0,
            cache: HashMap::new(),
            pending: HashSet::new(),
            pixel: false,
            tx,
            rx,
            max_side: 8192,
            fit: true,
            zoom: 1.0,
            offset: Vec2::ZERO,
            rotation: 0,
            fullscreen: true,
            show_info: false,
            show_help: false,
            opacity: 0,
            slideshow: None,
            started: false,
            initial,
            launched: Instant::now(),
            first_paint_ms: None,
        }
    }

    fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        // Lanzar la decodificación antes de listar la carpeta: lo primero es mostrarla.
        self.request(ctx, &path);
        let mut files = list_folder(&path);
        let index = match files.iter().position(|p| *p == path) {
            Some(i) => i,
            None => {
                files.insert(0, path.clone());
                0
            }
        };
        self.files = files;
        self.index = index;
        self.reset_view();
    }

    fn request(&mut self, ctx: &egui::Context, path: &Path) {
        let needed = match self.cache.get(path) {
            None => true,
            // Cambió el modo de filtrado: se vuelve a subir, pero se sigue mostrando la
            // textura antigua hasta que llegue la nueva (sin parpadeo).
            Some(Entry::Ready(_, _, px)) => *px != self.pixel,
            // Cargando sin hilo activo = su resultado se descartó; hay que repetir.
            Some(Entry::Loading) => true,
            Some(Entry::Failed(_)) => false,
        };
        if !needed || self.pending.contains(path) {
            return;
        }
        self.cache
            .entry(path.to_path_buf())
            .or_insert(Entry::Loading);
        self.pending.insert(path.to_path_buf());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let path = path.to_path_buf();
        let max_side = self.max_side;
        let pixel = self.pixel;
        std::thread::spawn(move || {
            let result = decode(&path, max_side);
            let _ = tx.send((path, pixel, result));
            ctx.request_repaint();
        });
    }

    /// Precarga las imágenes vecinas y libera las que ya no hacen falta.
    fn manage_cache(&mut self, ctx: &egui::Context) {
        let n = self.files.len();
        if n == 0 {
            return;
        }
        let mut idx = vec![self.index, (self.index + 1) % n, (self.index + n - 1) % n];
        idx.dedup();
        let keep: Vec<PathBuf> = idx.into_iter().map(|i| self.files[i].clone()).collect();
        self.request(ctx, &keep[0]);
        // Sólo precargar cuando la actual ya está lista, para no competir con ella.
        if !matches!(self.cache.get(&keep[0]), Some(Entry::Loading)) {
            for p in &keep[1..] {
                self.request(ctx, p);
            }
        }
        let keep: HashSet<PathBuf> = keep.into_iter().collect();
        self.cache
            .retain(|p, _| keep.contains(p));
    }

    fn go(&mut self, delta: isize) {
        let n = self.files.len() as isize;
        if n > 0 {
            self.index = (self.index as isize + delta).rem_euclid(n) as usize;
            self.reset_view();
        }
    }

    fn go_to(&mut self, index: usize) {
        if index < self.files.len() {
            self.index = index;
            self.reset_view();
        }
    }

    fn reset_view(&mut self) {
        self.fit = true;
        self.offset = Vec2::ZERO;
        self.rotation = 0;
    }

    /// Cambia el zoom manteniendo fijo el punto `anchor` de la pantalla.
    fn zoom_at(&mut self, factor: f32, anchor: Pos2, center: Pos2, current: f32) {
        let new_zoom = (current * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let ratio = new_zoom / current;
        let img_center = center + self.offset;
        let new_center = anchor - (anchor - img_center) * ratio;
        self.offset = new_center - center;
        self.zoom = new_zoom;
        self.fit = false;
    }

    /// Teclas que no dependen de tener imagen cargada. Devuelve `true` si hay que cerrar.
    fn handle_global_keys(&mut self, ctx: &egui::Context) -> bool {
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let any = |keys: &[Key]| keys.iter().any(|k| pressed(*k));

        if pressed(Key::Escape) {
            // Esc cierra primero los paneles; si no hay ninguno, cierra la app.
            if self.show_help {
                self.show_help = false;
            } else {
                return true;
            }
        }
        if ctx.input(|i| i.modifiers.command) && pressed(Key::W) {
            return true;
        }
        if any(&[Key::ArrowRight, Key::PageDown, Key::Space]) {
            self.go(1);
            self.slideshow = self.slideshow.map(|_| Instant::now());
        }
        if any(&[Key::ArrowLeft, Key::PageUp, Key::Backspace]) {
            self.go(-1);
            self.slideshow = self.slideshow.map(|_| Instant::now());
        }
        if pressed(Key::Home) {
            self.go_to(0);
        }
        if pressed(Key::End) && !self.files.is_empty() {
            self.go_to(self.files.len() - 1);
        }
        if pressed(Key::I) {
            self.show_info = !self.show_info;
        }
        if pressed(Key::H) || pressed(Key::Questionmark) {
            self.show_help = !self.show_help;
        }
        if pressed(Key::P) {
            self.pixel = !self.pixel;
        }
        if pressed(Key::T) {
            self.opacity = (self.opacity + 1) % OPACITIES.len();
        }
        if pressed(Key::S) {
            self.slideshow = match self.slideshow {
                Some(_) => None,
                None => Some(Instant::now()),
            };
        }
        if pressed(Key::R) {
            self.rotation = (self.rotation + 1) % 4;
            self.offset = Vec2::ZERO;
        }
        if pressed(Key::L) {
            self.rotation = (self.rotation + 3) % 4;
            self.offset = Vec2::ZERO;
        }
        if any(&[Key::F, Key::F11]) {
            self.fullscreen = !self.fullscreen;
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(self.fullscreen));
            if !self.fullscreen {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
            }
        }
        false
    }

    fn receive_decoded(&mut self, ctx: &egui::Context) {
        while let Ok((path, pixel, result)) = self.rx.try_recv() {
            self.pending.remove(&path);
            // Descartar si ya no interesa o si se decodificó para el otro modo de filtrado.
            if !self.cache.contains_key(&path) || pixel != self.pixel {
                continue;
            }
            let entry = match result {
                Ok(d) => {
                    // Reducir: siempre trilineal con mipmaps (nítido, sin aliasing).
                    // Ampliar: bilineal (suave, como Picasa) o vecino más próximo (píxeles).
                    let mut opts =
                        TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));
                    if pixel {
                        opts.magnification = TextureFilter::Nearest;
                    }
                    let name = path.to_string_lossy().into_owned();
                    Entry::Ready(ctx.load_texture(name, d.image, opts), d.info, pixel)
                }
                Err(e) => Entry::Failed(e),
            };
            self.cache.insert(path, entry);
        }
    }
}

/// Dibuja un cuadro de texto semitransparente.
fn panel(painter: &egui::Painter, pos: Pos2, anchor: egui::Align2, text: String, font: FontId) {
    let galley = painter.layout_no_wrap(text, font, Color32::from_gray(220));
    let pad = Vec2::new(14.0, 10.0);
    let rect = anchor.anchor_size(pos, galley.size() + pad * 2.0);
    painter.rect_filled(rect, 8.0, Color32::from_black_alpha(190));
    painter.galley(rect.min + pad, galley, Color32::from_gray(220));
}

fn format_table(rows: &[(String, String)]) -> String {
    let w = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    rows.iter()
        .map(|(k, v)| format!("{k:<w$}  {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl eframe::App for Viewer {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if !self.started {
            self.started = true;
            self.max_side = ctx.input(|i| i.max_texture_side).clamp(2048, 16384) as u32;
            if let Some(p) = self.initial.take() {
                self.open(&ctx, p);
            }
        }

        // Archivos arrastrados a la ventana.
        let dropped: Option<PathBuf> =
            ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()));
        if let Some(p) = dropped {
            self.open(&ctx, p);
        }

        if self.handle_global_keys(&ctx) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }

        // Presentación automática.
        if let Some(t) = self.slideshow {
            let elapsed = t.elapsed();
            if elapsed >= SLIDESHOW {
                self.go(1);
                self.slideshow = Some(Instant::now());
            } else {
                ctx.request_repaint_after(SLIDESHOW - elapsed);
            }
        }

        self.receive_decoded(&ctx);
        self.manage_cache(&ctx);

        let screen = ui.max_rect();
        let painter = ui.painter().clone();
        painter.rect_filled(screen, 0.0, Color32::from_black_alpha(OPACITIES[self.opacity]));

        let response = ui.interact(screen, egui::Id::unique("picarust-canvas"), Sense::click_and_drag());
        let ppp = ctx.pixels_per_point();
        let center = screen.center();

        let current = self.current().and_then(|p| match self.cache.get(p) {
            Some(Entry::Ready(t, info, _)) => Some(Ok((t.clone(), info.clone()))),
            Some(Entry::Failed(e)) => Some(Err(e.clone())),
            _ => None,
        });

        let mut status = String::new();
        let mut img_rect = Rect::NOTHING;
        let mut info_text = None;

        match current {
            Some(Ok((tex, info))) => {
                let [w, h] = tex.size();
                let rotated = self.rotation % 2 == 1;
                // Tamaño en puntos a zoom 1:1 (un píxel de imagen = un píxel físico).
                let mut img_pts = Vec2::new(w as f32, h as f32) / ppp;
                if rotated {
                    img_pts = Vec2::new(img_pts.y, img_pts.x);
                }
                let fit_zoom = (screen.width() / img_pts.x)
                    .min(screen.height() / img_pts.y)
                    .min(1.0);
                let cur_zoom = |s: &Self| if s.fit { fit_zoom } else { s.zoom };

                // Zoom: rueda (suavizada por egui), Ctrl+rueda / pellizco y teclas.
                let (scroll, pinch, hover) = ctx.input(|i| {
                    (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos())
                });
                let mut factor = (scroll * 0.004).exp() * pinch;
                ctx.input(|i| {
                    if i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals) {
                        factor *= 1.25;
                    }
                    if i.key_pressed(Key::Minus) {
                        factor /= 1.25;
                    }
                    for e in &i.events {
                        if let egui::Event::Key { key, pressed: true, repeat: false, .. } = e {
                            match key {
                                Key::ArrowUp => factor *= 1.15,
                                Key::ArrowDown => factor /= 1.15,
                                _ => {}
                            }
                        }
                    }
                });
                // Mantenida: zoom continuo, independiente de la repetición del teclado.
                let (up, down, dt) = ctx.input(|i| {
                    (i.key_down(Key::ArrowUp), i.key_down(Key::ArrowDown), i.stable_dt.min(0.05))
                });
                let held = up as i32 - down as i32;
                if held != 0 {
                    factor *= (held as f32 * ZOOM_SPEED * dt).exp();
                    ctx.request_repaint();
                }
                // Con el teclado se amplía hacia el centro de la pantalla.
                let hover = if held != 0 { None } else { hover };
                if (factor - 1.0).abs() > 1e-4 {
                    let z = cur_zoom(self);
                    self.zoom_at(factor, hover.unwrap_or(center), center, z);
                }
                if ctx.input(|i| i.key_pressed(Key::Num0)) {
                    self.fit = true;
                    self.offset = Vec2::ZERO;
                }
                if ctx.input(|i| i.key_pressed(Key::Num1)) {
                    self.fit = false;
                    self.zoom = 1.0;
                    self.offset = Vec2::ZERO;
                }

                if response.double_clicked() {
                    if self.fit && fit_zoom < 0.999 {
                        let anchor = response.interact_pointer_pos().unwrap_or(center);
                        self.zoom_at(1.0 / fit_zoom, anchor, center, fit_zoom);
                    } else {
                        self.fit = true;
                        self.offset = Vec2::ZERO;
                    }
                } else if response.dragged() {
                    if self.fit {
                        self.zoom = fit_zoom;
                        self.fit = false;
                    }
                    self.offset += response.drag_delta();
                }

                let zoom = cur_zoom(self);
                img_rect = Rect::from_center_size(center + self.offset, img_pts * zoom);

                // Malla con la textura; para girar, se dibuja sin girar y se rota la malla.
                let draw_size = if rotated {
                    Vec2::new(img_rect.height(), img_rect.width())
                } else {
                    img_rect.size()
                };
                let mut mesh = Mesh::with_texture(tex.id());
                mesh.add_rect_with_uv(
                    Rect::from_center_size(img_rect.center(), draw_size),
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
                if self.rotation != 0 {
                    let angle = self.rotation as f32 * std::f32::consts::FRAC_PI_2;
                    mesh.rotate(Rot2::from_angle(angle), img_rect.center());
                }
                painter.add(Shape::mesh(mesh));

                if self.first_paint_ms.is_none() {
                    self.first_paint_ms = Some(self.launched.elapsed().as_secs_f64() * 1000.0);
                }

                status = format!("{w} × {h}    {:.0} %", zoom * 100.0);
                if self.rotation != 0 {
                    status += &format!("    ↻ {}°", self.rotation as u32 * 90);
                }
                if self.pixel {
                    status += "    ▦ píxeles";
                }
                if self.slideshow.is_some() {
                    status += "    ▶ presentación";
                }

                if self.show_info {
                    let mut text = format_table(&info.general);
                    if let Some(ms) = self.first_paint_ms {
                        text += &format!("\nArranque → 1ª imagen  {ms:.0} ms");
                    }
                    text += "\n\n── EXIF ──\n";
                    if info.exif.is_empty() {
                        text += "(sin metadatos EXIF)";
                    } else {
                        text += &format_table(&info.exif);
                    }
                    info_text = Some(text);
                }
                let _ = info.decode_ms;
            }
            Some(Err(e)) => {
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    format!("No se pudo abrir la imagen:\n{e}"),
                    FontId::proportional(18.0),
                    Color32::from_gray(200),
                );
            }
            None if self.files.is_empty() => {
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    "PicaRust\n\nUso: picarust <imagen>\no arrastra una imagen aquí\n\nH ayuda · Esc salir",
                    FontId::proportional(20.0),
                    Color32::from_gray(200),
                );
            }
            None => {
                // Cargando: no se pinta nada para que la aparición sea limpia.
            }
        }

        // Cerrar al hacer clic fuera de la imagen (como Picasa).
        if response.clicked() && !response.double_clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                if !img_rect.contains(pos) && !self.files.is_empty() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }

        // Barra de estado inferior.
        if let Some(path) = self.current() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let text = format!(
                "{name}    {}/{}    {status}",
                self.index + 1,
                self.files.len()
            );
            panel(
                &painter,
                Pos2::new(center.x, screen.bottom() - 10.0),
                egui::Align2::CENTER_BOTTOM,
                text,
                FontId::proportional(14.0),
            );
        }

        if let Some(text) = info_text {
            panel(
                &painter,
                screen.right_top() + Vec2::new(-16.0, 16.0),
                egui::Align2::RIGHT_TOP,
                text,
                FontId::monospace(13.0),
            );
        }

        if self.show_help {
            panel(
                &painter,
                screen.left_top() + Vec2::new(16.0, 16.0),
                egui::Align2::LEFT_TOP,
                HELP.to_string(),
                FontId::monospace(14.0),
            );
        }
    }
}
