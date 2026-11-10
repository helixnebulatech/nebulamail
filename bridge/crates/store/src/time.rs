// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::StoreError;

/// A point in time as Unix seconds (UTC), the way the database stores it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixTime(i64);

impl UnixTime {
    /// The current time.
    ///
    /// # Errors
    /// [`StoreError::Clock`] if the system clock is before 1970.
    pub fn now() -> Result<Self, StoreError> {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StoreError::Clock)?;
        i64::try_from(since_epoch.as_secs())
            .map(Self)
            .map_err(|_| StoreError::Clock)
    }

    /// The time from Unix seconds.
    #[must_use]
    pub const fn from_secs(secs: i64) -> Self {
        Self(secs)
    }

    /// Unix seconds.
    #[must_use]
    pub fn secs(self) -> i64 {
        self.0
    }

    /// This time plus `duration` (whole seconds), saturating at the maximum.
    #[must_use]
    pub fn saturating_add(self, duration: Duration) -> Self {
        let secs = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
        Self(self.0.saturating_add(secs))
    }

    /// This time minus `duration` (whole seconds), saturating at the minimum.
    #[must_use]
    pub fn saturating_sub(self, duration: Duration) -> Self {
        let secs = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
        Self(self.0.saturating_sub(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_saturates() {
        let t = UnixTime::from_secs(i64::MAX - 1);
        assert_eq!(t.saturating_add(Duration::from_secs(10)).secs(), i64::MAX);
        assert_eq!(
            UnixTime::from_secs(100)
                .saturating_sub(Duration::from_secs(40))
                .secs(),
            60
        );
    }

    #[test]
    fn now_is_after_2026() {
        assert!(UnixTime::now().unwrap().secs() > 1_767_225_600);
    }
}
