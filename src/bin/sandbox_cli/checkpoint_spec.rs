use anyhow::{anyhow, Context, Result};

/// Parse a checkpoint specification string into checkpoint numbers.
///
/// Supported formats:
/// - single: `239615926`
/// - range: `239615920..239615926` (inclusive)
/// - list: `239615920,239615923,239615926`
///
/// `max_range_span` applies only to range format and represents the inclusive
/// number of checkpoints allowed in the range.
pub(crate) fn parse_checkpoint_spec_with_limit(
    spec: &str,
    max_range_span: Option<u64>,
) -> Result<Vec<u64>> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("checkpoint spec cannot be empty"));
    }

    if let Some((start_raw, end_raw)) = trimmed.split_once("..") {
        let start = start_raw
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid checkpoint range start: {}", start_raw.trim()))?;
        let end = end_raw
            .trim()
            .parse::<u64>()
            .with_context(|| format!("invalid checkpoint range end: {}", end_raw.trim()))?;
        if end < start {
            return Err(anyhow!(
                "invalid checkpoint range {}..{}: end must be >= start",
                start,
                end
            ));
        }
        let span = end - start + 1;
        if let Some(limit) = max_range_span {
            if span > limit {
                return Err(anyhow!(
                    "checkpoint range too large ({} checkpoints, max {})",
                    span,
                    limit
                ));
            }
        }
        return Ok((start..=end).collect());
    }

    if trimmed.contains(',') {
        let mut out = Vec::new();
        for part in trimmed.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let checkpoint = part
                .parse::<u64>()
                .with_context(|| format!("invalid checkpoint in list: {}", part))?;
            out.push(checkpoint);
        }
        if out.is_empty() {
            return Err(anyhow!("checkpoint list is empty"));
        }
        return Ok(out);
    }

    let single = trimmed
        .parse::<u64>()
        .with_context(|| format!("invalid checkpoint: {}", trimmed))?;
    Ok(vec![single])
}

#[cfg(test)]
mod tests {
    use super::parse_checkpoint_spec_with_limit;

    #[test]
    fn parses_single_checkpoint() {
        let out = parse_checkpoint_spec_with_limit("239615926", Some(100)).expect("single");
        assert_eq!(out, vec![239615926]);
    }

    #[test]
    fn parses_range_checkpoint() {
        let out = parse_checkpoint_spec_with_limit("10..12", Some(100)).expect("range");
        assert_eq!(out, vec![10, 11, 12]);
    }

    #[test]
    fn parses_list_checkpoint_preserves_order() {
        let out = parse_checkpoint_spec_with_limit("5,3,5,7", Some(100)).expect("list");
        assert_eq!(out, vec![5, 3, 5, 7]);
    }

    #[test]
    fn rejects_inverted_range() {
        let err = parse_checkpoint_spec_with_limit("20..10", Some(100)).expect_err("inverted");
        assert!(err.to_string().contains("end must be >= start"));
    }

    #[test]
    fn rejects_range_span_above_limit() {
        let err = parse_checkpoint_spec_with_limit("1..200", Some(100)).expect_err("too large");
        assert!(err.to_string().contains("checkpoint range too large"));
    }
}
