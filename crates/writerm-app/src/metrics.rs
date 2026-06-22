//! Document length and readability metrics for the writerm writing surface.
//!
//! The metrics are intentionally simple: they operate on the raw editor text
//! (not on the rendered Markdown) so they describe the source the writer is
//! actually producing, and so the counts stay stable as the user toggles
//! between rendered and source-peek modes.
//!
//! Definitions used here:
//!
//! * **characters** — every Unicode scalar value in the text (matches the
//!   writer's perception of "characters typed").
//! * **words** — whitespace-delimited tokens, mirroring
//!   [`str::split_whitespace`] so we agree with the existing
//!   `WritermApp::word_count` value shown in the top ribbon.
//! * **sentences** — a sentence is text that terminates in `.`, `!`, or `?`.
//!   We count each sentence-terminating punctuation character, so "Wait..."
//!   contributes three sentence terminators. This keeps the implementation
//!   unambiguous and avoids the false positives of abbreviation heuristics.
//! * **paragraphs** — per the writerm product spec, a paragraph is a single
//!   line in the text file that contains at least one sentence terminator.
//!   Blank lines and headings (no terminator) do not count.
//! * **reading time** — `ceil(words / 180 wpm * 60 s)`, displayed as
//!   "Xs" for under a minute, "Xm" for whole minutes, or "Xm Ys" otherwise.

/// Reading speed used for the reading-time estimate. The product spec pins
/// this at 180 words per minute.
pub const READING_WPM: u32 = 180;

/// Snapshot of the document-length metrics displayed in the bottom-eighth
/// panel of the writerm sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentMetrics {
    pub characters: usize,
    pub words: usize,
    pub sentences: usize,
    pub paragraphs: usize,
    pub reading_secs: u64,
}

/// Compute the full set of document metrics for `text`.
///
/// This is a pure function so it can be unit-tested without spinning up a
/// full `WritermApp`.
pub fn compute(text: &str) -> DocumentMetrics {
    let characters = text.chars().count();
    let words = text.split_whitespace().count();
    let sentences = text
        .chars()
        .filter(|c| matches!(*c, '.' | '!' | '?'))
        .count();
    let paragraphs = text
        .lines()
        .filter(|line| line.chars().any(is_sentence_terminator))
        .count();
    let reading_secs = reading_time_secs(words);

    DocumentMetrics {
        characters,
        words,
        sentences,
        paragraphs,
        reading_secs,
    }
}

/// Returns true for the ASCII sentence-terminating punctuation we treat as
/// the end of a sentence.
pub fn is_sentence_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?')
}

/// Format a reading time in seconds as a compact human-readable label.
///
/// * `0` → `"0s"`
/// * `45` → `"45s"`
/// * `60` → `"1m"`
/// * `66` → `"1m 6s"`
pub fn format_reading_time(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else {
        let minutes = secs / 60;
        let remainder = secs % 60;
        if remainder == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m {remainder}s")
        }
    }
}

