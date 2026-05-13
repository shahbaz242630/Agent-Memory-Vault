//! `LlmProvider` trait + `CompletionParams` + `MockLlmProvider`.
//!
//! Surface locked by T0.2.1 iteration 2 (2026-05-13, post-spike-2). Designed
//! for T0.2.3 consolidator merge-decisions as the only V0.2 consumer; the
//! trait stays minimal + model-agnostic so a Qwen3-4B-Instruct (or any future
//! provider) swap is a config-flag change — no `<|im_sep|>`/Phi-specific
//! tokenizer assumptions, no model-specific token-IDs in the trait surface.
//!
//! ## Consumer-side contract intent (iteration 2 §3 concern #4 + obs 1b)
//!
//! - `complete_json` returns raw `String`; **caller** deserializes via
//!   `serde_json::from_str` and treats parse-failure as a hard error, NOT a
//!   retry case. GBNF grammar gives structural validity at sample time; a
//!   parse failure means either (i) llama.cpp#18173-class bug (would have
//!   fired at `LlamaSampler::grammar` construction, not here) or (ii) a real
//!   downstream bug we want to surface rather than retry around.
//!
//! - The score field in T0.2.3-shape merge-decision output is a **confidence
//!   signal**, NOT a deterministic threshold-comparator. Spike-2 observed
//!   1.0 / 0.85 / 0.0 gradient on canned cases (see
//!   `tests/fixtures/canned_merge_decisions.json` + `score_caveat` field
//!   there). T0.2.3 consumers should write `if score > 0.7` style
//!   thresholds, NOT `if score == 1.0`. Score behavior may shift under
//!   model swap or quantization changes.
//!
//! ## Why async-trait
//!
//! Inference is CPU-bound (release-build Phi-4-mini ~9.8s p50 on a 13th-gen
//! i7-13620H per spike-2 Stage E) but the trait is async-clean so callers
//! can fan out parallel decisions in tokio runtime. The concrete
//! `Phi4MiniProvider` (lands at Phase 3) wraps `LlamaContext::decode` in
//! `tokio::task::spawn_blocking` internally; the trait surface stays async.

use async_trait::async_trait;

use crate::error::VaultLlmResult;

/// Capability surface for any local LLM that can serve T0.2.3 consolidator
/// merge-decisions (and future structured-JSON workloads).
///
/// Implementations: [`Phi4MiniProvider`] (Phase 3, the V0.2 default) +
/// [`MockLlmProvider`] (this module, behind `#[cfg(any(test, feature =
/// "test-utils"))]` per iteration 2 design detail (b)).
///
/// [`Phi4MiniProvider`]: ../phi4_mini/struct.Phi4MiniProvider.html
#[async_trait]
pub trait LlmProvider: Send + Sync + std::fmt::Debug {
    /// Generate a completion constrained to the given JSON schema (compiled
    /// to GBNF grammar internally). Returns the raw JSON output string;
    /// caller deserializes and treats parse-failure as a hard error.
    ///
    /// `prompt` — the full prompt string already formatted for the underlying
    /// model's chat template. The trait deliberately does NOT do template
    /// rendering; that lives inside the concrete provider so a Phi → Qwen
    /// swap doesn't leak template differences through the trait.
    ///
    /// `json_schema` — the JSON Schema source (as a string) for the desired
    /// output shape. Compiled to GBNF via `llama-cpp-2`'s
    /// `json_schema_to_grammar` (or equivalent for non-llama.cpp providers).
    async fn complete_json(
        &self,
        prompt: &str,
        json_schema: &str,
        params: &CompletionParams,
    ) -> VaultLlmResult<String>;

    /// Stable identifier for the active model. Used for audit-log lines,
    /// retry routing, and test assertions. Example:
    /// `"phi-4-mini-instruct-Q4_K_M"`. Stable across runs of the same
    /// provider with the same model file.
    fn model_id(&self) -> &str;
}

