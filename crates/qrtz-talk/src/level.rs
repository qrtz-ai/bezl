//! ADR 0112: a content-free summary of how loud the mic was during one dictation, for the `talk latency` line.
//!
//! The helper sends `dictation.level{rms}` about once per tap block while a dictation listens: the block's RMS
//! times 4, clamped to 1 (`Dictation.swift`, `feed`), so a value of 1 means the block was at or above -12 dBFS.
//! The meter keeps a fixed 1 dB histogram of those blocks (constant memory however long the hold) and reports the
//! 10th and 90th percentile in dBFS: p90 is about the speech level, p10 about the room's floor, and their
//! difference a rough signal-to-noise ratio. The Linux cross-check (docs/echo/dictation-tuning.md §10) says what
//! levels cost accuracy: a quiet mic (speech 20 dB down) and a 15 dB SNR both raise word errors by a quarter or
//! more. No audio, no words: two numbers and a block count.

/// The helper's scale: `rms * LEVEL_GAIN`, clamped to 1.
pub const LEVEL_GAIN: f64 = 4.0;
/// The lowest bin; quieter blocks (and digital silence) count here.
pub const FLOOR_DBFS: i32 = -100;
/// The level at which the helper's value saturates (20·log10(1/4) ≈ -12.04 dBFS).
pub const CEIL_DBFS: f64 = -12.041_199_826_559_248;

const BINS: usize = 89; // -100 ..= -12 dBFS, 1 dB each

/// Per-dictation level histogram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelMeter {
    bins: [u32; BINS],
    blocks: u32,
    saturated: u32,
}

impl Default for LevelMeter {
    fn default() -> Self {
        Self {
            bins: [0; BINS],
            blocks: 0,
            saturated: 0,
        }
    }
}

/// What the `talk latency` line carries.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelSummary {
    /// Blocks seen.
    pub blocks: u32,
    /// 10th percentile, dBFS (whole dB): the floor between words.
    pub p10_dbfs: i32,
    /// 90th percentile, dBFS (whole dB): about the speech level.
    pub p90_dbfs: i32,
    /// Blocks at the helper's clamp (≥ -12 dBFS); a loud mic, not necessarily clipping.
    pub saturated: u32,
}

/// One helper level value in dBFS (the block's RMS), `None` when it is not a number.
pub fn helper_level_dbfs(level: f64) -> Option<f64> {
    if !level.is_finite() {
        return None;
    }
    let rms = level.clamp(0.0, 1.0) / LEVEL_GAIN;
    Some(if rms <= 0.0 {
        f64::from(FLOOR_DBFS)
    } else {
        (20.0 * rms.log10()).max(f64::from(FLOOR_DBFS))
    })
}

impl LevelMeter {
    /// Count one `dictation.level{rms}` value. Not-a-number values are ignored.
    pub fn add(&mut self, level: f64) {
        let Some(db) = helper_level_dbfs(level) else {
            return;
        };
        if level >= 1.0 {
            self.saturated += 1;
        }
        let i = ((db.floor() as i32 - FLOOR_DBFS).clamp(0, BINS as i32 - 1)) as usize;
        self.bins[i] += 1;
        self.blocks = self.blocks.saturating_add(1);
    }

    pub fn blocks(&self) -> u32 {
        self.blocks
    }

    /// The bin (as its lower edge, dBFS) holding the q-th quantile (nearest rank).
    fn quantile(&self, q: f64) -> i32 {
        let rank = ((q * f64::from(self.blocks)).ceil() as u32).clamp(1, self.blocks);
        let mut seen = 0u32;
        for (i, n) in self.bins.iter().enumerate() {
            seen += n;
            if seen >= rank {
                return FLOOR_DBFS + i as i32;
            }
        }
        FLOOR_DBFS + BINS as i32 - 1
    }

    /// `None` when no level arrived (a tap of the key, or a helper that sends none).
    pub fn summary(&self) -> Option<LevelSummary> {
        (self.blocks > 0).then(|| LevelSummary {
            blocks: self.blocks,
            p10_dbfs: self.quantile(0.10),
            p90_dbfs: self.quantile(0.90),
            saturated: self.saturated,
        })
    }
}
