use eframe::egui;

#[derive(Clone, Copy)]
pub(crate) struct ViewTransform {
    pub(crate) zoom: f32,
    pub(crate) pan: egui::Vec2,
}

impl ViewTransform {
    pub(crate) fn new(zoom: f32, pan: egui::Vec2) -> Self {
        Self { zoom, pan }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AnimationSample {
    pub(crate) transform: ViewTransform,
    pub(crate) done: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Easing {
    EaseOutCubic,
}

impl Easing {
    fn sample(self, t: f32) -> f32 {
        match self {
            Self::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ViewAnimation {
    from: ViewTransform,
    to: ViewTransform,
    started_at: f64,
    duration: f64,
    easing: Easing,
}

impl ViewAnimation {
    pub(crate) fn new(
        from: ViewTransform,
        to: ViewTransform,
        started_at: f64,
        duration: f64,
        easing: Easing,
    ) -> Self {
        Self {
            from,
            to,
            started_at,
            duration,
            easing,
        }
    }

    pub(crate) fn sample(&self, now: f64) -> AnimationSample {
        let t = ((now - self.started_at) / self.duration).clamp(0.0, 1.0) as f32;

        if t >= 1.0 {
            return AnimationSample {
                transform: self.to,
                done: true,
            };
        }

        let eased = self.easing.sample(t);
        AnimationSample {
            transform: ViewTransform {
                zoom: self.from.zoom + (self.to.zoom - self.from.zoom) * eased,
                pan: self.from.pan + (self.to.pan - self.from.pan) * eased,
            },
            done: false,
        }
    }
}
