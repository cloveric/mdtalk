/// Result of checking whether agents have reached consensus.
#[derive(Debug, Clone)]
pub struct ConsensusResult {
    pub reached: bool,
    pub summary: String,
}

const ENGLISH_NEGATION_TOKENS: &[&str] = &[
    "no",
    "not",
    "dont",
    "don't",
    "doesnt",
    "doesn't",
    "cannot",
    "cant",
    "can't",
    "didnt",
    "didn't",
    "wont",
    "won't",
    "wouldnt",
    "wouldn't",
    "shouldnt",
    "shouldn't",
    "never",
];

const CHINESE_NEGATION_TOKENS: &[&str] = &["不", "没有", "未", "无法"];
const ENGLISH_TURNING_TOKENS: &[&str] = &[
    "but",
    "however",
    "although",
    "though",
    "yet",
    "nevertheless",
    "nonetheless",
];
const CHINESE_TURNING_TOKENS: &[&str] = &["但", "但是", "不过", "然而", "可是"];

fn is_clause_boundary(ch: char) -> bool {
    matches!(
        ch,
        '.' | '!' | '?' | ';' | ',' | '\n' | '。' | '！' | '？' | '；' | '，'
    )
}

fn has_recent_english_negation(preceding: &str) -> bool {
    let tokens: Vec<&str> = preceding
        .split(|ch: char| !(ch.is_ascii_alphabetic() || ch == '\''))
        .filter(|token| !token.is_empty())
        .collect();
    tokens
        .iter()
        .rev()
        .take(4)
        .any(|token| ENGLISH_NEGATION_TOKENS.contains(token))
}

fn has_recent_chinese_negation(preceding: &str) -> bool {
    let tail: String = preceding
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    CHINESE_NEGATION_TOKENS
        .iter()
        .any(|token| tail.contains(token))
}

fn is_negated_context(preceding: &str) -> bool {
    has_recent_english_negation(preceding) || has_recent_chinese_negation(preceding)
}

fn has_english_turning_word(text: &str) -> bool {
    text.split(|ch: char| !(ch.is_ascii_alphabetic() || ch == '\''))
        .filter(|token| !token.is_empty())
        .any(|token| ENGLISH_TURNING_TOKENS.contains(&token))
}

fn has_chinese_turning_word(text: &str) -> bool {
    CHINESE_TURNING_TOKENS
        .iter()
        .any(|token| text.contains(token))
}

fn has_turning_word_in_following_clause(text_lower: &str, keyword_end: usize) -> bool {
    let mut context_end = (keyword_end + 120).min(text_lower.len());
    while context_end > keyword_end && !text_lower.is_char_boundary(context_end) {
        context_end -= 1;
    }
    let following = &text_lower[keyword_end..context_end];
    let clause_end = following
        .char_indices()
        .find(|(_, ch)| matches!(ch, '.' | '!' | '?' | ';' | '\n' | '。' | '！' | '？' | '；'))
        .map_or(following.len(), |(idx, _)| idx);
    let local_following = following[..clause_end].trim_start();
    has_english_turning_word(local_following) || has_chinese_turning_word(local_following)
}

/// Check if a keyword appears in the text in an affirmative context.
/// When `require_unambiguous` is true, affirmative keywords followed by
/// turning words (e.g. but/however/但是) are treated as partial, not full consensus.
fn has_affirmative_keyword(text: &str, keyword: &str, require_unambiguous: bool) -> bool {
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

        // Check the local clause before this occurrence for negation.
        let mut context_start = abs_pos.saturating_sub(80);
        while context_start > 0 && !text_lower.is_char_boundary(context_start) {
            context_start -= 1;
        }
        let preceding = &text_lower[context_start..abs_pos];
        let clause_start = preceding
            .char_indices()
            .rev()
            .find(|(_, ch)| is_clause_boundary(*ch))
            .map_or(0, |(idx, ch)| idx + ch.len_utf8());
        let local_preceding = preceding[clause_start..].trim_end();

        let negated = is_negated_context(local_preceding);

        let has_turning =
            require_unambiguous && has_turning_word_in_following_clause(&text_lower, abs_end);

        if !negated && !has_turning {
            return true;
        }

        search_from = abs_end;
    }

    false
}