fn reading_time_secs(words: usize) -> u64 {
    if words == 0 {
        return 0;
    }
    // Ceiling division in floating point to keep the math obvious. With the
    // bounded u32 constant this never overflows.
    let secs = (words as f64) * 60.0 / (READING_WPM as f64);
    secs.ceil() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_all_zeros() {
        let metrics = compute("");

        assert_eq!(
            metrics,
            DocumentMetrics {
                characters: 0,
                words: 0,
                sentences: 0,
                paragraphs: 0,
                reading_secs: 0,
            }
        );
    }

    #[test]
    fn plain_paragraph_counts_one_paragraph_and_one_sentence() {
        let metrics = compute("Hello world.");

        assert_eq!(metrics.characters, 12);
        assert_eq!(metrics.words, 2);
        assert_eq!(metrics.sentences, 1);
        assert_eq!(metrics.paragraphs, 1);
        // 2 words at 180 wpm = ceil(0.667s) = 1s
        assert_eq!(metrics.reading_secs, 1);
    }

    #[test]
    fn multiple_paragraphs_split_by_blank_line() {
        let text =
            "First sentence. Second sentence.\n\nA new paragraph here!\n\nNo terminator line";
        let metrics = compute(text);

        // Lines: "First sentence. Second sentence." / "" / "A new paragraph here!" / "" / "No terminator line"
        // Paragraphs: only the first and third line have terminators.
        assert_eq!(metrics.paragraphs, 2);
        assert_eq!(metrics.sentences, 3);
        // 11 words -> ceil(11 * 60 / 180) = ceil(3.667) = 4s
        assert_eq!(metrics.reading_secs, 4);
    }

    #[test]
    fn question_and_exclamation_marks_count_as_terminators() {
        let metrics = compute("Are you sure? Yes!");

        assert_eq!(metrics.sentences, 2);
        assert_eq!(metrics.paragraphs, 1);
        assert_eq!(metrics.words, 4);
    }

    #[test]
    fn blank_lines_and_headings_without_terminator_do_not_count_as_paragraphs() {
        let text = "# Heading\n\nbody text.\n\n---\n\n> quote line with no terminator";
        let metrics = compute(text);

        // Only "body text." qualifies.
        assert_eq!(metrics.paragraphs, 1);
        assert_eq!(metrics.sentences, 1);
    }

    #[test]
    fn reading_time_rounds_up_to_next_second() {
        // 181 words at 180 wpm is just over one minute.
        let words_text = "word ".repeat(181);
        let metrics = compute(&words_text);

        assert_eq!(metrics.words, 181);
        assert_eq!(metrics.reading_secs, 61);
    }

    #[test]
    fn reading_time_hits_a_clean_minute() {
        // 360 words at 180 wpm = exactly 2 minutes.
        let words_text = "word ".repeat(360);
        let metrics = compute(&words_text);

        assert_eq!(metrics.reading_secs, 120);
    }

    #[test]
    fn reading_time_for_zero_words_is_zero() {
        let metrics = compute("   \n\n  \n");
        assert_eq!(metrics.reading_secs, 0);
        // Lines are all blank, so zero paragraphs even though there are 3
        // split_whitespace tokens? No: split_whitespace skips them, so 0.
        assert_eq!(metrics.words, 0);
        assert_eq!(metrics.paragraphs, 0);
    }

    #[test]
    fn characters_count_includes_whitespace_and_newlines() {
        // "a b\n" is 4 chars.
        let metrics = compute("a b\n");
        assert_eq!(metrics.characters, 4);
    }

    #[test]
    fn ellipsis_counts_each_dot_as_a_sentence_terminator() {
        // Per the simple-text-file spec, "Wait..." terminates three sentences.
        let metrics = compute("Wait...");

        assert_eq!(metrics.sentences, 3);
        // It still terminates, so the line is one paragraph.
        assert_eq!(metrics.paragraphs, 1);
    }

    #[test]
    fn format_reading_time_renders_compact_labels() {
        assert_eq!(format_reading_time(0), "0s");
        assert_eq!(format_reading_time(45), "45s");
        assert_eq!(format_reading_time(59), "59s");
        assert_eq!(format_reading_time(60), "1m");
        assert_eq!(format_reading_time(66), "1m 6s");
        assert_eq!(format_reading_time(120), "2m");
        assert_eq!(format_reading_time(125), "2m 5s");
    }

    #[test]
    fn is_sentence_terminator_only_matches_ascii_punctuation() {
        assert!(is_sentence_terminator('.'));
        assert!(is_sentence_terminator('!'));
        assert!(is_sentence_terminator('?'));
        assert!(!is_sentence_terminator(','));
        assert!(!is_sentence_terminator(':'));
        assert!(!is_sentence_terminator(';'));
        assert!(!is_sentence_terminator(' '));
        // Non-ASCII fullwidth equivalents are intentionally not counted.
        assert!(!is_sentence_terminator('。'));
    }
}
