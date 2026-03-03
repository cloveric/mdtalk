/// Result of checking whether agents have reached consensus.
#[derive(Debug, Clone)]
pub struct ConsensusResult {
    pub reached: bool,
    pub summary: String,
}

/// Negation patterns that invalidate a keyword match.
const NEGATION_PREFIXES: &[&str] = &[
    "no ",
    "not ",
    "not in ",
    "don't ",
    "doesn't ",
    "do not ",
    "does not ",
    "cannot ",
    "can't ",
    "didn't ",
    "did not ",
    "won't ",
    "will not ",
    "wouldn't ",
    "would not ",
    "shouldn't ",
    "should not ",
    "haven't ",
    "have not ",
    "hasn't ",
    "has not ",
    "never ",
    "不",
    "没有",
    "未",
    "无法",
];

/// Check if a keyword appears in the text in an affirmative context
/// (i.e., not preceded by a negation).
fn has_affirmative_keyword(text: &str, keyword: &str) -> bool {
    let kw_lower = keyword.to_lowercase();
    let text_lower = text.to_lowercase();
    let requires_word_boundary = kw_lower.chars().any(|ch| ch.is_ascii_alphabetic())
        && kw_lower
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch.is_ascii_whitespace());

    // Find all occurrences of the keyword
    let mut search_from = 0;
    while let Some(pos) = text_lower[search_from..].find(&kw_lower) {
        let abs_pos = search_from + pos;
        let abs_end = abs_pos + kw_lower.len();

        if requires_word_boundary {
            let before = text_lower[..abs_pos].chars().next_back();
            let after = text_lower[abs_end..].chars().next();
            let valid_boundary = !before.is_some_and(|ch| ch.is_ascii_alphabetic())
                && !after.is_some_and(|ch| ch.is_ascii_alphabetic());
            if !valid_boundary {
                search_from = abs_end;
                continue;
            }
        }

        // Check the ~20 chars before this occurrence for negation.
        // Find a valid UTF-8 char boundary to avoid panic on multibyte text.
        let mut context_start = abs_pos.saturating_sub(20);
        while context_start > 0 && !text_lower.is_char_boundary(context_start) {
            context_start -= 1;
        }
        let preceding = &text_lower[context_start..abs_pos];

        let negated = NEGATION_PREFIXES
            .iter()
            .any(|neg| preceding.ends_with(neg) || preceding.trim_end().ends_with(neg.trim()));

        if !negated {
            return true;
        }

        search_from = abs_end;
    }

    false
}

/// Check if a keyword appears in the text in a negated context.
fn has_negated_keyword(text: &str, keyword: &str) -> bool {
    let kw_lower = keyword.to_lowercase();
    let text_lower = text.to_lowercase();
    let requires_word_boundary = kw_lower.chars().any(|ch| ch.is_ascii_alphabetic())
        && kw_lower
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch.is_ascii_whitespace());

    let mut search_from = 0;
    while let Some(pos) = text_lower[search_from..].find(&kw_lower) {
        let abs_pos = search_from + pos;
        let abs_end = abs_pos + kw_lower.len();

        if requires_word_boundary {
            let before = text_lower[..abs_pos].chars().next_back();
            let after = text_lower[abs_end..].chars().next();
            let valid_boundary = !before.is_some_and(|ch| ch.is_ascii_alphabetic())
                && !after.is_some_and(|ch| ch.is_ascii_alphabetic());
            if !valid_boundary {
                search_from = abs_end;
                continue;
            }
        }

        let mut context_start = abs_pos.saturating_sub(20);
        while context_start > 0 && !text_lower.is_char_boundary(context_start) {
            context_start -= 1;
        }
        let preceding = &text_lower[context_start..abs_pos];

        let negated = NEGATION_PREFIXES
            .iter()
            .any(|neg| preceding.ends_with(neg) || preceding.trim_end().ends_with(neg.trim()));

        if negated {
            return true;
        }

        search_from = abs_end;
    }

    false
}

