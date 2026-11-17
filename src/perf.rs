//! Frame-time statistics for the performance HUD and benchmarks.

use std::collections::VecDeque;

/// Rolling window of frame CPU times in milliseconds.
pub struct FrameStats {
    samples: VecDeque<f32>,
    cap: usize,
    pub frames: u64,
}

impl Default for FrameStats {
    fn default() -> Self {
        Self::new(600)
    }
}

impl FrameStats {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(cap),
            cap,
            frames: 0,
        }
    }

    pub fn record(&mut self, ms: f32) {
        if self.samples.len() == self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
        self.frames += 1;
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    /// Nearest-rank percentile (0..=100) of the window.
    pub fn percentile(&self, p: f32) -> f32 {
        percentile(self.samples.iter().copied(), p)
    }

    pub fn max(&self) -> f32 {
        self.samples.iter().copied().fold(0.0, f32::max)
    }
}

pub fn percentile(values: impl Iterator<Item = f32>, p: f32) -> f32 {
    let mut v: Vec<f32> = values.collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let rank = ((p / 100.0) * v.len() as f32).ceil() as usize;
    v[rank.clamp(1, v.len()) - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles() {
        let mut s = FrameStats::new(100);
        for i in 1..=100 {
            s.record(i as f32);
        }
        assert_eq!(s.percentile(50.0), 50.0);
        assert_eq!(s.percentile(95.0), 95.0);
        assert_eq!(s.percentile(100.0), 100.0);
        assert_eq!(s.max(), 100.0);
        s.record(1000.0);
        assert_eq!(s.len(), 100);
    }
}