/// Check whether an agent's response shows unambiguous consensus.
/// Returns false if the response contains mixed signals (both affirmative
/// and negated consensus keywords).
fn agent_shows_consensus(response: &str, keywords: &[String], require_unambiguous: bool) -> bool {
    keywords
        .iter()
        .any(|kw| has_affirmative_keyword(response, kw, require_unambiguous))
}

/// Check consensus based on Agent B's response only, accepting full OR partial agreement.
/// Used when: (a) max_exchanges == 1 (only one shot), or (b) it's the last exchange
/// (exhausted all exchanges — apply whatever was agreed).
pub fn check_b_only(agent_b_response: &str, keywords: &[String]) -> ConsensusResult {
    if agent_shows_consensus(agent_b_response, keywords, false) {
        ConsensusResult {
            reached: true,
            summary: "Agent B 作为验证方表达了认可意见（全部或部分同意）。".to_string(),
        }
    } else {
        ConsensusResult {
            reached: false,
            summary: String::new(),
        }
    }
}

/// Check consensus based on Agent B's response only, accepting ONLY full agreement.
/// Used for exchange 1 when max_exchanges > 1: partial agreement means "keep discussing".
pub fn check_b_full_only(agent_b_response: &str, keywords: &[String]) -> ConsensusResult {
    // Filter out partial-agreement keywords (those containing "部分" or "partial")
    let full_keywords: Vec<String> = keywords
        .iter()
        .filter(|kw| {
            let lower = kw.to_lowercase();
            !lower.contains("部分") && !lower.contains("partial")
        })
        .cloned()
        .collect();

    if agent_shows_consensus(agent_b_response, &full_keywords, true) {
        ConsensusResult {
            reached: true,
            summary: "Agent B 完全认可审查意见。".to_string(),
        }
    } else {
        ConsensusResult {
            reached: false,
            summary: String::new(),
        }
    }
}

/// Check consensus for exchange 2+, where both agents have been debating
/// and both must explicitly express agreement.
pub fn check_consensus(
    agent_a_response: &str,
    agent_b_response: &str,
    keywords: &[String],
) -> ConsensusResult {
    let a_consensus = agent_shows_consensus(agent_a_response, keywords, false);
    let b_consensus = agent_shows_consensus(agent_b_response, keywords, false);

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
    fn inserted_words_between_not_and_agree_is_not_consensus() {
        let r = check_consensus(
            "I do not fully agree with this plan.",
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
    fn mixed_agree_and_disagree_is_partial_consensus() {
        // Agent A agrees with some, disagrees with others — partial agreement counts
        let r = check_consensus(
            "I agree with points 1-3, but I don't agree with point 4.",
            "I agree with all the suggestions. LGTM.",
            &kws(),
        );
        assert!(r.reached);
    }

    #[test]
    fn mixed_chinese_agree_disagree_is_partial_consensus() {
        let kws = vec!["同意".into()];
        let r = check_consensus(
            "我同意前三条建议，但不同意第四条。",
            "我同意所有改进建议。",
            &kws,
        );
        assert!(r.reached);
    }

    #[test]
    fn explicit_disagree_with_affirmative_is_partial_consensus() {
        // A says "agree" AND "disagree" — has affirmative keyword, counts as partial
        let r = check_consensus(
            "I agree with most points, but I disagree with item 3.",
            "I agree with the updated review.",
            &kws(),
        );
        assert!(r.reached);
    }

    #[test]
    fn chinese_partial_agreement_with_negation_is_consensus() {
        // Real-world case: B confirms 16/17 items, rejects 1 — should be consensus
        let kws = vec!["成立".into(), "同意".into()];
        let r = check_b_only(
            "汇总: 17 条中 16 条成立，1 条不成立（第 3 条，当前代码已无该重复实现）。",
            &kws,
        );
        assert!(r.reached);
    }

    #[test]
    fn pure_negation_no_consensus() {
        // B only says things are NOT valid — no affirmative keyword
        let kws = vec!["成立".into()];
        let r = check_b_only("以上问题均不成立，全部驳回。", &kws);
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_turning_word_after_affirmative_english() {
        let kws = vec!["agree".into(), "partially agree".into()];
        let r = check_b_full_only(
            "I agree with points 1-3, however point 4 is still wrong and needs changes.",
            &kws,
        );
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_turning_word_after_affirmative_chinese() {
        let kws = vec!["同意".into(), "部分同意".into()];
        let r = check_b_full_only("我同意前三条建议，但是第四条我不同意。", &kws);
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
