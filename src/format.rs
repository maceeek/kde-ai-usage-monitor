//! Human-facing formatting shared by the CLI's text output and the JSON the
//! applet renders.

/// Render a duration the way the upstream widget does: the largest unit that
/// still says something useful, never more than two.
pub fn humanize_duration(seconds: i64) -> String {
    if seconds <= 0 {
        return "now".to_string();
    }

    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

/// Percentages are shown without decimals above 10% — a panel widget has no
/// room for noise, and a tenth of a percent changes nothing for the reader.
pub fn format_percentage(percentage: f64) -> String {
    if percentage > 0.0 && percentage < 10.0 {
        format!("{percentage:.1}%")
    } else {
        format!("{:.0}%", percentage.round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_shrink_to_their_largest_useful_unit() {
        assert_eq!(humanize_duration(0), "now");
        assert_eq!(humanize_duration(-5), "now");
        assert_eq!(humanize_duration(45), "45s");
        assert_eq!(humanize_duration(90), "1m");
        assert_eq!(humanize_duration(3_600), "1h");
        assert_eq!(humanize_duration(3_720), "1h 2m");
        assert_eq!(humanize_duration(86_400), "1d");
        assert_eq!(humanize_duration(90_000), "1d 1h");
    }

    #[test]
    fn small_percentages_keep_one_decimal() {
        assert_eq!(format_percentage(0.0), "0%");
        assert_eq!(format_percentage(4.25), "4.2%");
        assert_eq!(format_percentage(42.6), "43%");
        assert_eq!(format_percentage(100.0), "100%");
    }
}
