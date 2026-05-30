//! Topic-level contradiction detection (T0.3.x A5 — the V0.2 ship-gate).
//!
//! ## Why this exists — decoupling detection from the 0.92 merge gate
//!
//! Phase-1 clustering (`phases/cluster.rs`) only forms edges at cosine
//! **≥ 0.92** — tuned to catch near-duplicates for *merging*. A
//! knowledge-update contradiction ("the user works at Vega" → later "the
//! user works at Atlas, having left Vega") is semantically *related* but its
//! pairwise cosine sits **below** 0.92, so the pair never clusters and
//! `decide_merge` never sees it. Result (confirmed live in Claude Desktop,
//! 2026-05-29): contradictions are never detected and reads return both the
//! stale and current facts.
//!
//! The fix runs contradiction detection over the looser **K-means topic**
//! grouping (`topics::discover_topics`), which already co-locates the
//! conflicting pair (verified live — both carried
//! `topic: "professional_transitions"`). The 0.92 gate is untouched; it
//! still governs *merging*. This module governs *contradiction*.
//!
//! ## Output shape — explicit stale ids, never a whole-group sweep
//!
//! [`detect_contradiction`] returns the **specific** memory ids that are no
//! longer true ([`ContradictionVerdict::stale_memory_ids`]) rather than a
//! "winner, everything-else-loses" verdict. A topic is a *loose* grouping —
//! it may hold many compatible facts plus one stale pair. Returning explicit
//! stale ids means the orchestrator retires only the facts the LLM
//! identified as superseded, never the whole topic. The orchestrator adds a
//! belt-and-braces guard (refuse to invalidate an entire group; ignore ids
//! not in the group) so even a misbehaving model cannot mass-retire.
//!
//! ## Conservative by construction
//!
//! The system prompt instructs the model to flag a contradiction ONLY when
//! two facts make incompatible claims about the *same attribute of the same
//! subject* — co-topical-but-compatible facts ("works at Atlas" + "commutes
//! by train") must return an empty list. The `as_of` (fact-time) of each
//! memory is supplied so the model can pick the current truth by recency or
//! explicit supersession.

use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

use vault_core::{Memory, MemoryId, VaultError, VaultResult};
use vault_llm::{CompletionParams, LlmProvider};

/// JSON schema (string form) the LLM is constrained to emit. `MemoryId`
/// deserialises transparently from a UUID string, so `stale_memory_ids`
/// accepts a plain JSON array of UUID strings.
const CONTRADICTION_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "stale_memory_ids": {
            "type": "array",
            "items": { "type": "string" },
            "description": "IDs of the facts in the group that are NO LONGER TRUE (superseded). Empty when there is no contradiction or none can be confidently retired."
        },
        "reasoning": { "type": "string" }
    },
    "required": ["stale_memory_ids", "reasoning"],
    "additionalProperties": false
}"#;

/// System prompt for the topic-level contradiction judge. Conservative:
/// only same-subject / same-attribute incompatibilities count; co-topical
/// compatible facts return an empty list.
const CONTRADICTION_SYSTEM_PROMPT: &str =
    "You are a contradiction detector for a personal memory vault. You receive a group of \
     memories about one person that a clustering pass grouped under a shared topic. Each carries \
     an id, content, and as_of date (when the fact became true). Decide whether the group \
     contains a CONTRADICTION: two or more facts that make INCOMPATIBLE claims about the SAME \
     attribute of the SAME subject, such that they cannot both be currently true (e.g., 'works \
     at Vega' vs 'works at Atlas, having left Vega'; 'lives in Berlin' vs 'lives in Lisbon'). \
     Do NOT flag facts that are merely related or co-topical but compatible (e.g., 'works at \
     Atlas' and 'commutes by train' are BOTH true — return an empty list). Do NOT flag facts \
     about different subjects or different attributes. When a contradiction exists, return \
     stale_memory_ids = the ids of the fact(s) that are NO LONGER TRUE — superseded by a more \
     recent fact (use the as_of dates), a more specific fact, or an explicit replacement \
     ('having left X'). NEVER include every id: at least one fact must remain as the current \
     truth. If facts contradict but you cannot tell which is current, return an empty list and \
     explain why in reasoning. Respond with strict JSON matching the schema.";

/// One contradiction judgement over a topic group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContradictionVerdict {
    /// IDs of memories in the group that are no longer true and should be
    /// invalidated. Empty = no contradiction (or none confidently
    /// resolvable). The orchestrator filters these to the group and refuses
    /// to invalidate the entire group as a safety net.
    #[serde(default)]
    pub stale_memory_ids: Vec<MemoryId>,
    /// The model's natural-language explanation; recorded in the invalidate
    /// audit event for each retired fact.
    pub reasoning: String,
}

