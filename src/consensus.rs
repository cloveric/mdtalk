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
    "while",
    "except",
    "nevertheless",
    "nonetheless",
];
const CHINESE_TURNING_TOKENS: &[&str] = &["但", "但是", "不过", "然而", "可是"];
const CHINESE_NEGATION_LOOKBACK_CHARS: usize = 16;
const CONCLUSION_FALLBACK_NON_EMPTY_LINES: usize = 12;

fn is_clause_boundary(ch: char) -> bool {
    matches!(
        ch,
        '.' | '!' | '?' | ';' | ',' | '\n' | '。' | '！' | '？' | '；' | '，'
    )
}

fn is_sentence_boundary(ch: char) -> bool {
    matches!(ch, '.' | '!' | '?' | ';' | '\n' | '。' | '！' | '？' | '；')
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
        .take(CHINESE_NEGATION_LOOKBACK_CHARS)
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

/// Check if preceding text contains a partial-agreement qualifier ("部分"/"partially").
/// Used in full-only checks to reject "部分成立" as full agreement.
fn has_partial_qualifier(preceding: &str) -> bool {
    let tail: String = preceding
        .chars()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    tail.contains("部分")
        || tail
            .split(|ch: char| !ch.is_ascii_alphabetic())
            .any(|w| w.eq_ignore_ascii_case("partially"))
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
    let following = text_lower[keyword_end..].trim_start();
    if following.is_empty() {
        return false;
    }

    let mut sentence_start = 0usize;
    for (idx, ch) in following.char_indices() {
        if !is_sentence_boundary(ch) {
            continue;
        }
        let sentence = following[sentence_start..idx].trim_start();
        if has_english_turning_word(sentence) || has_chinese_turning_word(sentence) {
            return true;
        }
        sentence_start = idx + ch.len_utf8();
    }

    let tail = following[sentence_start..].trim_start();
    has_english_turning_word(tail) || has_chinese_turning_word(tail)
}

fn has_turning_word_in_preceding_clause(local_preceding: &str) -> bool {
    let clause = local_preceding.trim_end();
    if clause.is_empty() {
        return false;
    }
    has_english_turning_word(clause) || has_chinese_turning_word(clause)
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

        let has_turning = require_unambiguous
            && (has_turning_word_in_following_clause(&text_lower, abs_end)
                || has_turning_word_in_preceding_clause(local_preceding));

        let is_partial = require_unambiguous && has_partial_qualifier(local_preceding);

        if !negated && !has_turning && !is_partial {
            return true;
        }

        search_from = abs_end;
    }

    false
}

/// Extract the conclusion section from an agent's response.
///
/// Looks for a "CONCLUSION:" or "结论：" line. If found, returns only that line
/// and everything after it. Otherwise, returns the last ~500 characters as
/// a best-effort fallback (the conclusion is always at the end of the response).
fn extract_conclusion_section(response: &str) -> &str {
    // Look for explicit conclusion markers (case-insensitive search)
    let lower = response.to_lowercase();
    for marker in &["conclusion:", "结论：", "结论:"] {
        if let Some(pos) = lower.rfind(marker) {
            // Find the start of the line containing the marker
            let line_start = response[..pos].rfind('\n').map_or(0, |p| p + 1);
            return &response[line_start..];
        }
    }
    // No conclusion marker found — use the trailing non-empty paragraph,
    // with a larger line cap for multi-line summaries.
    let mut line_starts: Vec<usize> = vec![0];
    for (idx, ch) in response.char_indices() {
        if ch == '\n' {
            line_starts.push(idx + 1);
        }
    }

    let mut started = false;
    let mut kept = 0usize;
    let mut start_idx = response.len();
    for &line_start in line_starts.iter().rev() {
        let line_end = response[line_start..]
            .find('\n')
            .map_or(response.len(), |offset| line_start + offset);
        let line = &response[line_start..line_end];
        let is_empty = line.trim().is_empty();

        if !started {
            if is_empty {
                continue;
            }
            started = true;
        } else if is_empty {
            break;
        }

        if kept >= CONCLUSION_FALLBACK_NON_EMPTY_LINES {
            break;
        }
        start_idx = line_start;
        kept += 1;
    }

    if kept == 0 {
        return response;
    }
    &response[start_idx..]
}

