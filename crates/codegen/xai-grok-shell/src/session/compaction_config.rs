//! Compaction configuration and runtime state for the session actor.

use std::cell::Cell;
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

/// Whether a context policy is being applied to the root session or to an
/// isolated subagent session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSessionKind {
    Main,
    Subagent,
}

/// Token limits and retention controls for one class of session.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextBand {
    pub logical_window_tokens: u64,
    pub soft_trigger_ratio: f64,
    pub summary_max_output_tokens: u64,
    pub post_compaction_target_tokens: u64,
    pub keep_recent_completed_tool_batches: usize,
    pub max_inline_tool_result_tokens: u64,
    pub max_inline_tool_result_bytes: u64,
}

impl ContextBand {
    /// Apply the harness ceiling to verified provider and optional gateway
    /// limits. Capability discovery must supply the verified native limit;
    /// callers must not guess one here.
    pub fn effective_window_tokens(
        &self,
        verified_native_window_tokens: u64,
        gateway_or_session_cap_tokens: Option<u64>,
    ) -> u64 {
        gateway_or_session_cap_tokens.map_or_else(
            || {
                self.logical_window_tokens
                    .min(verified_native_window_tokens)
            },
            |gateway_cap| {
                self.logical_window_tokens
                    .min(verified_native_window_tokens)
                    .min(gateway_cap)
            },
        )
    }

    /// Floor the configured ratio against the effective window. Invalid ratios
    /// fail closed to an immediate trigger instead of silently granting more
    /// context than the policy allows.
    pub fn soft_trigger_tokens(&self, effective_window_tokens: u64) -> u64 {
        if !self.soft_trigger_ratio.is_finite() || self.soft_trigger_ratio <= 0.0 {
            return 0;
        }
        if self.soft_trigger_ratio >= 1.0 {
            return effective_window_tokens;
        }
        (effective_window_tokens as f64 * self.soft_trigger_ratio).floor() as u64
    }
}

/// Provider-neutral context policy. Provider adapters still own verified model
/// capabilities and rendered token measurement.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextPolicy {
    pub main: ContextBand,
    pub subagent: ContextBand,
    pub normal_max_output_tokens: u64,
    pub minimum_output_reserve_tokens: u64,
    pub hard_safety_margin_tokens: u64,
    pub max_pending_model_calls: u8,
}

impl Default for ContextPolicy {
    fn default() -> Self {
        Self {
            main: ContextBand {
                logical_window_tokens: 258_000,
                soft_trigger_ratio: 0.50,
                summary_max_output_tokens: 8_192,
                post_compaction_target_tokens: 48_000,
                keep_recent_completed_tool_batches: 3,
                max_inline_tool_result_tokens: 16_384,
                max_inline_tool_result_bytes: 65_536,
            },
            subagent: ContextBand {
                logical_window_tokens: 128_000,
                soft_trigger_ratio: 0.50,
                summary_max_output_tokens: 4_096,
                post_compaction_target_tokens: 24_000,
                keep_recent_completed_tool_batches: 2,
                max_inline_tool_result_tokens: 8_192,
                max_inline_tool_result_bytes: 32_768,
            },
            normal_max_output_tokens: 32_768,
            minimum_output_reserve_tokens: 8_192,
            hard_safety_margin_tokens: 8_192,
            max_pending_model_calls: 1,
        }
    }
}

impl ContextPolicy {
    pub fn band(&self, kind: ContextSessionKind) -> &ContextBand {
        match kind {
            ContextSessionKind::Main => &self.main,
            ContextSessionKind::Subagent => &self.subagent,
        }
    }

