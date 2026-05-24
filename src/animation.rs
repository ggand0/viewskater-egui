use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

pub(crate) struct AnimationPlayer {
    tx: mpsc::Sender<()>,
    rx: mpsc::Receiver<WorkerMsg>,
    ctx: egui::Context,
    name: String,
    texture: Option<egui::TextureHandle>,
    next_frame_at: Instant,
    loading: bool,
}

pub(crate) enum AnimationPoll {
    None,
    Frame(egui::TextureHandle),
    Finished,
}

enum WorkerMsg {
    Frame(egui::ColorImage, Duration),
    Finished,
}

const MIN_FRAME_DELAY: Duration = Duration::from_millis(10);

impl AnimationPlayer {
    pub(crate) fn new(path: PathBuf, ctx: &egui::Context) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (request_tx, request_rx) = mpsc::channel();
        let (frame_tx, frame_rx) = mpsc::channel();
        let ctx = ctx.clone();
        let worker_ctx = ctx.clone();

        std::thread::spawn(move || animation_worker(path, request_rx, frame_tx, worker_ctx));

        let player = Self {
            tx: request_tx,
            rx: frame_rx,
            ctx,
            name,
            texture: None,
            next_frame_at: Instant::now(),
            loading: true,
        };
        let _ = player.tx.send(());
        player
    }

    pub(crate) fn poll(&mut self) -> AnimationPoll {
        let mut texture = None;
        while let Ok(msg) = self.rx.try_recv() {
            self.loading = false;
            let WorkerMsg::Frame(image, delay) = msg else {
                return AnimationPoll::Finished;
            };
            self.next_frame_at = Instant::now() + delay;
            texture = Some(match &mut self.texture {
                Some(texture) => {
                    texture.set(image, egui::TextureOptions::LINEAR);
                    texture.clone()
                }
                slot @ None => slot
                    .insert(self.ctx.load_texture(
                        &self.name,
                        image,
                        egui::TextureOptions::LINEAR,
                    ))
                    .clone(),
            });
        }
        let now = Instant::now();
        if !self.loading && now >= self.next_frame_at {
            self.loading = self.tx.send(()).is_ok();
            if !self.loading {
                return AnimationPoll::Finished;
            }
        }
        if self.loading {
            self.ctx.request_repaint();
        } else {
            self.ctx
                .request_repaint_after(self.next_frame_at.saturating_duration_since(now));
        }
        texture.map_or(AnimationPoll::None, AnimationPoll::Frame)
    }
}

fn animation_worker(
    path: PathBuf,
    requests: mpsc::Receiver<()>,
    frames: mpsc::Sender<WorkerMsg>,
    ctx: egui::Context,
) {
    let mut animation = None;
    let mut seen_frames = 0;

    while requests.recv().is_ok() {
        loop {
            let iter = match animation.as_mut() {
                Some(iter) => iter,
                None => match crate::file_io::open_animation_frames(&path) {
                    Ok(Some(iter)) => animation.insert(iter),
                    Ok(None) => {
                        let _ = frames.send(WorkerMsg::Finished);
                        return;
                    }
                    Err(e) => {
                        log::warn!("Animation decode failed for {}: {}", path.display(), e);
                        let _ = frames.send(WorkerMsg::Finished);
                        return;
                    }
                },
            };

            match iter.next() {
                Some(Ok(frame)) => {
                    seen_frames += 1;
                    let delay = frame_delay(frame.delay());
                    let image = crate::decode::image_to_color_image(
                        image::DynamicImage::ImageRgba8(frame.into_buffer()),
                    );
                    let _ = frames.send(WorkerMsg::Frame(image, delay));
                    ctx.request_repaint();
                    break;
                }
                Some(Err(e)) => {
                    log::warn!(
                        "Animation frame decode failed for {}: {}",
                        path.display(),
                        e
                    );
                    let _ = frames.send(WorkerMsg::Finished);
                    return;
                }
                None if seen_frames < 2 => {
                    let _ = frames.send(WorkerMsg::Finished);
                    return;
                }
                None => {
                    animation = None;
                    seen_frames = 0;
                }
            }
        }
    }
}

fn frame_delay(delay: image::Delay) -> Duration {
    let (num, denom) = delay.numer_denom_ms();
    if denom == 0 {
        return MIN_FRAME_DELAY;
    }
    let millis = u64::from(num).div_ceil(u64::from(denom));
    Duration::from_millis(millis).max(MIN_FRAME_DELAY)
}