/// Check whether an agent's response shows consensus.
///
/// Only checks the conclusion section of the response (the "CONCLUSION:" / "结论："
/// line and following text) to avoid false positives from per-item evaluation
/// markers like "【成立】" in the body of the response.
fn agent_shows_consensus(response: &str, keywords: &[String], require_unambiguous: bool) -> bool {
    let conclusion = extract_conclusion_section(response);
    keywords
        .iter()
        .any(|kw| has_affirmative_keyword(conclusion, kw, require_unambiguous))
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
/// No longer used in orchestrator (exchange 1 now skips consensus entirely when
/// max_exchanges > 1), but kept for tests and potential future use.
#[allow(dead_code)]
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
    let a_consensus = agent_shows_consensus(agent_a_response, keywords, true);
    let b_consensus = agent_shows_consensus(agent_b_response, keywords, true);

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
    fn mixed_agree_and_disagree_is_not_unambiguous_consensus() {
        let r = check_consensus(
            "I agree with points 1-3, but I don't agree with point 4.",
            "I agree with all the suggestions. LGTM.",
            &kws(),
        );
        assert!(!r.reached);
    }

    #[test]
    fn mixed_chinese_agree_disagree_is_not_unambiguous_consensus() {
        let kws = vec!["同意".into()];
        let r = check_consensus(
            "我同意前三条建议，但不同意第四条。",
            "我同意所有改进建议。",
            &kws,
        );
        assert!(!r.reached);
    }

    #[test]
    fn explicit_disagree_with_affirmative_is_not_unambiguous_consensus() {
        let r = check_consensus(
            "I agree with most points, but I disagree with item 3.",
            "I agree with the updated review.",
            &kws(),
        );
        assert!(!r.reached);
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
    fn full_only_rejects_partial_qualifier_chinese() {
        // "部分成立" should NOT count as full agreement
        let kws = vec!["成立".into(), "同意".into()];
        let r = check_b_full_only(
            "1. 【部分成立】UTF-8 字节切片确实可能 panic。\n2. 【不成立】当前代码已无该问题。",
            &kws,
        );
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_partially_qualifier_english() {
        let kws = vec!["agree".into()];
        let r = check_b_full_only("I partially agree with the review findings.", &kws);
        assert!(!r.reached);
    }

    #[test]
    fn b_only_accepts_partial_qualifier() {
        // check_b_only (last exchange) should still accept "部分成立"
        let kws = vec!["成立".into()];
        let r = check_b_only(
            "1. 【部分成立】问题存在但不严重。\n2. 【成立】确认该 bug。",
            &kws,
        );
        assert!(r.reached);
    }

    #[test]
    fn full_only_rejects_turning_word_after_affirmative_while() {
        let kws = vec!["agree".into()];
        let r = check_b_full_only(
            "I agree with this approach, while item 4 still has a blocker.",
            &kws,
        );
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_turning_word_after_affirmative_except() {
        let kws = vec!["agree".into()];
        let r = check_b_full_only("I agree, except point 4 still looks incorrect.", &kws);
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_cross_sentence_turning_word_english() {
        let kws = vec!["agree".into()];
        let r = check_b_full_only(
            "I agree with all items. But point 4 is still incorrect.",
            &kws,
        );
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_cross_sentence_turning_word_chinese() {
        let kws = vec!["同意".into()];
        let r = check_b_full_only("我同意所有建议。但是第4条仍然不正确。", &kws);
        assert!(!r.reached);
    }

    #[test]
    fn b_only_accepts_turning_word_after_affirmative_as_partial() {
        let kws = vec!["agree".into(), "同意".into()];
        let r = check_b_only(
            "I agree with items 1-3, but point 4 is completely wrong.",
            &kws,
        );
        assert!(r.reached);
    }

    #[test]
    fn b_only_accepts_explicit_partial_keyword_without_turning() {
        let kws = vec!["partially agree".into()];
        let r = check_b_only("CONCLUSION: partially agree", &kws);
        assert!(r.reached);
    }

    #[test]
    fn full_only_rejects_cross_keyword_turning_combo() {
        let kws = vec!["agree".into(), "LGTM".into()];
        let r = check_b_full_only("I agree with the plan, but LGTM is premature.", &kws);
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_turning_word_even_when_it_appears_later_than_200_bytes() {
        let kws = vec!["agree".into()];
        let long_middle = " detail ".repeat(40);
        let text = format!("I agree with items 1-3.{long_middle} But item 4 is still wrong.");
        let r = check_b_full_only(&text, &kws);
        assert!(!r.reached);
    }

    #[test]
    fn full_only_rejects_turning_word_even_when_it_appears_beyond_1200_bytes() {
        let kws = vec!["agree".into()];
        let long_middle = " detail ".repeat(260);
        let text =
            format!("I agree with items 1-3.{long_middle} However, item 4 is still incorrect.");
        let r = check_b_full_only(&text, &kws);
        assert!(!r.reached);
    }

    #[test]
    fn chinese_negation_with_long_modifier_is_still_negated() {
        let kws = vec!["同意".into()];
        let r = check_consensus(
            "我们团队经过讨论后认为目前并不完全同意这个方案。",
            "我同意所有改进建议。",
            &kws,
        );
        assert!(!r.reached);
    }

    #[test]
    fn bilateral_consensus_requires_unambiguous_agreement() {
        let r = check_consensus(
            "I agree with items 1-3, but I disagree with item 4.",
            "I agree with all points.",
            &kws(),
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

    #[test]
    fn per_item_markers_without_conclusion_do_not_trigger_consensus() {
        // B's response has many "【成立】" per-item markers but no conclusion line
        let kws = vec!["成立".into(), "同意".into()];
        let response = "\
**验证结论结果（基于当前代码）**\n\
\n\
1. 【成立】按关键词匹配 `any` 匹配，确实可能漏检。\n\
2. 【部分成立】`check_b_only` 确传了 `require_unambiguous=true`。\n\
3. 【成立】`read_tail_lines` 按字节切割存在风险。\n\
4. 【成立】`OrchestratorState` 包含大字段，clone 有性能风险。\n\
5. 【成立】Windows 仅用固定 `7600` 校验 prompt 长度，未纳入预留。\n\
6. 【成立】merge 失败后仅记录日志，不会切回原分支。\n\
7. 【部分成立】加载条件 `no_apply` && dashboard 确实存在歧义。\n\
8. 【成立】完成态 30 秒后全自动退出。\n\
9. 【部分成立】代码确实依赖 `entry[3..]` 和最小长度假设。\n\
\n\
以上是逐条核对结果，需要进一步讨论。";
        let r = check_b_full_only(response, &kws);
        assert!(
            !r.reached,
            "per-item markers should not trigger consensus without a conclusion line"
        );
    }

    #[test]
    fn conclusion_line_triggers_consensus_despite_body_markers() {
        let kws = vec!["成立".into(), "同意".into(), "结论：同意".into()];
        let response = "\
1. 【成立】问题确认。\n\
2. 【部分成立】问题部分存在。\n\
3. 【不成立】代码已修复。\n\
\n\
结论：同意";
        let r = check_b_full_only(response, &kws);
        assert!(r.reached, "conclusion line should trigger consensus");
    }

    #[test]
    fn conclusion_disagree_does_not_trigger_consensus() {
        let kws = vec!["成立".into(), "同意".into()];
        let response = "\
1. 【成立】问题确认。\n\
2. 【成立】确实存在。\n\
\n\
结论：不同意，还需进一步讨论。";
        let r = check_b_full_only(response, &kws);
        assert!(
            !r.reached,
            "conclusion with disagree should not be consensus"
        );
    }

    #[test]
    fn extract_conclusion_finds_chinese_marker() {
        let text = "正文内容。\n\n结论：同意所有修改。";
        let section = extract_conclusion_section(text);
        assert!(section.contains("结论：同意"));
        assert!(!section.contains("正文内容"));
    }

    #[test]
    fn extract_conclusion_finds_english_marker() {
        let text = "Body text here.\n\nCONCLUSION: I agree with all changes.";
        let section = extract_conclusion_section(text);
        assert!(section.contains("CONCLUSION: I agree"));
        assert!(!section.contains("Body text"));
    }

    #[test]
    fn extract_conclusion_fallback_uses_tail() {
        let text = "Some review text without a conclusion marker. I agree.";
        let section = extract_conclusion_section(text);
        assert!(section.contains("I agree"));
    }

    #[test]
    fn extract_conclusion_fallback_keeps_more_than_last_three_lines() {
        let text = "\
line 1
line 2
line 3
line 4 I agree with these fixes
line 5
line 6
line 7";
        let section = extract_conclusion_section(text);
        assert!(section.contains("I agree"));
    }
}
