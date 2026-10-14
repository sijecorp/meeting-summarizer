//! The recording's reference clock and the alignment rules (architecture 4.2).
//! Everything except `now` is pure, so the rules are tested with made-up clock readings.

/// Output sample rate of both tracks.
pub const RATE: u64 = 16_000;
/// 100 ms: how far a track may drift from the clock before it is corrected (REC-03).
pub const TOLERANCE: u64 = 1_600;
/// Samples running ahead are dropped only at this interval.
pub const DROP_EVERY_MS: u64 = 10_000;
/// A longer sleep ends the recording at the sleep point (REC-06).
pub const MAX_SLEEP_MS: u64 = 5 * 60_000;
/// Reading two clocks one after the other is not perfectly simultaneous, and Windows
/// `GetTickCount64` only ticks every ~16 ms. Below this, a difference is noise, not sleep.
const SLEEP_NOISE_MS: u64 = 1_000;

/// Two OS clocks read together, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// Stops while the system sleeps. The recording's timeline.
    pub awake_ms: u64,
    /// Keeps counting while the system sleeps.
    pub total_ms: u64,
}

#[cfg(target_os = "macos")]
pub fn now() -> Reading {
    fn ms(clock: libc::clockid_t) -> u64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(clock, &mut ts) };
        ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000
    }
    // CLOCK_UPTIME_RAW is mach_absolute_time, CLOCK_MONOTONIC is mach_continuous_time.
    Reading {
        awake_ms: ms(libc::CLOCK_UPTIME_RAW),
        total_ms: ms(libc::CLOCK_MONOTONIC),
    }
}

#[cfg(windows)]
pub fn now() -> Reading {
    use windows_sys::Win32::System::SystemInformation::GetTickCount64;
    use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;
    let mut unbiased = 0u64; // 100 ns units, sleep and hibernation excluded
    unsafe { QueryUnbiasedInterruptTime(&mut unbiased) };
    Reading {
        awake_ms: unbiased / 10_000,
        total_ms: unsafe { GetTickCount64() },
    }
}

/// Keeps one track on the reference clock (REC-03).
pub struct Aligner {
    /// Samples written so far, filled silence included.
    pub written: u64,
    /// Incoming samples still to discard because the track ran ahead.
    to_drop: u64,
    next_drop_check_ms: u64,
}

impl Default for Aligner {
    fn default() -> Self {
        Self {
            written: 0,
            to_drop: 0,
            next_drop_check_ms: DROP_EVERY_MS,
        }
    }
}

impl Aligner {
    /// Of `n` incoming samples, how many to write. The rest pay off an earlier overflow and are
    /// dropped from the front.
    pub fn keep(&mut self, n: u64) -> u64 {
        let dropped = n.min(self.to_drop);
        self.to_drop -= dropped;
        self.written += n - dropped;
        n - dropped
    }

    /// Call after writing what `keep` allowed. Returns how many zeros the caller must write now:
    /// a track more than 100 ms behind is filled up to the clock. Every 10 s, a track more than
    /// 100 ms ahead has the excess dropped from its next samples.
    pub fn check(&mut self, elapsed_ms: u64) -> u64 {
        let expected = elapsed_ms * RATE / 1000;
        if expected > self.written + TOLERANCE {
            let fill = expected - self.written;
            self.written = expected;
            self.to_drop = 0;
            return fill;
        }
        if elapsed_ms >= self.next_drop_check_ms {
            self.next_drop_check_ms = elapsed_ms - elapsed_ms % DROP_EVERY_MS + DROP_EVERY_MS;
            let ahead = self.written.saturating_sub(expected);
            self.to_drop = if ahead > TOLERANCE { ahead } else { 0 };
        }
        0
    }
}

#[derive(Debug, PartialEq)]
pub enum Sleep {
    Awake,
    /// At most 5 minutes: keep recording and note the gap.
    Short(u64),
    /// Over 5 minutes: end the recording at the sleep point.
    Long(u64),
}

/// Sleep is measured from the OS clocks only, never from missing audio (architecture 4.2):
/// it is how much more the sleep-inclusive clock advanced than the reference clock.
pub struct SleepWatch {
    start: Reading,
    reported_ms: u64,
}

impl SleepWatch {
    pub fn new(start: Reading) -> Self {
        Self {
            start,
            reported_ms: 0,
        }
    }