/// Per-call inference parameters.
///
/// `seed: Option<u32>` matches `llama-cpp-2`'s `LlamaContextParams::with_seed(u32)`
/// signature (corrected from iteration 1's `u64`). T0.2.3 consolidator will pass
/// `seed = Some(hash_of_memory_pair)` so the same input pair produces the same
/// merge decision across reruns (determinism for audit reproducibility).
///
/// `system_prompt: Option<String>` was added per **ADR-044 Amendment 1**
/// (T0.2.3 commit 1) — the T0.2.2 commit 2 plan amendment surfaced that
/// `Phi4MiniProvider`'s `build_chatml_prompt` hardcoded a merge-classifier
/// system message, blocking N-ary cluster prompts and any future
/// non-merge-classifier prompt shape (Phase 4 decay summaries,
/// ConflictReview-resolution prompts, fixture-generation, etc.). `None`
/// preserves the default hardcoded system message; `Some(s)` overrides at
/// call time. `MockLlmProvider` ignores the field (canned response logic
/// doesn't dispatch on prompt).
#[derive(Debug, Clone)]
pub struct CompletionParams {
    /// Hard upper bound on output tokens. T0.2.3 default ~256 (consolidator
    /// merge-decision JSON is short; longer outputs are noise).
    pub max_tokens: u32,

    /// Sampling temperature. T0.2.3 default `0.0` (deterministic /
    /// greedy under grammar constraint).
    pub temperature: f32,

    /// Top-p nucleus sampling threshold. T0.2.3 default `1.0` (no nucleus
    /// truncation; grammar constraint does the work).
    pub top_p: f32,

    /// Optional seed for deterministic reproducibility. `None` lets the
    /// underlying llama.cpp backend pick a default (typically a time-based
    /// seed) — fine for ad-hoc inference, not for audit-replayable
    /// consolidator runs.
    pub seed: Option<u32>,

    /// Optional system-prompt override (ADR-044 Amendment 1, T0.2.3
    /// commit 1). `None` (default) preserves `Phi4MiniProvider`'s
    /// hardcoded merge-classifier system message — backwards-compatible
    /// with every T0.2.1-era caller. `Some(s)` swaps the system message
    /// at call time, used by T0.2.3 Phase 2's N-ary cluster
    /// merge-decision prompt and any future non-merge-classifier prompt
    /// shape.
    pub system_prompt: Option<String>,
}

impl Default for CompletionParams {
    fn default() -> Self {
        Self {
            max_tokens: 256,
            temperature: 0.0,
            top_p: 1.0,
            seed: None,
            system_prompt: None,
        }
    }
}

// ─── MockLlmProvider ────────────────────────────────────────────────────────
//
// Feature-gated behind `#[cfg(any(test, feature = "test-utils"))]` per
// iteration 2 design detail (b). Always available in this crate's `[lib]`
// test target (the `test` cfg fires automatically there); downstream
// crates that want to consume it (e.g. T0.2.3's consolidator tests) opt
// in via `vault-llm = { path = "...", features = ["test-utils"] }` in
// their `[dev-dependencies]`. Mirrors `vault-storage`'s `test_helpers`
// pattern from T0.2.0 sub-task (a).

#[cfg(any(test, feature = "test-utils"))]
pub use mock::MockLlmProvider;

#[cfg(any(test, feature = "test-utils"))]
mod mock {
    use super::*;
    use std::sync::Mutex;

    /// Deterministic mock for `LlmProvider` used by trait-conformance tests
    /// and by downstream consumer tests (T0.2.3 consolidator) that need a
    /// fast in-process LLM substitute without loading a 2.49 GB GGUF.
    ///
    /// Returns a canned response on every `complete_json` call. For tests
    /// that need varied responses, construct multiple mocks with different
    /// canned strings or compose with a future scripted-response variant.
    pub struct MockLlmProvider {
        model_id: String,
        canned_response: String,
        call_count: Mutex<usize>,
    }