/// Ask the LLM whether a topic group contains a same-subject contradiction
/// and, if so, which member facts are stale.
///
/// `group` is the hydrated set of memories assigned to one topic (size ≥ 2
/// — the caller skips singletons). The content + `as_of` of each is sent to
/// the model.
///
/// # Errors
///
/// - [`VaultError::Llm`] — prompt serialisation, the LLM call, or parsing
///   the response into a [`ContradictionVerdict`] failed. The caller logs
///   and continues (one bad topic does not abort the run; the next nightly
///   cycle retries).
#[instrument(skip(group, llm), fields(group_size = group.len()))]
pub async fn detect_contradiction(
    group: &[&Memory],
    llm: &dyn LlmProvider,
) -> VaultResult<ContradictionVerdict> {
    #[derive(Serialize)]
    struct PromptMemory<'a> {
        id: String,
        content: &'a str,
        as_of: String,
    }
    #[derive(Serialize)]
    struct PromptBody<'a> {
        memories: Vec<PromptMemory<'a>>,
    }

    let body = PromptBody {
        memories: group
            .iter()
            .map(|m| PromptMemory {
                id: m.id.to_string(),
                content: &m.content,
                as_of: m.valid_from.to_rfc3339(),
            })
            .collect(),
    };
    let user_prompt = serde_json::to_string(&body).map_err(|e| {
        VaultError::Llm(format!(
            "contradiction prompt JSON serialisation failed: {e}"
        ))
    })?;

    // Deterministic judge: temperature 0 + fixed seed so re-running the same
    // nightly cycle is reproducible (mirrors topics.rs label determinism).
    let params = CompletionParams {
        max_tokens: 256,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(0xC0_07_AD_1C),
        system_prompt: Some(CONTRADICTION_SYSTEM_PROMPT.to_string()),
    };

    let raw = llm
        .complete_json(&user_prompt, CONTRADICTION_SCHEMA, &params)
        .await
        .map_err(|e| VaultError::Llm(format!("contradiction LLM call failed: {e}")))?;

    let verdict: ContradictionVerdict = serde_json::from_str(&raw).map_err(|e| {
        warn!(raw_response = %raw, "contradiction judge returned malformed JSON");
        VaultError::Llm(format!(
            "contradiction LLM response failed to parse as ContradictionVerdict: {e} (raw: {raw})"
        ))
    })?;

    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vault_core::{Boundary, MemoryType, NewMemory};
    use vault_llm::MockLlmProvider;

    fn mem(content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("testeval").unwrap(),
            source_agent: None,
            confidence: 0.95,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory")
    }

    #[tokio::test]
    async fn parses_stale_ids_on_contradiction() {
        let vega = mem("As of 2026-01-10 the user worked at Vega Bridgeworks.");
        let atlas = mem("As of 2026-04-01 the user works at Atlas Structures, having left Vega.");
        let group = [&vega, &atlas];

        let llm = MockLlmProvider::new(
            "phi-4-test",
            format!(
                r#"{{"stale_memory_ids":["{}"],"reasoning":"Atlas supersedes Vega"}}"#,
                vega.id
            ),
        );
        let verdict = detect_contradiction(&group, &llm).await.unwrap();
        assert_eq!(verdict.stale_memory_ids, vec![vega.id]);
    }

    #[tokio::test]
    async fn returns_empty_when_no_contradiction() {
        let a = mem("The user works at Atlas Structures.");
        let b = mem("The user commutes to work by train.");
        let group = [&a, &b];

        let llm = MockLlmProvider::new(
            "phi-4-test",
            r#"{"stale_memory_ids":[],"reasoning":"compatible facts; no contradiction"}"#,
        );
        let verdict = detect_contradiction(&group, &llm).await.unwrap();
        assert!(
            verdict.stale_memory_ids.is_empty(),
            "compatible co-topical facts MUST yield no stale ids"
        );
    }

    #[tokio::test]
    async fn malformed_json_surfaces_as_llm_error() {
        let a = mem("a");
        let b = mem("b");
        let group = [&a, &b];
        let llm = MockLlmProvider::new("phi-4-test", "not json at all");
        let err = detect_contradiction(&group, &llm).await.unwrap_err();
        assert!(
            matches!(err, VaultError::Llm(_)),
            "malformed JSON MUST surface as VaultError::Llm, got {err:?}"
        );
    }

    #[tokio::test]
    async fn empty_stale_ids_field_defaults_when_absent() {
        // `#[serde(default)]` on stale_memory_ids: a response with only
        // reasoning MUST parse as an empty list, not a hard error.
        let a = mem("a");
        let b = mem("b");
        let group = [&a, &b];
        let llm = MockLlmProvider::new("phi-4-test", r#"{"reasoning":"no conflict"}"#);
        let verdict = detect_contradiction(&group, &llm).await.unwrap();
        assert!(verdict.stale_memory_ids.is_empty());
    }
}