    /// Sleep that ended since the last call.
    pub fn poll(&mut self, now: Reading) -> Sleep {
        let slept =
            (now.total_ms - self.start.total_ms).saturating_sub(now.awake_ms - self.start.awake_ms);
        let new = slept.saturating_sub(self.reported_ms);
        if new < SLEEP_NOISE_MS {
            return Sleep::Awake;
        }
        self.reported_ms = slept;
        if new > MAX_SLEEP_MS {
            Sleep::Long(new)
        } else {
            Sleep::Short(new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behind_by_more_than_100ms_is_filled_up_to_the_clock() {
        let mut a = Aligner::default();
        // 100 ms with nothing written: exactly the tolerance, no fill yet.
        assert_eq!(a.check(100), 0);
        // 1 ms later the track is 1,616 samples behind.
        assert_eq!(a.check(101), 1_616);
        assert_eq!(a.written, 1_616);
        // Data arriving later is written as usual.
        assert_eq!(a.keep(320), 320);
        assert_eq!(a.check(120), 0);
    }

    #[test]
    fn ahead_by_more_than_100ms_is_dropped_only_at_the_10s_check() {
        let mut a = Aligner::default();
        a.keep(9_999 * 16 + 2_000); // 2,000 samples ahead at 9.999 s
        assert_eq!(a.check(9_999), 0);
        assert_eq!(a.keep(100), 100, "not a check yet");

        assert_eq!(a.check(10_000), 0);
        // 162,084 written vs 160,000 expected: 2,084 ahead, dropped from what comes next.
        assert_eq!(a.keep(2_000), 0);
        assert_eq!(a.keep(500), 416);
        assert_eq!(a.keep(500), 500);
        // Back on the clock: 160,000 at the check plus the 3,000 that arrived after it.
        assert_eq!(a.written, 160_000 + 3_000);
    }

    #[test]
    fn ahead_by_exactly_100ms_is_kept() {
        let mut a = Aligner::default();
        a.keep(160_000 + TOLERANCE);
        assert_eq!(a.check(10_000), 0);
        assert_eq!(a.keep(10), 10);
    }

    #[test]
    fn falling_behind_cancels_a_pending_drop() {
        let mut a = Aligner::default();
        a.keep(160_000 + 5_000);
        a.check(10_000);
        // The device then stops: the clock passes the track, so nothing more may be dropped.
        assert!(a.check(10_500) > 0);
        assert_eq!(a.keep(100), 100);
    }

    /// REC-03: a device clock 0.1% off for 4 hours stays within 0.5 s of the reference.
    #[test]
    fn four_hours_with_a_drifting_device_stay_aligned() {
        for device_rate in [16_016.0, 15_984.0] {
            let mut a = Aligner::default();
            let mut produced = 0.0f64;
            let mut worst = 0i64;
            for tick in 1..=(4 * 3_600 * 50u64) {
                let elapsed_ms = tick * 20;
                let target = elapsed_ms as f64 / 1000.0 * device_rate;
                let n = (target - produced).floor();
                produced += n;
                a.keep(n as u64);
                a.check(elapsed_ms);
                worst = worst.max((a.written as i64 - (elapsed_ms * 16) as i64).abs());
            }
            assert!(worst <= 8_000, "{device_rate}: off by {worst} samples");
        }
    }

    fn at(awake_ms: u64, total_ms: u64) -> Reading {
        Reading { awake_ms, total_ms }
    }

    #[test]
    fn sleep_up_to_5_minutes_continues_and_longer_ends() {
        let mut w = SleepWatch::new(at(1_000, 5_000));
        assert_eq!(w.poll(at(2_000, 6_000)), Sleep::Awake);
        // Asleep for exactly 5 minutes between two polls.
        assert_eq!(
            w.poll(at(3_000, 7_000 + MAX_SLEEP_MS)),
            Sleep::Short(MAX_SLEEP_MS)
        );
        // Already reported: not counted again.
        assert_eq!(w.poll(at(4_000, 8_000 + MAX_SLEEP_MS)), Sleep::Awake);
        assert_eq!(
            w.poll(at(5_000, 9_000 + 2 * MAX_SLEEP_MS + 1)),
            Sleep::Long(MAX_SLEEP_MS + 1)
        );
    }

    #[test]
    fn clock_read_jitter_is_not_sleep() {
        let mut w = SleepWatch::new(at(0, 0));
        assert_eq!(w.poll(at(10_000, 10_999)), Sleep::Awake);
        assert_eq!(w.poll(at(20_000, 20_016)), Sleep::Awake);
    }

    /// A 10-minute data gap (mic unplugged, Windows loopback silent) moves both clocks equally:
    /// it is filled with silence and never mistaken for sleep.
    #[test]
    fn data_gap_is_filled_not_counted_as_sleep() {
        let mut w = SleepWatch::new(at(0, 0));
        let mut a = Aligner::default();
        a.keep(16_000);
        assert_eq!(a.check(1_000), 0);
        for s in 2..=601 {
            assert_eq!(w.poll(at(s * 1_000, s * 1_000)), Sleep::Awake);
            a.check(s * 1_000);
        }
        assert_eq!(a.written, 601 * 16_000);
    }
}
