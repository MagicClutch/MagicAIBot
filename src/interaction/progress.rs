use std::time::{Duration, SystemTime};
pub fn estimated_progress(start: SystemTime, timeout: Duration) -> u8 {
    let elapsed = start.elapsed().unwrap_or_default().as_secs_f64();
    ((elapsed / timeout.as_secs_f64().max(0.001) * 95.0) as u8).min(95)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progress_is_bounded() {
        assert!(estimated_progress(SystemTime::now(), Duration::from_secs(1)) <= 95);
    }
}