    pub fn pressure(
        &self,
        kind: ContextSessionKind,
        rendered_input_tokens: u64,
        verified_native_window_tokens: u64,
        gateway_or_session_cap_tokens: Option<u64>,
    ) -> ContextPressure {
        let band = self.band(kind);
        let effective_window_tokens = band
            .effective_window_tokens(verified_native_window_tokens, gateway_or_session_cap_tokens);
        let soft_trigger_tokens = band.soft_trigger_tokens(effective_window_tokens);
        let hard_guard = rendered_input_tokens
            .saturating_add(self.minimum_output_reserve_tokens)
            .saturating_add(self.hard_safety_margin_tokens)
            >= effective_window_tokens;
        ContextPressure {
            rendered_input_tokens,
            effective_window_tokens,
            soft_trigger_tokens,
            at_soft_trigger: rendered_input_tokens >= soft_trigger_tokens,
            hard_guard,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextPressure {
    pub rendered_input_tokens: u64,
    pub effective_window_tokens: u64,
    pub soft_trigger_tokens: u64,
    pub at_soft_trigger: bool,
    pub hard_guard: bool,
}

/// Safe-boundary state kept separately from the existing failure-suppression
/// atomics until the session actor is migrated to this universal policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CompactionBoundaryState {
    #[default]
    Healthy,
    Pending {
        crossed_at_tokens: u64,
        model_calls_since_crossing: u8,
    },
    Compacting,
    Suppressed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionPreflightAction {
    Proceed,
    CompactNow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactionBoundaryTransition {
    pub state: CompactionBoundaryState,
    pub action: CompactionPreflightAction,
}

/// Pure preflight transition. Crossing the soft trigger marks the session
/// pending but permits one model sample. The hard guard always wins, including
/// over suppression.
pub fn transition_before_model_request(
    state: CompactionBoundaryState,
    pressure: ContextPressure,
    max_pending_model_calls: u8,
) -> CompactionBoundaryTransition {
    if pressure.hard_guard {
        return CompactionBoundaryTransition {
            state: CompactionBoundaryState::Compacting,
            action: CompactionPreflightAction::CompactNow,
        };
    }
    match state {
        CompactionBoundaryState::Healthy if pressure.at_soft_trigger => {
            CompactionBoundaryTransition {
                state: CompactionBoundaryState::Pending {
                    crossed_at_tokens: pressure.rendered_input_tokens,
                    model_calls_since_crossing: 0,
                },
                action: CompactionPreflightAction::Proceed,
            }
        }
        CompactionBoundaryState::Pending {
            model_calls_since_crossing,
            ..
        } if model_calls_since_crossing >= max_pending_model_calls => {
            CompactionBoundaryTransition {
                state: CompactionBoundaryState::Compacting,
                action: CompactionPreflightAction::CompactNow,
            }
        }
        _ => CompactionBoundaryTransition {
            state,
            action: CompactionPreflightAction::Proceed,
        },
    }
}

/// Record completion of a model sample while pending. Tool-producing samples
/// remain pending until their entire result batch is committed. A chat-only
/// sample advances the fallback counter checked by the next preflight.
pub fn transition_after_model_sample(
    state: CompactionBoundaryState,
    emitted_tool_calls: bool,
) -> CompactionBoundaryState {
    match state {
        CompactionBoundaryState::Pending {
            crossed_at_tokens,
            model_calls_since_crossing,
        } if !emitted_tool_calls => CompactionBoundaryState::Pending {
            crossed_at_tokens,
            model_calls_since_crossing: model_calls_since_crossing.saturating_add(1),
        },
        _ => state,
    }
}

/// Compact only after every member of the next tool batch has reached a
/// terminal state and its result has been committed.
pub fn transition_after_tool_batch_committed(
    state: CompactionBoundaryState,
    all_results_terminal_and_committed: bool,
) -> CompactionBoundaryTransition {
    if all_results_terminal_and_committed
        && matches!(state, CompactionBoundaryState::Pending { .. })
    {
        CompactionBoundaryTransition {
            state: CompactionBoundaryState::Compacting,
            action: CompactionPreflightAction::CompactNow,
        }
    } else {
        CompactionBoundaryTransition {
            state,
            action: CompactionPreflightAction::Proceed,
        }
    }
}

#[cfg(test)]
mod context_policy_tests {
    use super::*;

    #[test]
    fn defaults_match_universal_context_policy() {
        let policy = ContextPolicy::default();
        assert_eq!(policy.main.logical_window_tokens, 258_000);
        assert_eq!(policy.main.soft_trigger_tokens(258_000), 129_000);
        assert_eq!(policy.main.summary_max_output_tokens, 8_192);
        assert_eq!(policy.main.post_compaction_target_tokens, 48_000);
        assert_eq!(policy.main.keep_recent_completed_tool_batches, 3);
        assert_eq!(policy.main.max_inline_tool_result_tokens, 16_384);
        assert_eq!(policy.main.max_inline_tool_result_bytes, 65_536);

        assert_eq!(policy.subagent.logical_window_tokens, 128_000);
        assert_eq!(policy.subagent.soft_trigger_tokens(128_000), 64_000);
        assert_eq!(policy.subagent.summary_max_output_tokens, 4_096);
        assert_eq!(policy.subagent.post_compaction_target_tokens, 24_000);
        assert_eq!(policy.subagent.keep_recent_completed_tool_batches, 2);
        assert_eq!(policy.subagent.max_inline_tool_result_tokens, 8_192);
        assert_eq!(policy.subagent.max_inline_tool_result_bytes, 32_768);

        assert_eq!(policy.normal_max_output_tokens, 32_768);
        assert_eq!(policy.minimum_output_reserve_tokens, 8_192);
        assert_eq!(policy.hard_safety_margin_tokens, 8_192);
        assert_eq!(policy.max_pending_model_calls, 1);
    }

    #[test]
    fn exact_main_and_subagent_soft_boundaries() {
        let policy = ContextPolicy::default();
        let main_below = policy.pressure(ContextSessionKind::Main, 128_999, 300_000, None);
        let main_at = policy.pressure(ContextSessionKind::Main, 129_000, 300_000, None);
        assert!(!main_below.at_soft_trigger);
        assert!(main_at.at_soft_trigger);

        let sub_below = policy.pressure(ContextSessionKind::Subagent, 63_999, 300_000, None);
        let sub_at = policy.pressure(ContextSessionKind::Subagent, 64_000, 300_000, None);
        assert!(!sub_below.at_soft_trigger);
        assert!(sub_at.at_soft_trigger);
    }

    #[test]
    fn lower_native_and_gateway_caps_reduce_effective_window_and_trigger() {
        let policy = ContextPolicy::default();
        let native_limited = policy.pressure(ContextSessionKind::Main, 99_999, 200_000, None);
        assert_eq!(native_limited.effective_window_tokens, 200_000);
        assert_eq!(native_limited.soft_trigger_tokens, 100_000);
        assert!(!native_limited.at_soft_trigger);

        let gateway_limited =
            policy.pressure(ContextSessionKind::Main, 64_000, 300_000, Some(128_000));
        assert_eq!(gateway_limited.effective_window_tokens, 128_000);
        assert_eq!(gateway_limited.soft_trigger_tokens, 64_000);
        assert!(gateway_limited.at_soft_trigger);
    }

    #[test]
    fn hard_guard_reserves_output_and_margin() {
        let policy = ContextPolicy::default();
        let below = policy.pressure(ContextSessionKind::Subagent, 111_615, 128_000, None);
        let at = policy.pressure(ContextSessionKind::Subagent, 111_616, 128_000, None);
        assert!(!below.hard_guard);
        assert!(at.hard_guard);
    }

    #[test]
    fn soft_crossing_waits_for_complete_tool_batch() {
        let policy = ContextPolicy::default();
        let pressure = policy.pressure(ContextSessionKind::Main, 129_000, 300_000, None);
        let crossed = transition_before_model_request(
            CompactionBoundaryState::Healthy,
            pressure,
            policy.max_pending_model_calls,
        );
        assert_eq!(crossed.action, CompactionPreflightAction::Proceed);
        assert_eq!(
            crossed.state,
            CompactionBoundaryState::Pending {
                crossed_at_tokens: 129_000,
                model_calls_since_crossing: 0,
            }
        );

        let incomplete = transition_after_tool_batch_committed(crossed.state, false);
        assert_eq!(incomplete.action, CompactionPreflightAction::Proceed);
        assert_eq!(incomplete.state, crossed.state);

        let complete = transition_after_tool_batch_committed(crossed.state, true);
        assert_eq!(complete.action, CompactionPreflightAction::CompactNow);
        assert_eq!(complete.state, CompactionBoundaryState::Compacting);
    }

    #[test]
    fn chat_only_pending_path_allows_one_sample_then_compacts() {
        let policy = ContextPolicy::default();
        let pressure = policy.pressure(ContextSessionKind::Main, 129_000, 300_000, None);
        let crossed = transition_before_model_request(
            CompactionBoundaryState::Healthy,
            pressure,
            policy.max_pending_model_calls,
        );
        let after_sample = transition_after_model_sample(crossed.state, false);
        assert_eq!(
            after_sample,
            CompactionBoundaryState::Pending {
                crossed_at_tokens: 129_000,
                model_calls_since_crossing: 1,
            }
        );
        let next =
            transition_before_model_request(after_sample, pressure, policy.max_pending_model_calls);
        assert_eq!(next.action, CompactionPreflightAction::CompactNow);
        assert_eq!(next.state, CompactionBoundaryState::Compacting);
    }

    #[test]
    fn hard_guard_overrides_pending_and_suppressed_states() {
        let policy = ContextPolicy::default();
        let pressure = policy.pressure(ContextSessionKind::Subagent, 120_000, 128_000, None);
        assert!(pressure.hard_guard);
        for state in [
            CompactionBoundaryState::Pending {
                crossed_at_tokens: 64_000,
                model_calls_since_crossing: 0,
            },
            CompactionBoundaryState::Suppressed,
        ] {
            let transition =
                transition_before_model_request(state, pressure, policy.max_pending_model_calls);
            assert_eq!(transition.action, CompactionPreflightAction::CompactNow);
            assert_eq!(transition.state, CompactionBoundaryState::Compacting);
        }
    }
}

/// Auto-compaction is gated whenever `auto_compact_suppressed` is not [`SUPPRESS_NONE`].
pub(crate) const SUPPRESS_NONE: u8 = 0;
/// Resolvable failure (`other`): suppressed for the current turn, then
/// cleared at the next turn start so compaction self-heals once the cause clears.
pub(crate) const SUPPRESS_TURN: u8 = 1;
/// Fatal failure (size/schema) retrying can never fix: survives turn boundaries,
/// cleared only when the context budget changes — a successful compaction, a
/// rewind (context shrank), or a model switch (a larger window may now fit).
pub(crate) const SUPPRESS_STICKY: u8 = 2;
/// Credit block: suppress until a model `200` (credits aren't client-observable).
/// Survives turns; context changes can't fix it. Token refresh must not clear this.
pub(crate) const SUPPRESS_UNTIL_SUCCESS: u8 = 3;
/// Auth-expired auto-compact: suppress until login/token refresh, not until 200
/// (waiting for a sample deadlocks when context is already over the window).
pub(crate) const SUPPRESS_AUTH: u8 = 4;

/// Model slug and context window from the previous turn.
#[derive(Clone, Debug)]
pub struct PreviousModelInfo {
    pub model_slug: String,
    pub context_window: u64,
}

/// Cached result of an **async** (background / prefire) pass-1 sample for
/// two-pass compaction. Held on the session actor between the background
/// pass-1 and the synchronous pass-2 apply at compaction time.
#[derive(Clone, Debug)]
pub struct AsyncCompactionCache {
    /// The successor-usable NOTE₁ text (extracted `<summary>` or full pass-1 output).
    pub note1: String,
    /// Number of leading conversation items pass-1 summarized (the prefix
    /// boundary in the LIVE conversation as of pass-1 time). The pass-2 tail is
    /// `conversation[prefix_len..]`.
    pub prefix_len: usize,
    /// Fingerprint of `conversation[..prefix_len]` at pass-1 time. Pass-2 only
    /// applies NOTE₁ when the current conversation still has this exact prefix.
    pub fingerprint: u64,
    /// Model slug pass-1 ran under; invalidated on model switch.
    pub model_slug: String,
    /// Wall time pass-1 took (ms) — latency that ran off the critical path
    /// when prefire finished before compact (not counted in telemetry TTFT unless
    /// the user waited on an in-flight pass-1).
    pub pass1_latency_ms: u64,
}

/// Prefire two-pass state. `Default` so it drops into existing `CompactionConfig`
/// struct literals with a single `prefire: PrefireState::default()` field.
///
/// `SessionActor` is `!Send` and single-threaded; the `AtomicBool` is only used
/// for its ergonomic `compare_exchange` (no cross-thread sharing), and the
/// `RefCell`s need no locking (the `JoinHandle` is from `spawn_local`, so it is
/// local to this LocalSet and never crosses threads).
#[derive(Default)]
pub struct PrefireState {
    /// Set while a background pass-1 sample is running, so the per-turn trigger
    /// never spawns a second concurrent job.
    in_flight: AtomicBool,
    /// Cached async pass-1 result, ready for pass-2 apply (or `None`).
    cache: RefCell<Option<AsyncCompactionCache>>,
    /// Handle to the in-flight background pass-1 task. Pass-2 awaits this when
    /// compaction fires before prefire finished, so a still-running pass-1 is
    /// used rather than discarded for a full single-pass.
    handle: RefCell<Option<tokio::task::JoinHandle<()>>>,
    /// Provider-neutral safe-boundary state. Kept in this existing runtime
    /// container so older `CompactionConfig` construction sites remain source
    /// compatible while the policy is rolled out.
    boundary_state: Cell<CompactionBoundaryState>,
    /// Lazily initialized runtime prompt store. A failed reload retains the
    /// store's last-known-good snapshot.
    prompt_store: RefCell<Option<xai_grok_compaction::RuntimePromptStore>>,
}

impl PrefireState {
    /// Try to claim the single in-flight slot. Returns `true` iff this caller
    /// won the race and should spawn pass-1 (the caller must later call
    /// [`Self::finish`]).
    pub fn try_begin(&self) -> bool {
        self.in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    }

    /// Release the in-flight slot (call exactly once after a `try_begin` win).
    pub fn finish(&self) {
        self.in_flight.store(false, Ordering::Release);
    }

    pub fn is_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::Acquire)
    }

    /// Stash the spawned pass-1 task handle so pass-2 can await it if it is
    /// still running when compaction fires.
    pub fn set_handle(&self, handle: tokio::task::JoinHandle<()>) {
        self.handle.replace(Some(handle));
    }

    /// Take the pass-1 task handle, if any, so the caller can await completion
    /// before reading the cache. Leaves `None`.
    pub fn take_handle(&self) -> Option<tokio::task::JoinHandle<()>> {
        self.handle.borrow_mut().take()
    }

    pub fn store(&self, cache: AsyncCompactionCache) {
        self.cache.replace(Some(cache));
    }

    /// Take the cache, leaving `None`.
    pub fn take(&self) -> Option<AsyncCompactionCache> {
        self.cache.borrow_mut().take()
    }

    /// Drop any cached async pass-1 result (invalidation: model switch, rewind,
    /// apply, edits).
    pub fn clear(&self) {
        self.cache.replace(None);
    }

    pub fn has_cache(&self) -> bool {
        self.cache.borrow().is_some()
    }

    pub fn boundary_state(&self) -> CompactionBoundaryState {
        self.boundary_state.get()
    }

    pub fn set_boundary_state(&self, state: CompactionBoundaryState) {
        self.boundary_state.set(state);
    }

    pub fn runtime_prompt_snapshot(
        &self,
    ) -> Result<xai_grok_compaction::RuntimePromptSnapshot, xai_grok_compaction::RuntimePromptError>
    {
        let mut store = self.prompt_store.borrow_mut();
        if store.is_none() {
            *store = Some(xai_grok_compaction::RuntimePromptStore::new(None)?);
        }
        let store = store.as_mut().expect("runtime prompt store initialized");
        if let Err(error) = store.reload_if_changed() {
            tracing::warn!(%error, "runtime compaction prompt reload rejected; retaining last-known-good prompt");
        }
        Ok(store.snapshot())
    }

    pub fn force_reload_runtime_prompt(
        &self,
    ) -> Result<xai_grok_compaction::RuntimePromptSnapshot, xai_grok_compaction::RuntimePromptError>
    {
        let mut store = self.prompt_store.borrow_mut();
        if store.is_none() {
            *store = Some(xai_grok_compaction::RuntimePromptStore::new(None)?);
        }
        let store = store.as_mut().expect("runtime prompt store initialized");
        store.force_reload()?;
        Ok(store.snapshot())
    }
}

pub struct CompactionConfig {
    /// Context window usage percentage (0-100) at which auto-compact triggers.
    ///
    /// `Cell` so the value can be re-resolved at model-switch time without
    /// holding `&mut self` on the actor. `SessionActor` is `!Send`, so
    /// `Cell` is sufficient (no atomic ordering needed).
    pub threshold_percent: Cell<u8>,
    /// Debug: when set, next auto-compact check triggers unconditionally.
    pub force_compact: Arc<AtomicBool>,
    /// Auto-compaction suppression state (`SUPPRESS_*`) after a deterministic
    /// failure; the gates early-return unless `SUPPRESS_NONE`. Manual `/compact` ignores it.
    pub auto_compact_suppressed: AtomicU8,
    /// Locks the context window when `GROK_DEBUG_CONTEXT_WINDOW` is set.
    pub context_window_override: Option<std::num::NonZeroU64>,
    pub count: AtomicU64,
    /// Set at turn end; consumed at next turn start for model-switch compaction.
    /// `Cell` because `SessionActor` is `!Send`.
    pub previous_model: Cell<Option<PreviousModelInfo>>,
    /// The resolved mode; `Segments` carries its detail level inline.
    pub compaction_mode: xai_chat_state::CompactionMode,
    /// When `true`, feed the summarizer the verbatim conversation instead of the lossy rewrite (the retry loop may still fall back).
    pub verbatim_input: bool,
    pub tool_choice: crate::util::config::CompactionToolChoice,
    /// Prefire two-pass state (background NOTE₁ cache + in-flight guard).
    /// `Default` (empty cache, not in-flight).
    pub prefire: PrefireState,
    /// Sticky once a forked session releases its inherited prefix under compaction pressure (see `run_compact_inner`), so it stops re-pinning it.
    pub prefix_released: AtomicBool,
}

#[cfg(test)]
mod prefire_state_tests {
    use super::*;

    fn dummy_cache() -> AsyncCompactionCache {
        AsyncCompactionCache {
            note1: "NOTE1".to_string(),
            prefix_len: 3,
            fingerprint: 42,
            model_slug: "grok".to_string(),
            pass1_latency_ms: 5,
        }
    }

    /// Pass-2 must be able to await a still-running pass-1 and then read its
    /// cache — i.e. an in-flight prefire is waited for, not discarded for a full
    /// single-pass.
    #[tokio::test]
    async fn take_handle_awaits_in_flight_pass1_then_cache_is_available() {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async {
                let state = std::rc::Rc::new(PrefireState::default());
                let worker = std::rc::Rc::clone(&state);
                // Background pass-1 that stores its cache only after yielding,
                // so the cache is absent at the moment pass-2 starts.
                let handle = tokio::task::spawn_local(async move {
                    tokio::task::yield_now().await;
                    worker.store(dummy_cache());
                    worker.finish();
                });
                state.set_handle(handle);

                assert!(!state.has_cache(), "cache absent before pass-1 completes");

                if let Some(h) = state.take_handle() {
                    let _ = h.await;
                }

                assert!(state.has_cache(), "cache present after awaiting pass-1");
                assert_eq!(state.take().unwrap().note1, "NOTE1");
                assert!(state.take_handle().is_none(), "handle consumed once taken");
            })
            .await;
    }

    /// No prefire spawned → no handle to await (pass-2 falls straight through to
    /// the single-pass path via the `take()?` that follows in the caller).
    #[tokio::test]
    async fn take_handle_is_none_without_a_spawned_pass1() {
        let state = PrefireState::default();
        assert!(state.take_handle().is_none());
        assert!(state.take().is_none());
    }
}
