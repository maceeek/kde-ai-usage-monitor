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

/// The headline windows a provider reported, in display order. Any of them can
/// be missing: Copilot has no short window, and a Codex account on a spending
/// cap may have only the monthly one.
function headlineSections(provider) {
    if (!provider) {
        return [];
    }
    return [provider.session, provider.weekly, provider.monthly].filter(function (section) {
        return !!section;
    });
}

/// Every window worth listing in the popup and the tooltip: the headline ones,
/// then per-model and other secondary quotas.
function allSections(provider) {
    return headlineSections(provider).concat(provider && provider.limits ? provider.limits : []);
}

/// `Locale.ShortFormat`. The `Locale` type is not in scope inside a
/// `.pragma library` file, so its value is spelled out.
var SHORT_FORMAT = 1;

/// Wall-clock time a window resets, as the user's locale writes it. Resets more
/// than a day out get the weekday too, since "08:00" alone would be ambiguous.
function resetClock(resetsAt, now) {
    if (!resetsAt) {
        return "";
    }
    var date = new Date(resetsAt * 1000);
    var locale = Qt.locale();
    var time = date.toLocaleTimeString(locale, SHORT_FORMAT);
    if (resetsAt * 1000 - now > 20 * 3600 * 1000) {
        return locale.dayName(date.getDay(), SHORT_FORMAT) + " " + time;
    }
    return time;
}

/// "1h 12m · 17:40": how long, and when.
function resetSummary(resetsAt, now) {
    var remaining = secondsUntil(resetsAt, now);
    if (remaining === undefined) {
        return "";
    }
    return duration(remaining) + " · " + resetClock(resetsAt, now);
}

/// Plan names arrive lower-case from the APIs ("max", "plus", "enterprise").
function planName(plan) {
    if (!plan) {
        return "";
    }
    return plan.split(/[_\s]+/).map(function (word) {
        return word.charAt(0).toUpperCase() + word.slice(1);
    }).join(" ");
}
