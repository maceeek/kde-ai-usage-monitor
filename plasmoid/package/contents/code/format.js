.pragma library

/// Render a duration the way the backend's text output does, so the applet and
/// the CLI never disagree about how long is left. Kept in QML as well as Rust
/// because the countdown ticks between polls.
function duration(seconds) {
    if (seconds === undefined || seconds === null || seconds <= 0) {
        return "now";
    }

    var days = Math.floor(seconds / 86400);
    var hours = Math.floor((seconds % 86400) / 3600);
    var minutes = Math.floor((seconds % 3600) / 60);

    if (days > 0) {
        return hours > 0 ? days + "d " + hours + "h" : days + "d";
    }
    if (hours > 0) {
        return minutes > 0 ? hours + "h " + minutes + "m" : hours + "h";
    }
    if (minutes > 0) {
        return minutes + "m";
    }
    return Math.floor(seconds) + "s";
}

function percentage(value) {
    if (value === undefined || value === null) {
        return "–";
    }
    if (value > 0 && value < 10) {
        return value.toFixed(1) + "%";
    }
    return Math.round(value) + "%";
}

/// Seconds left in a window, recomputed against the wall clock rather than
/// trusting the value the last poll baked in.
function secondsUntil(resetsAt, now) {
    if (!resetsAt) {
        return undefined;
    }
    var remaining = resetsAt - Math.floor(now / 1000);
    return remaining > 0 ? remaining : 0;
}
