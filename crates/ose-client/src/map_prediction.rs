//! Cache map-only encounter searches independently of camera animation.
use glam::DVec3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub vessel: u64,
    pub target: u64,
    pub body: u16,
    pub warp: u32,
    pub vessel_prediction: u64,
    pub target_prediction: u64,
}

#[derive(Clone, Copy)]
pub struct Approach {
    /// Positions relative to the common reference body, in metres.
    pub vessel: DVec3,
    pub target: DVec3,
    pub distance: f64,
    pub ut: f64,
}

#[derive(Default)]
pub struct Cache {
    key: Option<Key>,
    expires_ms: f64,
    computed_ut: f64,
    value: Option<Approach>,
}

impl Cache {
    pub fn sample(
        &mut self,
        key: Key,
        now_ms: f64,
        ut: f64,
        compute: impl FnOnce() -> Option<Approach>,
    ) -> Option<Approach> {
        if self.key != Some(key) || now_ms >= self.expires_ms || ut < self.computed_ut {
            self.value = compute();
            self.key = Some(key);
            self.expires_ms = now_ms + 500.0;
            self.computed_ut = ut;
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_frames_reuse_search_but_target_prediction_warp_and_reload_invalidate() {
        let mut cache = Cache::default();
        let mut key = Key {
            vessel: 1,
            target: 2,
            body: 1,
            warp: 1,
            vessel_prediction: 4,
            target_prediction: 5,
        };
        let calls = std::cell::Cell::new(0);
        let compute = || {
            calls.set(calls.get() + 1);
            None
        };
        for now in [0.0, 10.0, 100.0, 499.0] {
            cache.sample(key, now, now / 1000.0, compute);
        }
        assert_eq!(calls.get(), 1);
        cache.sample(key, 500.0, 0.5, compute);
        assert_eq!(calls.get(), 2);
        key.target = 3;
        cache.sample(key, 501.0, 0.501, compute);
        key.vessel_prediction += 1;
        cache.sample(key, 502.0, 0.502, compute);
        key.target_prediction += 1;
        cache.sample(key, 503.0, 0.503, compute);
        key.warp = 4;
        cache.sample(key, 504.0, 0.504, compute);
        cache.sample(key, 505.0, 0.0, compute);
        assert_eq!(calls.get(), 7);
    }
}