    impl MockLlmProvider {
        /// Construct with the canned JSON response every `complete_json` call
        /// will return. The string is NOT validated as JSON at construction
        /// time — tests can pass invalid JSON deliberately to exercise the
        /// caller-side parse-failure path.
        pub fn new(model_id: impl Into<String>, canned_response: impl Into<String>) -> Self {
            Self {
                model_id: model_id.into(),
                canned_response: canned_response.into(),
                call_count: Mutex::new(0),
            }
        }

        /// How many times `complete_json` has been called against this
        /// instance. Useful for assertions in caller tests.
        pub fn call_count(&self) -> usize {
            *self
                .call_count
                .lock()
                .expect("MockLlmProvider mutex poisoned")
        }
    }

    impl std::fmt::Debug for MockLlmProvider {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockLlmProvider")
                .field("model_id", &self.model_id)
                .field("call_count", &self.call_count())
                // canned_response intentionally omitted from Debug to keep
                // logs compact when mocks return large fixture strings.
                .finish_non_exhaustive()
        }
    }

    #[async_trait]
    impl LlmProvider for MockLlmProvider {
        async fn complete_json(
            &self,
            _prompt: &str,
            _json_schema: &str,
            _params: &CompletionParams,
        ) -> VaultLlmResult<String> {
            *self
                .call_count
                .lock()
                .expect("MockLlmProvider mutex poisoned") += 1;
            Ok(self.canned_response.clone())
        }

        fn model_id(&self) -> &str {
            &self.model_id
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests covering `CompletionParams` + `MockLlmProvider` surface.
    //!
    //! Plus the 4 firm-floor "mock conformance" tests from iteration 2 §10.
    //! Originally drafted as `tests/provider_contract.rs` integration tests,
    //! but `#[cfg(test)]` on the `MockLlmProvider` re-export in lib.rs only
    //! fires for the library's own test build (not for integration tests
    //! linking it as a non-test library) — moved here as `#[cfg(test)] mod`
    //! unit tests. Same coverage, same +4 floor count, no plan-amendment
    //! drift. Downstream T0.2.3 consumer tests still get `MockLlmProvider`
    //! via the `test-utils` feature opt-in per iteration 2 design detail (b).

    use super::*;

    // ─── synchronous surface ────────────────────────────────────────────

    #[test]
    fn completion_params_default_matches_consolidator_intent() {
        let p = CompletionParams::default();
        assert_eq!(p.max_tokens, 256);
        assert_eq!(p.temperature, 0.0);
        assert_eq!(p.top_p, 1.0);
        assert_eq!(p.seed, None);
        assert_eq!(p.system_prompt, None);
    }

    /// ADR-044 Amendment 1 (T0.2.3 commit 1) pin: `system_prompt` defaults
    /// to `None` so every T0.2.1-era caller behaves identically. Breaking
    /// this default would silently change how T0.2.1's smoke tests + the
    /// merge-classifier prompt path behave.
    #[test]
    fn completion_params_default_system_prompt_is_none() {
        assert_eq!(CompletionParams::default().system_prompt, None);
    }

    #[test]
    fn mock_provider_constructs_with_model_id() {
        let mock = MockLlmProvider::new("mock-model-A", r#"{"merge":true,"score":1.0}"#);
        assert_eq!(mock.model_id(), "mock-model-A");
        assert_eq!(mock.call_count(), 0);
    }

    #[test]
    fn mock_provider_debug_redacts_canned_response() {
        let mock = MockLlmProvider::new("mock-A", r#"{"merge":true}"#);
        let debug_str = format!("{mock:?}");
        assert!(debug_str.contains("mock-A"));
        assert!(
            !debug_str.contains("merge"),
            "canned_response must NOT appear in Debug — got: {debug_str}"
        );
    }

    // ─── mock conformance (iteration 2 §10 floor: +4 firm) ──────────────

    #[tokio::test]
    async fn floor_1_mock_provider_satisfies_llm_provider_trait() {
        // Compile-time assertion via generic param: MockLlmProvider can
        // stand in wherever `impl LlmProvider` is required. Body exercises
        // the full surface (complete_json + model_id).
        async fn takes_any_provider<P: LlmProvider>(p: &P) -> String {
            let params = CompletionParams::default();
            let result = p
                .complete_json("test prompt", r#"{"type":"object"}"#, &params)
                .await
                .expect("mock complete_json must not fail");
            format!("model={} result={}", p.model_id(), result)
        }

        let mock = MockLlmProvider::new("mock-A", r#"{"merge":true,"score":1.0}"#);
        let s = takes_any_provider(&mock).await;
        assert!(s.starts_with("model=mock-A "));
        assert!(s.contains("\"merge\":true"));
    }

    #[tokio::test]
    async fn floor_2_complete_json_returns_canned_response_and_call_count_advances() {
        let canned = r#"{"merge":false,"score":0.0,"merged_text":""}"#;
        let mock = MockLlmProvider::new("mock-B", canned);
        let params = CompletionParams::default();

        assert_eq!(mock.call_count(), 0);

        let out1 = mock
            .complete_json("prompt 1", r#"{}"#, &params)
            .await
            .expect("first call");
        assert_eq!(out1, canned);
        assert_eq!(mock.call_count(), 1);

        let out2 = mock
            .complete_json("prompt 2", r#"{}"#, &params)
            .await
            .expect("second call");
        assert_eq!(out2, canned);
        assert_eq!(mock.call_count(), 2);
    }

    #[tokio::test]
    async fn floor_3_model_id_returns_constructor_argument_verbatim() {
        let mock = MockLlmProvider::new("phi-4-mini-instruct-Q4_K_M", r#"{}"#);
        assert_eq!(mock.model_id(), "phi-4-mini-instruct-Q4_K_M");

        let mock2 = MockLlmProvider::new("qwen3-4b-instruct-Q4_K_M", r#"{}"#);
        assert_eq!(mock2.model_id(), "qwen3-4b-instruct-Q4_K_M");
    }

    /// ADR-044 Amendment 1 (T0.2.3 commit 1) regression pin: MockLlmProvider
    /// MUST return its canned response unchanged whether `system_prompt`
    /// is `None`, `Some("")`, or `Some("...")`. Confirms the mock's
    /// canned-response dispatch doesn't accidentally start branching on
    /// the new field. Source-read confirmed at iteration 3 recon:
    /// `provider.rs::mock::complete_json` binds all params to `_`.
    #[tokio::test]
    async fn mock_provider_ignores_system_prompt_override() {
        let canned = r#"{"decision":"merge","merged_text":"x","reasoning":"y"}"#;
        let mock = MockLlmProvider::new("mock-sysprompt-pin", canned);

        let p_none = CompletionParams::default();
        let p_empty = CompletionParams {
            system_prompt: Some(String::new()),
            ..CompletionParams::default()
        };
        let p_set = CompletionParams {
            system_prompt: Some("You are an N-ary cluster classifier.".to_string()),
            ..CompletionParams::default()
        };

        for params in [&p_none, &p_empty, &p_set] {
            let out = mock
                .complete_json("test prompt", r#"{}"#, params)
                .await
                .expect("mock call");
            assert_eq!(out, canned);
        }
        assert_eq!(mock.call_count(), 3);
    }

    #[tokio::test]
    async fn floor_4_trait_object_boxing_compiles_and_dispatches() {
        // `Box<dyn LlmProvider>` is what vault-app + vault-consolidator
        // hold (provider choice is config-driven at runtime, so monomorph
        // doesn't apply). Pins that the trait stays object-safe AND that
        // dispatch actually works.
        let boxed: Box<dyn LlmProvider> = Box::new(MockLlmProvider::new(
            "boxed-mock",
            r#"{"merge":true,"score":0.9}"#,
        ));
        assert_eq!(boxed.model_id(), "boxed-mock");

        let params = CompletionParams::default();
        let out = boxed
            .complete_json("p", r#"{}"#, &params)
            .await
            .expect("boxed dispatch");
        assert!(out.contains("\"score\":0.9"));
    }
}
