//! Process-local sticky hop and cooldown state (no secrets).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::classify::FailoverReason;

#[derive(Debug, Clone)]
struct Cooldown {
    until: Instant,
    reason: FailoverReason,
}

/// In-memory fallback state for one Bandicot process.
#[derive(Debug, Default)]
pub struct FallbackState {
    sticky_hop_id: Option<String>,
    sticky_until: Option<Instant>,
    cooldowns: HashMap<String, Cooldown>,
}

impl FallbackState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn mark_success(&mut self, hop_id: &str, sticky_ttl_secs: u64) {
        self.cooldowns.remove(hop_id);
        if sticky_ttl_secs == 0 {
            self.sticky_hop_id = None;
            self.sticky_until = None;
            return;
        }
        self.sticky_hop_id = Some(hop_id.to_owned());
        self.sticky_until = Some(Instant::now() + Duration::from_secs(sticky_ttl_secs));
    }

    pub fn mark_exhausted(
        &mut self,
        hop_id: &str,
        reason: FailoverReason,
        retry_after_secs: Option<u64>,
    ) {
        if self.sticky_hop_id.as_deref() == Some(hop_id) {
            self.sticky_hop_id = None;
            self.sticky_until = None;
        }
        let secs = retry_after_secs.unwrap_or(default_cooldown_secs(reason));
        self.cooldowns.insert(
            hop_id.to_owned(),
            Cooldown {
                until: Instant::now() + Duration::from_secs(secs.max(1)),
                reason,
            },
        );
    }

    pub fn sticky_hop_id(&self) -> Option<&str> {
        let until = self.sticky_until?;
        if Instant::now() >= until {
            return None;
        }
        self.sticky_hop_id.as_deref()
    }

    pub fn is_cooling_down(&self, hop_id: &str) -> bool {
        self.cooldowns
            .get(hop_id)
            .is_some_and(|c| Instant::now() < c.until)
    }

    pub fn cooldown_reason(&self, hop_id: &str) -> Option<FailoverReason> {
        let c = self.cooldowns.get(hop_id)?;
        if Instant::now() >= c.until {
            return None;
        }
        Some(c.reason)
    }

    /// Drop expired cooldowns (best-effort hygiene).
    pub fn gc(&mut self) {
        let now = Instant::now();
        self.cooldowns.retain(|_, c| c.until > now);
        if self.sticky_until.is_some_and(|u| now >= u) {
            self.sticky_hop_id = None;
            self.sticky_until = None;
        }
    }
}

fn default_cooldown_secs(reason: FailoverReason) -> u64 {
    match reason {
        FailoverReason::RateLimited => 60,
        FailoverReason::QuotaExhausted => 900,
        FailoverReason::Unauthorized => 300,
        FailoverReason::ServerError => 30,
        FailoverReason::MissingCredential => 3_600,
        FailoverReason::CapabilityMismatch => 3_600,
        FailoverReason::ContextTooLarge => 3_600,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_and_cooldown() {
        let mut s = FallbackState::new();
        s.mark_success("go-1", 60);
        assert_eq!(s.sticky_hop_id(), Some("go-1"));
        s.mark_exhausted("go-1", FailoverReason::RateLimited, Some(120));
        assert!(s.is_cooling_down("go-1"));
        assert_eq!(s.sticky_hop_id(), None);
    }
}
