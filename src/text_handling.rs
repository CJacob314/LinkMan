use anyhow::{Result, anyhow};
use strip_ansi_escapes::strip_str;

/// Returns a reference ([`&str`]) the word at the given position in the given lines of text.
///
/// # NOTE
/// This function is `unsafe` because it can only be called from a single-threaded context.
/// This is due to the fact that it uses a `static mut` map to cache computations.
pub unsafe fn word_at_position(
    lines: &[String],
    scroll: usize,
    row: usize,
    mut col: usize,
) -> Option<&str> {
    use unicode_segmentation::UnicodeSegmentation;

    col = col.checked_sub(1)?;

    // Module in place to prevent accidental direct use of `static mut` pointer `LINE_OFFSETS_CACHE`.
    mod offsets_cache {
        use std::collections::HashMap;
        use std::ptr;
        static mut LINE_OFFSETS_CACHE: *mut HashMap<String, Vec<usize>> = ptr::null_mut();

        pub(super) fn get_cache<'a>() -> &'a mut HashMap<String, Vec<usize>> {
            unsafe {
                if LINE_OFFSETS_CACHE.is_null() {
                    LINE_OFFSETS_CACHE = Box::into_raw(Box::new(HashMap::new()));
                }
                &mut *LINE_OFFSETS_CACHE
            }
        }
    }

    let line = lines.get(row.checked_add(scroll)?.checked_sub(1)?)?;

    // Group line by Unicode extended grapheme clusters, as recommended by [UAX #29](https://www.unicode.org/reports/tr29/#Grapheme_Cluster_Boundaries)
    let graphemes: Vec<&str> = UnicodeSegmentation::graphemes(line.as_str(), true).collect();

    if col >= graphemes.len() {
        // Column is out of bounds
        return None;
    }

    // If the grapheme is whitespace, return None
    if graphemes[col].chars().all(char::is_whitespace) {
        return None;
    }

    // Walk backward to find the start of the word
    let mut start = col;
    while start > 0
        && !graphemes[start - 1]
            .chars()
            .all(|c| char::is_whitespace(c) || c == '/' || c == '(' || c == ')')
    {
        start -= 1;
    }

    // Walk forward to find the end of the word
    let mut end = col;
    while end < graphemes.len()
        && !graphemes[end]
            .chars()
            .all(|c| char::is_whitespace(c) || c == '/')
    {
        end += 1;
    }

    // TODO: Benchmark this code with vs. without the cache and use whichever version was faster
    let offsets_cache = offsets_cache::get_cache();
    if let Some(offsets) = offsets_cache.get(line.as_str()) {
        // Cached offsets were present. Use those to compute returned string slice.
        let start_byte = offsets[start];
        let end_byte = offsets[end];

        Some(&line[start_byte..end_byte])
    } else {
        // Compute cached offsets
        let mut byte_offsets = Vec::with_capacity(graphemes.len() + 1);
        let mut offset_accum = 0;
        byte_offsets.push(offset_accum);
        for grapheme in &graphemes {
            offset_accum += grapheme.len();
            byte_offsets.push(offset_accum);
        }

        let start_byte = byte_offsets[start];
        let end_byte = byte_offsets[end];

        // Update cache
        offsets_cache.insert(line.clone(), byte_offsets);

        Some(&line[start_byte..end_byte])
    }
}

pub(crate) fn get_man_string(s: &str) -> Result<String> {
    Ok(strip_str(
        &s[..s
            .find(char::is_whitespace)
            .ok_or_else(|| anyhow!("No whitespace in entire man page"))?],
    ))
}
