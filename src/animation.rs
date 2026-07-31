use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;

pub(crate) struct AnimationPlayer {
    rx: mpsc::Receiver<DecodedFrame>,
    ctx: egui::Context,
    name: String,
    texture: Option<egui::TextureHandle>,
    next_frame_at: Option<Instant>,
}

pub(crate) enum AnimationPoll {
    Unchanged,
    NewTexture(egui::TextureHandle),
    Finished,
}

struct DecodedFrame {
    image: egui::ColorImage,
    delay: Duration,
}

const MIN_FRAME_DELAY: Duration = Duration::from_millis(10);

impl AnimationPlayer {
    pub(crate) fn new(path: PathBuf, ctx: &egui::Context) -> Self {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        // A rendezvous channel lets the worker decode one frame ahead,
        // then blocks until the UI is ready to display it.
        let (frame_tx, frame_rx) = mpsc::sync_channel(0);

        std::thread::spawn(move || animation_worker(path, frame_tx));

        Self {
            rx: frame_rx,
            ctx: ctx.clone(),
            name,
            texture: None,
            next_frame_at: None,
        }
    }

    pub(crate) fn poll(&mut self) -> AnimationPoll {
        let now = Instant::now();
        let frame_due = self.next_frame_at.is_none_or(|deadline| now >= deadline);

        let result = if frame_due {
            match self.rx.try_recv() {
                Ok(DecodedFrame { image, delay }) => {
                    // Stay on the original timeline instead of accumulating
                    // decode, render, and wake-up lateness every frame.
                    self.next_frame_at = Some(self.next_frame_at.unwrap_or(now) + delay);
                    if let Some(texture) = &mut self.texture {
                        texture.set(image, egui::TextureOptions::LINEAR);
                        AnimationPoll::Unchanged
                    } else {
                        let texture =
                            self.ctx
                                .load_texture(&self.name, image, egui::TextureOptions::LINEAR);
                        self.texture = Some(texture.clone());
                        AnimationPoll::NewTexture(texture)
                    }
                }
                Err(mpsc::TryRecvError::Empty) => AnimationPoll::Unchanged,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return AnimationPoll::Finished;
                }
            }
        } else {
            AnimationPoll::Unchanged
        };

        match self.next_frame_at {
            Some(next_frame_at) => self
                .ctx
                .request_repaint_after(next_frame_at.saturating_duration_since(Instant::now())),
            None => self.ctx.request_repaint(),
        }
        result
    }
}

fn animation_worker(path: PathBuf, tx: mpsc::SyncSender<DecodedFrame>) {
    loop {
        let frames = match crate::file_io::open_animation_frames(&path) {
            Ok(Some(frames)) => frames,
            Ok(None) => return,
            Err(e) => {
                log::warn!("Animation decode failed for {}: {}", path.display(), e);
                return;
            }
        };
        let mut frame_count = 0;

        for frame in frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(e) => {
                    log::warn!(
                        "Animation frame decode failed for {}: {}",
                        path.display(),
                        e
                    );
                    return;
                }
            };
            frame_count += 1;
            let decoded = DecodedFrame {
                delay: frame_delay(frame.delay()),
                image: crate::decode::image_to_color_image(image::DynamicImage::ImageRgba8(
                    frame.into_buffer(),
                )),
            };
            if tx.send(decoded).is_err() {
                return;
            }
        }

        if frame_count < 2 {
            return;
        }
    }
}

fn frame_delay(delay: image::Delay) -> Duration {
    Duration::from(delay).max(MIN_FRAME_DELAY)
}