/// Check whether an agent's response shows unambiguous consensus.
/// Returns false if the response contains mixed signals (both affirmative
/// and negated consensus keywords).
fn agent_shows_consensus(response: &str, keywords: &[String]) -> bool {
    let has_affirmative = keywords
        .iter()
        .any(|kw| has_affirmative_keyword(response, kw));
    if !has_affirmative {
        return false;
    }
    // If any keyword is negated elsewhere in the same response, treat as mixed signal
    let has_negated = keywords.iter().any(|kw| has_negated_keyword(response, kw));
    !has_negated
}

/// Check whether the latest round's content indicates consensus.
pub fn check_consensus(
    agent_a_response: &str,
    agent_b_response: &str,
    keywords: &[String],
) -> ConsensusResult {
    // Both agents must show unambiguous consensus (affirmative keywords
    // without any negated keywords in the same response)
    let a_consensus = agent_shows_consensus(agent_a_response, keywords);
    let b_consensus = agent_shows_consensus(agent_b_response, keywords);

    if a_consensus && b_consensus {
        return ConsensusResult {
            reached: true,
            summary: "双方均通过共识关键词表达了一致意见。".to_string(),
        };
    }

    ConsensusResult {
        reached: false,
        summary: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kws() -> Vec<String> {
        vec!["agree".into(), "LGTM".into(), "同意".into()]
    }

    #[test]
    fn both_agree() {
        let r = check_consensus("I agree with all points.", "I agree, LGTM.", &kws());
        assert!(r.reached);
    }

    #[test]
    fn negated_agree_not_counted() {
        let r = check_consensus(
            "I don't agree with this approach.",
            "I agree with the review.",
            &kws(),
        );
        assert!(!r.reached);
    }

    #[test]
    fn one_side_only() {
        let r = check_consensus("I agree.", "I have more suggestions.", &kws());
        assert!(!r.reached);
    }

    #[test]
    fn chinese_keywords_no_panic() {
        let kws = vec!["同意".into(), "达成一致".into()];
        let r = check_consensus(
            "我完全同意以上所有观点，没有进一步补充。",
            "我也同意，审查意见全部达成一致。",
            &kws,
        );
        assert!(r.reached);
    }

    #[test]
    fn chinese_negation() {
        let kws = vec!["同意".into()];
        let r = check_consensus("我不同意这个方案。", "我同意所有改进建议。", &kws);
        assert!(!r.reached);
    }

    #[test]
    fn english_word_boundary_is_respected() {
        let r = check_consensus(
            "There is still disagreement on this part.",
            "I agree with the plan.",
            &kws(),
        );
        assert!(!r.reached);
    }

    #[test]
    fn mixed_agree_and_disagree_not_consensus() {
        // Agent A says "agree" and "don't agree" in the same response — mixed signal
        let r = check_consensus(
            "I agree with points 1-3, but I don't agree with point 4.",
            "I agree with all the suggestions. LGTM.",
            &kws(),
        );
        assert!(!r.reached);
    }

    #[test]
    fn mixed_chinese_agree_disagree_not_consensus() {
        let kws = vec!["同意".into()];
        let r = check_consensus(
            "我同意前三条建议，但不同意第四条。",
            "我同意所有改进建议。",
            &kws,
        );
        assert!(!r.reached);
    }

    #[test]
    fn no_consensus_phrase_is_not_treated_as_consensus() {
        let kws = vec!["consensus".into()];
        let r = check_consensus("No consensus yet.", "No consensus reached.", &kws);
        assert!(!r.reached);
    }

    #[test]
    fn not_in_agreement_is_not_treated_as_consensus() {
        let kws = vec!["agreement".into()];
        let r = check_consensus(
            "We are not in agreement on this item.",
            "Still not in agreement overall.",
            &kws,
        );
        assert!(!r.reached);
    }
}
